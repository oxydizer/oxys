#!/usr/bin/env bash
set -euo pipefail

ARCH_NAME="${OXYS_ARCH:?OXYS_ARCH is required}"
MARCH="${OXYS_MARCH:?OXYS_MARCH is required}"
BUILD_PROFILE="${OXYS_BUILD_PROFILE:?OXYS_BUILD_PROFILE is required}"
WORK_ROOT="${OXYS_WORK_ROOT:-/work}"
SOURCE_ROOT="${OXYS_SOURCE_ROOT:-/src}"
OUTPUT_ROOT="${OXYS_OUTPUT_ROOT:-/out}"
PACKAGE_LIST_FILE="${WORK_ROOT}/oxys-packages.txt"
if [[ ! -f "${PACKAGE_LIST_FILE}" ]]; then
  PACKAGE_LIST_FILE="${SOURCE_ROOT}/oxys-packages.txt"
fi
OVERLAY_ROOT="${WORK_ROOT}/oxys-overlay"
if [[ ! -d "${OVERLAY_ROOT}" ]]; then
  OVERLAY_ROOT="${SOURCE_ROOT}/oxys-overlay"
fi
KERNEL_BASE_CONFIG_FILE="${SOURCE_ROOT}/podman/kernel/base.config"
if [[ ! -f "${KERNEL_BASE_CONFIG_FILE}" ]]; then
  KERNEL_BASE_CONFIG_FILE="${WORK_ROOT}/kernel/base.config"
fi
KERNEL_ARCH_FRAGMENT_FILE="${SOURCE_ROOT}/podman/kernel/${ARCH_NAME}.fragment"
if [[ ! -f "${KERNEL_ARCH_FRAGMENT_FILE}" ]]; then
  KERNEL_ARCH_FRAGMENT_FILE="${WORK_ROOT}/kernel/${ARCH_NAME}.fragment"
fi
KERNEL_BORE_PATCH_FILE="${SOURCE_ROOT}/podman/kernel/bore.patch"
if [[ ! -f "${KERNEL_BORE_PATCH_FILE}" ]]; then
  KERNEL_BORE_PATCH_FILE="${WORK_ROOT}/kernel/bore.patch"
fi
TIMES_LOG="${OUTPUT_ROOT}/${ARCH_NAME}/build-times.tsv"
CONTAINER_LOG="${OUTPUT_ROOT}/${ARCH_NAME}/container.log"
EMERGE_LOG_DIR="${OUTPUT_ROOT}/${ARCH_NAME}/emerge-logs"
KERNEL_STAGE_ROOT="${OUTPUT_ROOT}/${ARCH_NAME}/kernel-stage"
KERNEL_CONFIG_STAGE_DIR="${OUTPUT_ROOT}/${ARCH_NAME}/kernel-config"
BUILD_ID_FILE="${OUTPUT_ROOT}/${ARCH_NAME}/build-id"
KERNEL_RELEASE_FILE="${OUTPUT_ROOT}/${ARCH_NAME}/kernel-release"

readonly BINHOST_BASELINE_USE_FLAGS="X wayland dbus systemd -elogind policykit alsa pipewire pulseaudio vulkan opengl gtk jpeg png webp svg fontconfig harfbuzz udev ssl threads unicode"
readonly GLOBAL_USE_FLAGS="${BINHOST_BASELINE_USE_FLAGS} -debug zfs -california -colorado"
readonly COMMON_FEATURES="parallel-fetch candy"
readonly ACCEPT_KEYWORDS_VALUE="amd64"
readonly FIREFOX_ATOM="www-client/firefox"
readonly PROFILE_TARGET="default/linux/amd64/23.0"
# Gentoo's official binhost for the x86-64-v3 baseline. Only the "generic"
# build profile (utilities where -march tuning barely matters) is allowed to
# pull from it -- "native"/"kernel"/"pgo" exist specifically to produce
# march-tuned/PGO/kernel-paired builds a generic binhost package can't
# provide, so those always build from source.
readonly GENERIC_BINHOST_URL="https://distfiles.gentoo.org/releases/amd64/binpackages/23.0/x86-64-v3/"
readonly OXYS_OVERLAY_REPO="/var/db/repos/oxys"
readonly OXYS_REPOS_CONF_FILE="/etc/portage/repos.conf/oxys.conf"
readonly KERNEL_MASK_FILE="/etc/portage/package.mask/no-kernel"
readonly OXYS_APPLY_BORE="${OXYS_APPLY_BORE:-0}"
readonly OXYS_BORE_PATCH_URL="${OXYS_BORE_PATCH_URL:-}"

