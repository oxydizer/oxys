#!/usr/bin/env bash
# Replace only oxys-installer and its bundled configs in an existing ISO.
# Catalyst, the kernel, and the live package set are not rebuilt.
set -euo pipefail
ORIGINAL_ARGS=("$@")

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ISO_DIR="$(cd "${SCRIPT_DIR}/.." && pwd)"
MONOREPO_ROOT="$(cd "${ISO_DIR}/.." && pwd)"
INSTALLER_DIR="${MONOREPO_ROOT}/oxys-installer"
TARGET="x86_64-unknown-linux-musl"
CARGO_TARGET_ROOT="${CARGO_TARGET_DIR:-${MONOREPO_ROOT}/target}"
INSTALLER_BIN="${CARGO_TARGET_ROOT}/${TARGET}/release/oxys-installer"

usage() {
	cat <<EOF
Usage: $(basename "$0") [input.iso] [output.iso]

Rebuild only oxys-installer and inject it, plus configs/*.fe2o3, into an
existing OxysOS ISO. If input.iso is omitted, the newest ISO is selected from
OXYS_ISO_DIR, ~/catalyst/builds/23.0-default, or the current directory.
EOF
}

case "${1:-}" in
	-h|--help) usage; exit 0 ;;
esac

for command in cargo xorriso unsquashfs mksquashfs sha256sum; do
	command -v "${command}" >/dev/null 2>&1 || {
		echo "ERROR: required command not found: ${command}" >&2
		exit 1
	}
done

input_iso="${1:-}"
if [[ -z "${input_iso}" ]]; then
	search_dirs=(
		"${OXYS_ISO_DIR:-}"
		"${HOME}/catalyst/builds/23.0-default"
		"${PWD}"
	)
	for dir in "${search_dirs[@]}"; do
		[[ -n "${dir}" && -d "${dir}" ]] || continue
		input_iso="$(find "${dir}" -maxdepth 1 -type f -name 'oxysos-*.iso' -printf '%T@ %p\n' 2>/dev/null \
			| sort -rn | head -n1 | cut -d' ' -f2-)"
		[[ -n "${input_iso}" ]] && break
	done
fi

[[ -n "${input_iso}" && -f "${input_iso}" ]] || {
	echo "ERROR: no input ISO found; pass its path explicitly." >&2
	exit 1
}
input_iso="$(realpath "${input_iso}")"

stamp="$(date -u +%Y%m%dT%H%M%SZ)"
output_iso="${2:-${input_iso%.iso}-installer-${stamp}.iso}"
output_iso="$(realpath -m "${output_iso}")"
[[ "${input_iso}" != "${output_iso}" ]] || {
	echo "ERROR: output must differ from input; the source ISO is never modified in place." >&2
	exit 1
}
[[ ! -e "${output_iso}" ]] || {
	echo "ERROR: output already exists: ${output_iso}" >&2
	exit 1
}
mkdir -p "$(dirname "${output_iso}")"

if command -v rustup >/dev/null 2>&1; then
	rustup target add "${TARGET}" >/dev/null
fi

if [[ "${OXYS_INSTALLER_ALREADY_BUILT:-0}" != "1" ]]; then
	echo ">> building oxys-installer (${TARGET})"
	cargo build \
		--manifest-path "${INSTALLER_DIR}/Cargo.toml" \
		--release \
		--target "${TARGET}"
fi
[[ -x "${INSTALLER_BIN}" ]] || {
	echo "ERROR: installer build did not produce ${INSTALLER_BIN}" >&2
	exit 1
}

# Squashfs contains device nodes, root-owned files, and privileged modes.
# fakeroot lets unsquashfs/mksquashfs round-trip that metadata without making
# Cargo or the resulting ISO genuinely root-owned.
if [[ "${EUID}" -ne 0 && -z "${FAKEROOTKEY:-}" ]]; then
	command -v fakeroot >/dev/null 2>&1 || {
		echo "ERROR: fakeroot is required to preserve live-root metadata." >&2
		exit 1
	}
	exec env OXYS_INSTALLER_ALREADY_BUILT=1 \
		CARGO_TARGET_DIR="${CARGO_TARGET_ROOT}" \
		fakeroot -- "$0" "${ORIGINAL_ARGS[@]}"
fi

work="$(mktemp -d "${TMPDIR:-/tmp}/oxys-refresh-installer.XXXXXX")"
cleanup() {
	# ISO9660 extraction preserves read-only directory modes. Restore owner
	# write/search permission so an unprivileged invocation can remove scratch.
	chmod -R u+rwX "${work}" 2>/dev/null || true
	rm -rf "${work}"
}
trap cleanup EXIT

# xorriso writes even `-find ... -exec echo` results to stderr, so merge the
# streams and retain only its single-quoted ISO path result.
squash_path="$(xorriso -indev "${input_iso}" \
	-find / -name image.squashfs -type f -exec echo -- 2>&1 \
	| sed -n "s|^'\(/.*image\.squashfs\)'$|\1|p" \
	| tail -n1)"
[[ -n "${squash_path}" ]] || {
	echo "ERROR: image.squashfs was not found in ${input_iso}" >&2
	exit 1
}

echo ">> extracting ISO filesystem"
xorriso -osirrox on -indev "${input_iso}" \
	-extract / "${work}/iso-root" >/dev/null 2>&1
# ISO9660 directory modes are read-only. Make the scratch copy replaceable;
# this never alters the source ISO.
chmod -R u+rwX "${work}/iso-root"
disk_squash_path="${work}/iso-root${squash_path}"
[[ -f "${disk_squash_path}" ]] || {
	echo "ERROR: extracted squashfs is missing: ${disk_squash_path}" >&2
	exit 1
}
# Device nodes such as /dev/console cannot be recreated by an unprivileged
# caller. They are supplied by devtmpfs at boot, so keep extraction going and
# do not turn those non-fatal warnings into a failed refresh.
unsquashfs -no-exit-code -d "${work}/root" "${disk_squash_path}" >/dev/null

install -D -m 0755 "${INSTALLER_BIN}" "${work}/root/usr/local/bin/oxys-installer"
install -d -m 0755 "${work}/root/root/configs"
for config in base desktop custom; do
	install -m 0644 "${INSTALLER_DIR}/configs/${config}.fe2o3" \
		"${work}/root/root/configs/${config}.fe2o3"
done

squash_info="$(unsquashfs -s "${disk_squash_path}")"
compression="$(awk '/^Compression / {print $2; exit}' <<<"${squash_info}")"
case "${compression}" in
	gzip|lzo|lz4|xz|zstd) ;;
	*) compression=xz ;;
esac
block_size="$(awk '/^Block size / {print $3; exit}' <<<"${squash_info}")"
[[ "${block_size}" =~ ^[0-9]+$ ]] || block_size=1048576
squash_options=(-noappend -comp "${compression}" -b "${block_size}")
if grep -q '^Filesystem is not exportable' <<<"${squash_info}"; then
	squash_options+=(-no-exports)
fi
if grep -q '^Xattrs are not stored' <<<"${squash_info}"; then
	squash_options+=(-no-xattrs)
fi
if grep -q '^Tailends are packed into fragments' <<<"${squash_info}"; then
	squash_options+=(-always-use-fragments)
fi

echo ">> repacking live filesystem (${compression})"
# The extracted live root is large. Remove the old squashfs before creating
# its replacement so /tmp never has to hold two ~1.8 GiB copies at once.
rm -f "${disk_squash_path}"
mksquashfs "${work}/root" "${disk_squash_path}" \
	"${squash_options[@]}" >/dev/null

echo ">> writing refreshed bootable ISO"
volume_id="$(xorriso -indev "${input_iso}" -pvd_info 2>&1 \
	| sed -n 's/^Volume Id *: *//p' | head -n1)"
volume_id="${volume_id:-OxysOS-refresh}"

# Rebuild the hybrid image from the extracted tree. Importing an existing
# Catalyst ISO and asking xorriso to replay its boot setup loses or overlaps
# the EFI/GPT partitions on xorriso 1.5.x. These are the same hybrid GRUB
# options Catalyst used for the source image, including BIOS + UEFI boot.
xorriso -as mkisofs \
	-o "${output_iso}" \
	-V "${volume_id}" \
	--grub2-mbr "--interval:local_fs:0s-15s:zero_mbrpt,zero_gpt,zero_apm:${input_iso}" \
	--protective-msdos-label \
	-partition_cyl_align off \
	-partition_offset 0 \
	-partition_hd_cyl 114 \
	-partition_sec_hd 32 \
	--mbr-force-bootable \
	-apm-block-size 2048 \
	-hfsplus \
	-efi-boot-part --efi-boot-image \
	-c /boot.catalog \
	-b /boot/grub/i386-pc/eltorito.img \
	-no-emul-boot -boot-load-size 4 -boot-info-table --grub2-boot-info \
	-eltorito-alt-boot \
	-e /efi.img -no-emul-boot \
	"${work}/iso-root" >/dev/null 2>&1

boot_report="$(xorriso -indev "${output_iso}" \
	-report_el_torito plain -report_system_area plain 2>&1)"
if ! grep -q 'El Torito boot img.*BIOS.* y ' <<<"${boot_report}" \
	|| ! grep -q 'El Torito boot img.*UEFI.* y ' <<<"${boot_report}" \
	|| ! grep -q 'GPT partition path.* /efi.img' <<<"${boot_report}"; then
	echo "ERROR: refreshed ISO failed BIOS/UEFI boot validation; removing it." >&2
	rm -f "${output_iso}"
	exit 1
fi

output_volume_id="$(xorriso -indev "${output_iso}" -pvd_info 2>&1 \
	| sed -n 's/^Volume Id *: *//p' | head -n1)"
if [[ "${output_volume_id}" != "${volume_id}" ]]; then
	echo "ERROR: refreshed ISO label '${output_volume_id}' does not match source label '${volume_id}'; removing it." >&2
	rm -f "${output_iso}"
	exit 1
fi

input_size="$(stat -c%s "${input_iso}")"
output_size="$(stat -c%s "${output_iso}")"
if (( output_size < input_size / 2 )); then
	echo "ERROR: refreshed ISO is implausibly small; removing it." >&2
	rm -f "${output_iso}"
	exit 1
fi

sha256sum "${output_iso}" > "${output_iso}.sha256"
echo ">> done: ${output_iso}"
echo "   sha256: $(cut -d' ' -f1 "${output_iso}.sha256")"
