#!/usr/bin/env bash
# run-qemu.sh - boot a built OxysOS ISO in QEMU for testing.
#
#   ./scripts/run-qemu.sh                       # newest ISO, UEFI default
#   ./scripts/run-qemu.sh bios                  # newest ISO, legacy BIOS
#   ./scripts/run-qemu.sh disk=4G               # use a 4 GiB install target disk
#   ./scripts/run-qemu.sh no-disk               # boot without an install target
#   ./scripts/run-qemu.sh persist               # attach 4 GiB OXYS_PERSIST disk
#   ./scripts/run-qemu.sh /path/to/oxysos.iso   # explicit ISO path
#   ./scripts/run-qemu.sh out/foo.iso bios persist
#
# Set OXYS_ISO_DIR to override the auto-detected ISO search directory.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_DIR="$(cd "${SCRIPT_DIR}/.." && pwd)"
DEFAULT_CATALYST_DIR="${HOME}/catalyst/builds/23.0-default"
FALLBACK_OUT_DIR="${REPO_DIR}/out"
ISO_DIR="${OXYS_ISO_DIR:-}"

ISO=""
MODE="uefi"
PERSIST=""
INSTALL_DISK="1"
INSTALL_DISK_SIZE="${OXYS_DISK_SIZE:-8G}"
INSTALL_DISK_PATH="${OXYS_DISK:-}"

usage() {
    cat >&2 <<EOF
Usage: $0 [iso] [uefi|bios] [disk=SIZE|no-disk] [persist]

Examples:
  $0
  $0 bios
  $0 disk=4G
  $0 no-disk
  $0 persist
  $0 /path/to/oxysos-amd64.iso bios persist

Environment:
  OXYS_ISO_DIR  Directory to search for the newest .iso
  OXYS_DISK     Path for the install target qcow2 disk
  OXYS_DISK_SIZE
                Size for a newly created install target disk, default: 8G
  OXYS_RES      Guest display resolution WIDTHxHEIGHT, default: 1280x800
  GL=0          Disable virgl/OpenGL display acceleration
EOF
}

for arg in "$@"; do
    if [[ -z "$ISO" && -f "$arg" ]]; then
        ISO="$arg"
    elif [[ "$arg" == "bios" || "$arg" == "uefi" ]]; then
        MODE="$arg"
    elif [[ "$arg" == "persist" ]]; then
        PERSIST="persist"
    elif [[ "$arg" == "no-disk" ]]; then
        INSTALL_DISK=""
    elif [[ "$arg" == disk=* ]]; then
        INSTALL_DISK="1"
        INSTALL_DISK_SIZE="${arg#disk=}"
    elif [[ "$arg" == "-h" || "$arg" == "--help" ]]; then
        usage
        exit 0
    elif [[ "$arg" == *.iso || "$arg" == *.ISO ]]; then
        ISO="$arg"
    else
        echo "Unknown argument: $arg" >&2
        usage
        exit 1
    fi
done