build_jobs() {
  nproc --ignore=1
}

readonly COMMON_MAKEOPTS="-j$(build_jobs) -l$(build_jobs)"
# Latest *stable* amd64 as of this pin -- 7.x is still ~amd64 testing.
# zfs-kmod/zfs pinned to the matching stable release (2.3.6, not the ~amd64
# 2.3.8) since ACCEPT_KEYWORDS is stable-only now; OpenZFS 2.3.6 supports
# Linux 4.18-6.19, which comfortably covers this kernel.
readonly KERNEL_SOURCE_ATOM="=sys-kernel/gentoo-sources-6.18.38"
readonly ZFS_KMOD_ATOM="=sys-fs/zfs-kmod-2.3.6"
readonly ZFS_USERLAND_ATOM="=sys-fs/zfs-2.3.6"
BUILD_ID=""

readonly -a KERNEL_PACKAGES=(
  "${KERNEL_SOURCE_ATOM}"
  "${ZFS_KMOD_ATOM}"
  "${ZFS_USERLAND_ATOM}"
)

readonly -a NATIVE_PACKAGES=(
  "media-libs/mesa"
  "sys-libs/glibc"
  "sys-apps/openrc"
  "gui-wm/niri"
  "gui-shells/noctalia"
  "x11-base/xwayland-satellite"
  "media-video/pipewire"
  "media-sound/wireplumber"
  "gui-apps/waybar"
)

readonly -a V3_GENERIC_EXTRAS=(
  "gui-apps/fuzzel"
  "gui-apps/mako"
  "gui-apps/foot"
  "app-shells/fish"
  "gui-apps/swaylock"
  "gui-apps/swayidle"
  "gui-apps/swaybg"
  "net-misc/networkmanager"
  "gui-apps/cliphist"
  "gui-apps/wl-clipboard"
  "sys-auth/polkit"
  "app-admin/sudo"
)

log() {
  local ts
  ts="$(date -u +"%Y-%m-%dT%H:%M:%SZ")"
  printf '[%s] %s\n' "${ts}" "$*" | tee -a "${CONTAINER_LOG}"
}

sanitize_build_id() {
  local raw="$1"
  raw="${raw//[^A-Za-z0-9_.-]/-}"
  while [[ "${raw}" == -* ]]; do
    raw="${raw#-}"
  done
  while [[ "${raw}" == *- ]]; do
    raw="${raw%-}"
  done
  printf '%s\n' "${raw}"
}

portage_snapshot_id() {
  local timestamp_file="/var/db/repos/gentoo/metadata/timestamp"
  local raw

  if [[ ! -f "${timestamp_file}" ]]; then
    printf 'unknown\n'
    return
  fi

  raw="$(<"${timestamp_file}")"
  if date -u -d "${raw}" +"%Y%m%dT%H%M%SZ" >/dev/null 2>&1; then
    date -u -d "${raw}" +"%Y%m%dT%H%M%SZ"
    return
  fi

  date -u -r "${timestamp_file}" +"%Y%m%dT%H%M%SZ"
}

generated_build_id() {
  local portage_snapshot
  portage_snapshot="$(portage_snapshot_id)"
  printf '%s-gentoo-%s\n' "$(date -u +"%Y%m%dT%H%M%SZ")" "${portage_snapshot}"
}

ensure_dirs() {
  mkdir -p "${OUTPUT_ROOT}/${ARCH_NAME}" "${EMERGE_LOG_DIR}" "${KERNEL_STAGE_ROOT}" "${KERNEL_CONFIG_STAGE_DIR}"
  touch "${TIMES_LOG}" "${CONTAINER_LOG}"
  if [[ -n "${OXYS_BUILD_ID:-}" ]]; then
    BUILD_ID="$(sanitize_build_id "${OXYS_BUILD_ID}")"
  elif [[ -f "${BUILD_ID_FILE}" ]]; then
    BUILD_ID="$(sanitize_build_id "$(<"${BUILD_ID_FILE}")")"
    if [[ ! "${BUILD_ID}" =~ ^[0-9]{8}T[0-9]{6}Z-gentoo-[0-9]{8}T[0-9]{6}Z$ ]]; then
      BUILD_ID="$(generated_build_id)"
      printf '%s\n' "${BUILD_ID}" > "${BUILD_ID_FILE}"
    fi
  else
    BUILD_ID="$(generated_build_id)"
    printf '%s\n' "${BUILD_ID}" > "${BUILD_ID_FILE}"
  fi
  if [[ -z "${BUILD_ID}" || ! "${BUILD_ID}" =~ ^[A-Za-z0-9][A-Za-z0-9_.-]*$ ]]; then
    printf 'Invalid build id: %s\n' "${BUILD_ID}" >&2
    exit 1
  fi
  log "Using build id ${BUILD_ID}"
}

write_portage_config() {
  # Only the "generic" profile is allowed to pull from Gentoo's official
  # binhost -- see GENERIC_BINHOST_URL above. native/kernel/pgo always build
  # from source, so they get no PORTAGE_BINHOST and no getbinpkg feature.
  local features="${COMMON_FEATURES}"
  local binhost_line=""
  if [[ "${BUILD_PROFILE}" == "generic" ]]; then
    features="${COMMON_FEATURES} getbinpkg"
    binhost_line="PORTAGE_BINHOST=\"${GENERIC_BINHOST_URL}\""
  fi

  cat > /etc/portage/make.conf <<EOF_MAKE
COMMON_FLAGS="-O3 -march=${MARCH} -pipe"
CFLAGS="\${COMMON_FLAGS}"
CXXFLAGS="\${COMMON_FLAGS}"
FCFLAGS="\${COMMON_FLAGS}"
FFLAGS="\${COMMON_FLAGS}"
RUSTFLAGS="-C target-cpu=${MARCH}"
MAKEOPTS="${COMMON_MAKEOPTS}"
FEATURES="${features}"
USE="${GLOBAL_USE_FLAGS} -dist-kernel"
ACCEPT_KEYWORDS="${ACCEPT_KEYWORDS_VALUE}"
EMERGE_DEFAULT_OPTS="--ask=n --verbose --keep-going=y --with-bdeps=y --jobs=1 --load-average=$(build_jobs)"
PKGDIR="/var/cache/binpkgs"
PORTAGE_BINHOST_HEADER_URI=""
${binhost_line}
# Keep QEMU's virtio/virgl renderer in the reusable desktop package set.
VIDEO_CARDS="intel radeon radeonsi amdgpu virgl"
INPUT_DEVICES="libinput"
LLVM_TARGETS="X86 AMDGPU"
KERNEL="manual"
EOF_MAKE

  mkdir -p /etc/portage/package.use /etc/portage/package.accept_keywords /etc/portage/package.mask /etc/portage/repos.conf /etc/portage/env /etc/portage/package.env
  if [[ -d "${OVERLAY_ROOT}" ]]; then
    rm -rf "${OXYS_OVERLAY_REPO}"
    cp -a "${OVERLAY_ROOT}" "${OXYS_OVERLAY_REPO}"
    cat > "${OXYS_REPOS_CONF_FILE}" <<EOF_REPO
[oxys]
location = ${OXYS_OVERLAY_REPO}
masters = gentoo
auto-sync = no
EOF_REPO
  fi

  cat > /etc/portage/package.use/oxys <<'EOF_USE'
www-client/firefox pgo wayland
media-video/pipewire sound-server
gui-wm/niri screencast
gui-shells/noctalia jemalloc
EOF_USE

  cat > /etc/portage/package.accept_keywords/oxys <<'EOF_KEYWORDS'
gui-wm/niri ~amd64
gui-shells/noctalia **
x11-base/xwayland-satellite ~amd64
gui-apps/waybar ~amd64
media-video/pipewire ~amd64
media-sound/wireplumber ~amd64
www-client/firefox ~amd64
EOF_KEYWORDS

  # -O2 override for sys-fs/zfs-kmod ONLY.
  # ------------------------------------------------------------------------
  # Global CFLAGS above use -O3 (see write_portage_config). -O3 is not a
  # supported build level for either the Linux kernel or OpenZFS, and it
  # miscompiles OpenZFS's vendored in-kernel ZSTD: an -O3 zfs.ko oopsed the
  # kernel inside zfs_ZSTD_decompressStream_simpleArgs (null-page read) on the
  # first read-back of zstd-compressed blocks during the installer's rsync,
  # taking down the whole install. The kernel proper is unaffected (it's built
  # with a plain `make`, so Kbuild's own -O2 applies), but zfs-kmod is emerged
  # through Portage and inherits these CFLAGS. Appending -O2 makes gcc use -O2
  # (the last -O wins) while keeping the exact -march/-pipe from the global
  # flags -- so we don't duplicate the arch string here. Scoped to zfs-kmod via
  # package.env so the rest of userland keeps its aggressive -O3.
  cat > /etc/portage/env/zfs-kmod-no-o3.conf <<'EOF_ENV'
CFLAGS="${CFLAGS} -O2"
CXXFLAGS="${CXXFLAGS} -O2"
EOF_ENV
  cat > /etc/portage/package.env/zfs-kmod <<'EOF_PKGENV'
sys-fs/zfs-kmod zfs-kmod-no-o3.conf
EOF_PKGENV

  restore_kernel_mask
}