find_newest_iso() {
    local dir="$1"

    [[ -d "$dir" ]] || return 1

    shopt -s nullglob
    local candidates=("$dir"/*.iso)
    shopt -u nullglob

    [[ ${#candidates[@]} -gt 0 ]] || return 1

    ls -t "${candidates[@]}" | head -n 1
}

if [[ -z "$ISO" ]]; then
    search_dirs=()
    if [[ -n "$ISO_DIR" ]]; then
        search_dirs+=("$ISO_DIR")
    else
        search_dirs+=("$DEFAULT_CATALYST_DIR" "$FALLBACK_OUT_DIR")
    fi

    for dir in "${search_dirs[@]}"; do
        if ISO="$(find_newest_iso "$dir")"; then
            break
        fi
        ISO=""
    done

    if [[ -z "$ISO" ]]; then
        echo "No .iso found. Build one with ./build.sh, pass an ISO path, or set OXYS_ISO_DIR." >&2
        echo "Searched:" >&2
        printf '  %s\n' "${search_dirs[@]}" >&2
        exit 1
    fi
fi

[[ -f "$ISO" ]] || { echo "ISO not found: $ISO" >&2; exit 1; }
command -v qemu-system-x86_64 >/dev/null || { echo "Install qemu-system-x86_64/qemu-full first." >&2; exit 1; }

# Boot order is decided by content, not forced to the CD. The install disk gets
# the highest boot priority (bootindex=1, set where the disk is attached below),
# so:
#   - first run: the disk is empty, firmware finds nothing bootable on it and
#     fails over to the CD -> the installer runs;
#   - after install + reboot: the disk now has an ESP (grub --removable), so
#     firmware boots the installed system instead of the live CD.
# `menu=on` lets you press Esc/F12 at the firmware screen to pick the CD manually
# (e.g. to reinstall over an existing install). The old `-boot d` forced the CD
# on every boot, so a post-install reboot always dropped back to the live medium.
ARGS=(
    -enable-kvm
    -m 8192
    -smp 2
    -machine q35
    -cpu host
    -boot menu=on
    -cdrom "$ISO"
    -serial mon:stdio
)

# Display resolution. The virtio GPU otherwise advertises a small 1280x800 EDID
# mode, so the guest desktop and the SDL window come up cramped. OXYS_RES=WxH
# sets the GPU's preferred resolution reported to the guest via EDID; the
# compositor picks it up on boot. Override e.g. OXYS_RES=2560x1440.
RES="${OXYS_RES:-1280x800}"
if [[ ! "$RES" =~ ^[0-9]+x[0-9]+$ ]]; then
    echo ":: OXYS_RES='$RES' is not WIDTHxHEIGHT; falling back to 1280x800." >&2
    RES="1280x800"
fi
XRES="${RES%x*}"
YRES="${RES#*x}"

if [[ "${GL:-1}" == "1" ]]; then
    ARGS+=(-device "virtio-vga-gl,xres=${XRES},yres=${YRES}" -display sdl,gl=on)
else
    echo ":: GL=0 - no host GL acceleration; Niri/Wayland may not render." >&2
    ARGS+=(-device "virtio-gpu-pci,xres=${XRES},yres=${YRES}" -display sdl)
fi

if [[ "$MODE" == "uefi" ]]; then
    OVMF=""
    for c in \
        /usr/share/edk2/x64/OVMF_CODE.4m.fd \
        /usr/share/edk2-ovmf/x64/OVMF_CODE.fd \
        /usr/share/OVMF/OVMF_CODE.fd; do
        [[ -f "$c" ]] && { OVMF="$c"; break; }
    done
    [[ -n "$OVMF" ]] || { echo "OVMF firmware not found; install edk2-ovmf/ovmf." >&2; exit 1; }
    ARGS+=(-drive "if=pflash,format=raw,readonly=on,file=${OVMF}")
fi

if [[ -n "$INSTALL_DISK" ]]; then
    command -v qemu-img >/dev/null || { echo "qemu-img not found; install qemu tools first." >&2; exit 1; }

    if [[ -z "$INSTALL_DISK_PATH" ]]; then
        INSTALL_DISK_PATH="$(dirname "$ISO")/oxys-install.qcow2"
    fi

    if [[ ! -f "$INSTALL_DISK_PATH" ]]; then
        qemu-img create -f qcow2 "$INSTALL_DISK_PATH" "$INSTALL_DISK_SIZE" >/dev/null
        echo ":: Created install target disk $INSTALL_DISK_PATH (${INSTALL_DISK_SIZE})"
    fi
    # bootindex=1 makes the firmware try the install disk before the CD. When the
    # disk is empty this cleanly fails over to the CD (installer); once installed
    # it boots the disk. Attached explicitly (if=none + device) because bootindex
    # is a device property, not a `-drive` shorthand option.
    ARGS+=(
        -drive "file=${INSTALL_DISK_PATH},if=none,id=oxysdisk,format=qcow2"
        -device "virtio-blk-pci,drive=oxysdisk,bootindex=1"
    )
fi

if [[ "$PERSIST" == "persist" ]]; then
    command -v qemu-img >/dev/null || { echo "qemu-img not found; install qemu tools first." >&2; exit 1; }

    DISK="$(dirname "$ISO")/oxys-persist.qcow2"
    if [[ ! -f "$DISK" ]]; then
        qemu-img create -f qcow2 "$DISK" 4G >/dev/null
        echo ":: Created persistence disk $DISK"
        echo ":: Format it once from inside OxysOS:"
        echo "   mkfs.ext4 -L OXYS_PERSIST <attached-virtio-disk>"
    fi
    ARGS+=(-drive "file=${DISK},if=virtio,format=qcow2")
fi

echo ":: Launching QEMU with $(basename "$ISO") (${MODE}${INSTALL_DISK:+, install disk}${PERSIST:+, $PERSIST}) ..."
exec qemu-system-x86_64 "${ARGS[@]}"