restore_kernel_mask() {
  cat > "${KERNEL_MASK_FILE}" <<'EOF_MASK'
sys-kernel/gentoo-sources
EOF_MASK
}

remove_kernel_mask() {
  rm -f "${KERNEL_MASK_FILE}"
}

ensure_profile() {
  local profile_link="/etc/portage/make.profile"
  local profile_target="/var/db/repos/gentoo/profiles/${PROFILE_TARGET}"
  if [[ ! -L "${profile_link}" || "$(readlink "${profile_link}" || true)" != "${profile_target}" ]]; then
    rm -f "${profile_link}"
    ln -s "${profile_target}" "${profile_link}"
  fi
}

array_contains() {
  local needle="$1"
  shift
  local item
  for item in "$@"; do
    if [[ "${item}" == "${needle}" ]]; then
      return 0
    fi
  done
  return 1
}

read_all_atoms() {
  awk '
    /^[[:space:]]*#/ { next }
    /^[[:space:]]*$/ { next }
    {
      atom = $1
      if (atom ~ /^[A-Za-z0-9+_.-]+\/[A-Za-z0-9+_.@-]+$/) {
        print atom
      }
    }
  ' "${PACKAGE_LIST_FILE}"
}

build_package_queue() {
  local atom
  case "${BUILD_PROFILE}" in
    kernel)
      printf '%s\n' "${KERNEL_PACKAGES[@]}"
      ;;
    native)
      printf '%s\n' "${NATIVE_PACKAGES[@]}"
      ;;
    generic)
      printf '%s\n' "${V3_GENERIC_EXTRAS[@]}"
      while IFS= read -r atom; do
        if array_contains "${atom}" "${KERNEL_PACKAGES[@]}"; then
          continue
        fi
        if array_contains "${atom}" "${NATIVE_PACKAGES[@]}"; then
          continue
        fi
        if [[ "${atom}" == "${FIREFOX_ATOM}" ]]; then
          continue
        fi
        printf '%s\n' "${atom}"
      done < <(read_all_atoms)
      ;;
    pgo)
      printf '%s\n' "${FIREFOX_ATOM}"
      ;;
    *)
      log "Unknown build profile: ${BUILD_PROFILE}"
      exit 1
      ;;
  esac | awk '!seen[$0]++'
}

package_slug() {
  local atom="$1"
  atom="${atom#=}"
  printf '%s' "${atom##*/}"
}

package_key() {
  local atom="$1"
  atom="${atom#=}"
  printf '%s\n' "${atom%-[0-9]*}"
}

resolved_pf() {
  local atom="$1"
  atom="$(package_key "${atom}")"
  local category="${atom%%/*}"
  local package="${atom##*/}"
  local pkg_dir="/var/db/pkg/${category}"

  find "${pkg_dir}" -maxdepth 1 -mindepth 1 -type d -name "${package}-[0-9]*" -printf '%f\n' \
    | sort -V \
    | tail -n 1
}

package_version() {
  local pf="$1"
  local pkg_name="$2"
  printf '%s\n' "${pf#${pkg_name}-}"
}

copy_kernel_configs_for_archive() {
  mkdir -p "${KERNEL_CONFIG_STAGE_DIR}"
  cp "${KERNEL_BASE_CONFIG_FILE}" "${KERNEL_CONFIG_STAGE_DIR}/base.config"
  cp "${KERNEL_ARCH_FRAGMENT_FILE}" "${KERNEL_CONFIG_STAGE_DIR}/${ARCH_NAME}.fragment"
  cp /usr/src/linux/.config "${KERNEL_CONFIG_STAGE_DIR}/merged.config"
}

archive_from_vdb() {
  local atom="$1"
  local suffix="$2"
  local merge_start_epoch="$3"
  local pkg_name
  pkg_name="$(package_slug "$(package_key "${atom}")")"
  local pf
  pf="$(resolved_pf "${atom}")"
  if [[ -z "${pf}" ]]; then
    log "Could not resolve installed package for ${atom}"
    exit 1
  fi

  local version
  version="$(package_version "${pf}" "${pkg_name}")"
  local category="${atom#=}"
  category="${category%%/*}"
  local vdb_dir="/var/db/pkg/${category}/${pf}"
  local archive_base="${pkg_name}"
  if [[ "$(package_key "${atom}")" == "www-client/firefox" ]]; then
    archive_base="firefox"
  fi
  local archive_name="${archive_base}-${suffix}-${version}.tar.gz"
  local archive_path="${OUTPUT_ROOT}/${ARCH_NAME}/${archive_name}"
  local manifest_raw manifest
  manifest_raw="$(mktemp)"
  manifest="$(mktemp)"

  awk '
    $1 == "obj" || $1 == "sym" || $1 == "dir" {
      print substr($2, 2)
    }
  ' "${vdb_dir}/CONTENTS" | sort -u > "${manifest_raw}"

  # Some packages' CONTENTS include files this merge did not actually
  # write -- observed with sys-fs/zfs, whose CONTENTS lists every sibling
  # file under shared directories it merely touches (e.g. every other
  # package's module under /lib64/security, not just its own
  # pam_zfs_key.so), with those unrelated files carrying the *original*
  # package's old mtime, not this merge's. Filtering the manifest down to
  # paths with an on-disk mtime at or after this merge's start keeps only
  # what this package actually (re)wrote, regardless of why CONTENTS
  # over-lists it.
  (cd / && xargs -a "${manifest_raw}" -d '\n' stat -c '%Y %n' -- 2>/dev/null) \
    | awk -v since="$((merge_start_epoch - 5))" '$1 >= since { $1=""; sub(/^ /, ""); print }' \
    > "${manifest}"
  rm -f "${manifest_raw}"

  # --no-recursion: the manifest already lists every file/symlink/dir this
  # package owns individually (from CONTENTS). Without this flag, GNU tar
  # recursively archives the FULL contents of any bare directory entry in
  # --files-from -- and CONTENTS commonly includes high-level shared dirs
  # (e.g. /usr/src, /etc) that a package merely touches, not owns. That
  # silently swept in hundreds of thousands of unrelated files (the entire
  # kernel source tree, in one observed case) into what should have been a
  # package-scoped archive.
  tar -C / -czf "${archive_path}" --numeric-owner --owner=0 --group=0 --no-recursion --files-from "${manifest}"
  rm -f "${manifest}"
  write_archive_metadata "${archive_path}" "${atom}" "${version}" ""
  log "Created ${archive_name}"
}

write_archive_metadata() {
  local archive_path="$1"
  local atom="$2"
  local version="$3"
  local kernel_release="$4"
  local metadata_path="${archive_path%.tar.gz}.metadata"

  if [[ -z "${kernel_release}" && -f "${KERNEL_RELEASE_FILE}" ]]; then
    kernel_release="$(<"${KERNEL_RELEASE_FILE}")"
  fi

  {
    printf 'build_id=%s\n' "${BUILD_ID}"
    printf 'arch=%s\n' "${ARCH_NAME}"
    printf 'build_profile=%s\n' "${BUILD_PROFILE}"
    printf 'atom=%s\n' "${atom#=}"
    printf 'version=%s\n' "${version}"
    printf 'kernel_release=%s\n' "${kernel_release}"
    printf 'archive=%s\n' "$(basename "${archive_path}")"
    printf 'created_utc=%s\n' "$(date -u +"%Y-%m-%dT%H:%M:%SZ")"
  } > "${metadata_path}"
}

apply_bore_patch() {
  local source_dir="$1"
  local patch_marker="${source_dir}/.oxys-bore-patched"

  if [[ "${OXYS_APPLY_BORE}" != "1" ]]; then
    log "Skipping BORE patch"
    return 0
  fi
  if [[ -f "${patch_marker}" ]]; then
    log "BORE patch already applied"
    return 0
  fi
  if [[ -f "${KERNEL_BORE_PATCH_FILE}" ]]; then
    log "Applying BORE patch from ${KERNEL_BORE_PATCH_FILE}"
    patch -d "${source_dir}" -p1 < "${KERNEL_BORE_PATCH_FILE}" 2>&1 | tee -a "${CONTAINER_LOG}"
    touch "${patch_marker}"
    return 0
  fi
  if [[ -z "${OXYS_BORE_PATCH_URL}" ]]; then
    log "Provide podman/kernel/bore.patch or set OXYS_BORE_PATCH_URL for BORE-enabled kernel builds"
    exit 1
  fi

  log "Applying BORE patch from ${OXYS_BORE_PATCH_URL}"
  curl -fsSL "${OXYS_BORE_PATCH_URL}" | patch -d "${source_dir}" -p1 2>&1 | tee -a "${CONTAINER_LOG}"
  touch "${patch_marker}"
}

merge_kernel_config() {
  local source_dir="$1"

  if [[ ! -f "${KERNEL_BASE_CONFIG_FILE}" ]]; then
    log "Missing base kernel config: ${KERNEL_BASE_CONFIG_FILE}"
    exit 1
  fi
  if [[ ! -f "${KERNEL_ARCH_FRAGMENT_FILE}" ]]; then
    log "Missing arch kernel fragment: ${KERNEL_ARCH_FRAGMENT_FILE}"
    exit 1
  fi

  cat "${KERNEL_BASE_CONFIG_FILE}" "${KERNEL_ARCH_FRAGMENT_FILE}" > "${source_dir}/.config"
}

# olddefconfig silently drops any requested symbol whose dependencies aren't
# met (or that no longer exists after a kernel bump). For most options that's
# tolerable; for the live-ISO/boot-media set it produces a kernel that only
# fails hours later, deep inside catalyst stage2's dracut run (e.g. "Module
# 'dmsquash-live' depends on module 'overlayfs', which can't be installed")
# or -- worse -- at first boot. Fail the kernel build immediately instead.
assert_boot_critical_kernel_config() {
  local config="/usr/src/linux/.config"
  local missing=() opt
  for opt in OVERLAY_FS SQUASHFS BLK_DEV_LOOP ISO9660_FS BLK_DEV_DM \
             SCSI BLK_DEV_SD BLK_DEV_SR SATA_AHCI BLK_DEV_NVME USB_STORAGE \
             VFAT_FS EFI EFI_STUB INPUT_EVDEV VIRTIO_INPUT \
             VIRTIO_NET E1000E IGB IGC R8169 TIGON3 BNX2 ALX \
             USB_USBNET USB_NET_CDCETHER USB_RTL8152 \
             SERIAL_8250 SERIAL_8250_CONSOLE NET_9P NET_9P_VIRTIO 9P_FS; do
    grep -qE "^CONFIG_${opt}=[ym]$" "${config}" || missing+=("CONFIG_${opt}")
  done
  if (( ${#missing[@]} > 0 )); then
    log "Required boot/live-hardware kernel options missing after olddefconfig: ${missing[*]}"
    log "Check kernel/base.config (and its Kconfig dependencies) against this kernel version."
    exit 1
  fi
}

prepare_kernel_tree() {
  local source_dir
  source_dir="$(find /usr/src -maxdepth 1 -mindepth 1 -type d -name 'linux-*' | sort -V | tail -n 1)"
  if [[ -z "${source_dir}" ]]; then
    log "No kernel source directory found after gentoo-sources install"
    exit 1
  fi

  ln -sfn "${source_dir}" /usr/src/linux
  # gentoo-sources sets EXTRAVERSION=-gentoo in its top-level Makefile. Keep
  # the patched source package, but expose the built kernel as <version>-oxys.
  sed -i -E 's/^(EXTRAVERSION[[:space:]]*=).*/\1/' "${source_dir}/Makefile"
  if grep -qE '^EXTRAVERSION[[:space:]]*=[[:space:]]*.+$' "${source_dir}/Makefile"; then
    log "Failed to clear the gentoo-sources EXTRAVERSION"
    exit 1
  fi
  apply_bore_patch "${source_dir}"
  merge_kernel_config "${source_dir}"
  make -C /usr/src/linux olddefconfig prepare modules_prepare 2>&1 | tee -a "${CONTAINER_LOG}"
  assert_boot_critical_kernel_config
}

build_kernel_artifacts() {
  local kernel_release archive_name archive_path

  log "Building kernel for ${ARCH_NAME}"
  rm -rf "${KERNEL_STAGE_ROOT}"
  mkdir -p "${KERNEL_STAGE_ROOT}/boot"

  make -C /usr/src/linux -j"$(build_jobs)" bzImage modules 2>&1 | tee -a "${CONTAINER_LOG}"
  kernel_release="$(make -s -C /usr/src/linux kernelrelease)"
  make -C /usr/src/linux INSTALL_MOD_PATH="${KERNEL_STAGE_ROOT}" modules_install 2>&1 | tee -a "${CONTAINER_LOG}"
  printf '%s\n' "${kernel_release}" > "${KERNEL_RELEASE_FILE}"
  mkdir -p "/lib/modules/${kernel_release}"
  cp -a "${KERNEL_STAGE_ROOT}/lib/modules/${kernel_release}/." "/lib/modules/${kernel_release}/"

  install -m 0644 /usr/src/linux/arch/x86/boot/bzImage "${KERNEL_STAGE_ROOT}/boot/vmlinuz-${kernel_release}"
  install -m 0644 /usr/src/linux/System.map "${KERNEL_STAGE_ROOT}/boot/System.map-${kernel_release}"
  install -m 0644 /usr/src/linux/.config "${KERNEL_STAGE_ROOT}/boot/config-${kernel_release}"
  copy_kernel_configs_for_archive
  mkdir -p "${KERNEL_STAGE_ROOT}/usr/src/oxysos"
  cp "${KERNEL_CONFIG_STAGE_DIR}/base.config" "${KERNEL_STAGE_ROOT}/usr/src/oxysos/base.config"
  cp "${KERNEL_CONFIG_STAGE_DIR}/${ARCH_NAME}.fragment" "${KERNEL_STAGE_ROOT}/usr/src/oxysos/${ARCH_NAME}.fragment"
  cp "${KERNEL_CONFIG_STAGE_DIR}/merged.config" "${KERNEL_STAGE_ROOT}/usr/src/oxysos/kernel.config"
  {
    printf 'build_id=%s\n' "${BUILD_ID}"
    printf 'arch=%s\n' "${ARCH_NAME}"
    printf 'kernel_release=%s\n' "${kernel_release}"
    printf 'kernel_source=sys-kernel/gentoo-sources\n'
    printf 'zfs_module_source=sys-fs/zfs-kmod\n'
  } > "${KERNEL_STAGE_ROOT}/usr/src/oxysos/build-metadata.env"

  archive_name="kernel-${ARCH_NAME}-${kernel_release}-${BUILD_ID}.tar.gz"
  archive_path="${OUTPUT_ROOT}/${ARCH_NAME}/${archive_name}"
  tar -C "${KERNEL_STAGE_ROOT}" -czf "${archive_path}" .
  write_archive_metadata "${archive_path}" "sys-kernel/gentoo-sources" "${kernel_release}" "${kernel_release}"
  log "Created ${archive_name}"
}

validate_zfs_kmod_build() {
  local expected_kernel
  if [[ ! -f "${KERNEL_RELEASE_FILE}" ]]; then
    log "Missing ${KERNEL_RELEASE_FILE}; cannot validate zfs-kmod kernel pairing"
    exit 1
  fi
  expected_kernel="$(<"${KERNEL_RELEASE_FILE}")"

  local module_dir="/lib/modules/${expected_kernel}"
  if [[ ! -d "${module_dir}" ]]; then
    log "zfs-kmod validation failed: missing ${module_dir}"
    exit 1
  fi

  local zfs_module
  zfs_module="$(find "${module_dir}" -type f \( -name 'zfs.ko' -o -name 'zfs.ko.*' \) | sort | head -n 1)"
  if [[ -z "${zfs_module}" ]]; then
    log "zfs-kmod validation failed: no zfs.ko under ${module_dir}"
    exit 1
  fi

  if command -v modinfo >/dev/null 2>&1; then
    modinfo "${zfs_module}" 2>&1 | tee -a "${CONTAINER_LOG}"
    if ! modinfo "${zfs_module}" | awk -v kernel="${expected_kernel}" '$1 == "vermagic:" { found = index($0, kernel) } END { exit found ? 0 : 1 }'; then
      log "zfs-kmod validation failed: ${zfs_module} vermagic does not match ${expected_kernel}"
      exit 1
    fi
  fi

  depmod -b / "${expected_kernel}" 2>&1 | tee -a "${CONTAINER_LOG}"
  if command -v modprobe >/dev/null 2>&1; then
    modprobe -S "${expected_kernel}" --show-depends zfs 2>&1 | tee -a "${CONTAINER_LOG}"
    if [[ "$(uname -r)" == "${expected_kernel}" ]]; then
      log "Loading zfs module into running kernel ${expected_kernel}"
      modprobe zfs 2>&1 | tee -a "${CONTAINER_LOG}"
      lsmod | awk '$1 == "zfs" { found = 1 } END { exit found ? 0 : 1 }'
    else
      log "Skipping live zfs module load: running kernel $(uname -r) differs from built kernel ${expected_kernel}"
    fi
  fi

  log "Validated zfs-kmod for ${expected_kernel}: ${zfs_module}"
}

ensure_linux_headers() {
  local log_file="${EMERGE_LOG_DIR}/linux-headers.log"
  log "Installing sys-kernel/linux-headers"
  emerge --autounmask=y --autounmask-write=y --verbose sys-kernel/linux-headers 2>&1 | tee "${log_file}" || true
  etc-update --automode -5 2>&1 | tee -a "${log_file}"
  emerge --verbose sys-kernel/linux-headers 2>&1 | tee -a "${log_file}"
}

ensure_elfutils() {
  local log_file="${EMERGE_LOG_DIR}/elfutils.log"
  log "Installing dev-libs/elfutils"
  emerge --autounmask=y --autounmask-write=y --verbose dev-libs/elfutils 2>&1 | tee "${log_file}" || true
  etc-update --automode -5 2>&1 | tee -a "${log_file}"
  emerge --verbose dev-libs/elfutils 2>&1 | tee -a "${log_file}"
}

run_emerge() {
  local atom="$1"
  local start end elapsed suffix log_file
  suffix="${ARCH_NAME}"
  if [[ "${BUILD_PROFILE}" == "kernel" ]]; then
    suffix="${ARCH_NAME}-${BUILD_ID}"
  fi
  if [[ "${BUILD_PROFILE}" == "pgo" ]]; then
    suffix="${ARCH_NAME}-pgo"
  fi

  log_file="${EMERGE_LOG_DIR}/$(package_slug "${atom}").log"
  start="$(date +%s)"
  log "Starting ${atom}"

  if [[ "$(package_key "${atom}")" == "sys-kernel/gentoo-sources" ]]; then
    remove_kernel_mask
  fi

  emerge_package "${atom}" "${log_file}"

  end="$(date +%s)"
  elapsed="$((end - start))"
  printf '%s\t%s\t%s\n' "${ARCH_NAME}" "${atom}" "${elapsed}" >> "${TIMES_LOG}"

  if [[ "$(package_key "${atom}")" == "sys-kernel/gentoo-sources" ]]; then
    prepare_kernel_tree
    if [[ "${BUILD_PROFILE}" == "kernel" ]]; then
      build_kernel_artifacts
    fi
    restore_kernel_mask
  fi

  if [[ "$(package_key "${atom}")" != "sys-kernel/gentoo-sources" || "${BUILD_PROFILE}" != "kernel" ]]; then
    archive_from_vdb "${atom}" "${suffix}" "${start}"
  fi
  if [[ "$(package_key "${atom}")" == "sys-fs/zfs-kmod" && "${BUILD_PROFILE}" == "kernel" ]]; then
    validate_zfs_kmod_build
  fi
}

emerge_package() {
  local atom="$1"
  local log_file="$2"

  if [[ "${BUILD_PROFILE}" == "pgo" && "${atom}" == "${FIREFOX_ATOM}" ]]; then
    FEATURES="${COMMON_FEATURES} pgo" emerge --autounmask=y --autounmask-write=y --verbose "${atom}" 2>&1 | tee "${log_file}" || true
  else
    emerge --autounmask=y --autounmask-write=y --verbose "${atom}" 2>&1 | tee "${log_file}" || true
  fi

  etc-update --automode -5 2>&1 | tee -a "${log_file}"

  if [[ "${BUILD_PROFILE}" == "pgo" && "${atom}" == "${FIREFOX_ATOM}" ]]; then
    FEATURES="${COMMON_FEATURES} pgo" emerge --verbose "${atom}" 2>&1 | tee -a "${log_file}"
  else
    emerge --verbose "${atom}" 2>&1 | tee -a "${log_file}"
  fi
}

main() {
  ensure_dirs
  ensure_profile
  write_portage_config

  log "Using builder-provided Portage"
  ensure_elfutils
  ensure_linux_headers

  while IFS= read -r atom; do
    [[ -z "${atom}" ]] && continue
    run_emerge "${atom}"
  done < <(build_package_queue)

  log "Build profile ${BUILD_PROFILE} for ${ARCH_NAME} completed"
}

main "$@"
