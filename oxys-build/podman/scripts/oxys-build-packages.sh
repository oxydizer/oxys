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
KERNEL_HARDWARE_FRAGMENT_FILE="${SOURCE_ROOT}/podman/kernel/hardware.fragment"
if [[ ! -f "${KERNEL_HARDWARE_FRAGMENT_FILE}" ]]; then
  KERNEL_HARDWARE_FRAGMENT_FILE="${WORK_ROOT}/kernel/hardware.fragment"
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
KERNEL_ARTIFACTS_FILE="${OUTPUT_ROOT}/${ARCH_NAME}/kernel-artifacts.env"

# Session flags MUST be "-systemd elogind": every OxysOS consumer (the
# catalyst ISO build and oxys-generated target make.confs) runs OpenRC with
# elogind and --binpkg-respect-use=y, so a systemd-flavored binpkg of any
# package with systemd/elogind in IUSE (pipewire, wireplumber, dbus, polkit,
# NetworkManager, ...) is rejected and rebuilt from source on every install.
# Keep the rest of this list in sync with installcd-stage1.spec livecd/use,
# portage_confdir/make.conf, and BINHOST_BASELINE_COMMON_USE_FLAGS in
# oxys/src/use_resolver/generate.rs.
readonly BINHOST_BASELINE_USE_FLAGS="X wayland dbus -systemd elogind policykit alsa pipewire pulseaudio vulkan opengl gtk jpeg png webp svg fontconfig harfbuzz udev ssl threads unicode"
readonly GLOBAL_USE_FLAGS="${BINHOST_BASELINE_USE_FLAGS} -debug zfs -california -colorado"
readonly COMMON_FEATURES="parallel-fetch candy"
readonly ACCEPT_KEYWORDS_VALUE="amd64"
readonly FIREFOX_ATOM="www-client/firefox"
readonly PROFILE_TARGET="default/linux/amd64/23.0"
readonly BINHOST_PROFILE_TARGET="default/linux/amd64/23.0/no-multilib"
readonly BINHOST_ATOM="app-admin/oxys::oxys"
readonly BINHOST_FINAL_DIRNAME="x86-64-v3"
BINHOST_WORK_ROOT="${OXYS_BINHOST_WORK_ROOT:-${OUTPUT_ROOT}/.binhost-work/${ARCH_NAME}}"
BINHOST_UNSIGNED_REPO="${BINHOST_WORK_ROOT}/unsigned"
BINHOST_SIGNED_REPO="${BINHOST_WORK_ROOT}/signed"
BINHOST_PUBLISH_ROOT="${OXYS_BINHOST_PUBLISH_ROOT:-${OUTPUT_ROOT}/binpackages}"
BINHOST_SIGNING_SOURCE="${OXYS_BINPKG_SIGNING_GPG_HOME:-}"
BINHOST_SIGNING_KEY="${OXYS_BINPKG_SIGNING_KEY:-}"
BINHOST_SIGNING_ROOT=""
BINHOST_SIGNING_HOME=""
BINHOST_VERIFY_HOME=""
BINHOST_PUBLISH_STAGE=""
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

# Graphics is an image-build input, not merely an installer-time wish. These
# defaults preserve the capability set Oxys shipped before the policy became
# configurable; callers can narrow or extend it for a particular image.
VIDEO_CARDS_POLICY="${OXYS_VIDEO_CARDS:-intel radeon radeonsi amdgpu virgl}"
case "${ARCH_NAME}" in
  alderlake) DEFAULT_DRM_DRIVERS="intel virtio_gpu" ;;
  v3) DEFAULT_DRM_DRIVERS="intel amdgpu virtio_gpu" ;;
  znver3|znver4|znver5) DEFAULT_DRM_DRIVERS="amdgpu virtio_gpu" ;;
  *) DEFAULT_DRM_DRIVERS="virtio_gpu" ;;
esac
DRM_DRIVERS_POLICY="${OXYS_DRM_DRIVERS:-${DEFAULT_DRM_DRIVERS}}"

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
KERNEL_ARCHIVE_NAME=""
ZFS_KMOD_ARCHIVE_NAME=""
ZFS_USERLAND_ARCHIVE_NAME=""

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
  "media-video/wireplumber"
  "gui-apps/waybar"
)

readonly -a V3_GENERIC_EXTRAS=(
  # gui-apps/fuzzel and app-misc/cliphist are GURU-only (not in ::gentoo),
  # same as gui-wm/niri and x11-base/xwayland-satellite above -- this
  # container has no GURU repos.conf entry. Left off the binhost for the
  # same reason: no install-time cost, since the ISO build clones GURU and
  # builds these itself, and the target then rsyncs + skips the rebuild.
  "gui-apps/mako"
  "gui-apps/foot"
  "app-shells/fish"
  "gui-apps/swaylock"
  "gui-apps/swayidle"
  "gui-apps/swaybg"
  # Live/target networking: kept in lockstep with installcd-stage1.spec's
  # "live networking" and "Bluetooth userspace" package blocks, and with
  # package.use/live-networking's per-package flags below, so the binhost
  # binpkg matches what --binpkg-respect-use=y expects and isn't rebuilt
  # from source at ISO/install time.
  "sys-apps/dbus"
  "net-misc/networkmanager"
  "net-wireless/wpa_supplicant"
  "net-wireless/iw"
  "net-misc/dhcpcd"
  "net-wireless/bluez"
  "gui-apps/wl-clipboard"
  "sys-auth/polkit"
  "app-admin/sudo"
  # Manifest packages the target would otherwise source-build at install
  # time: iucode_tool (intel-microcode BDEPEND, tracked in @world) plus the
  # toolchain the generated make.conf demands (LDFLAGS=-fuse-ld=mold,
  # FEATURES=ccache).
  "sys-apps/iucode_tool"
  "sys-devel/mold"
  "dev-util/ccache"
)

log() {
  local ts
  ts="$(date -u +"%Y-%m-%dT%H:%M:%SZ")"
  printf '[%s] %s\n' "${ts}" "$*" | tee -a "${CONTAINER_LOG}"
}

normalize_graphics_policy() {
  local kind="$1" raw="$2"
  shift 2
  local -a allowed=("$@") values=()
  local value candidate known duplicate

  raw="${raw//,/ }"
  for value in ${raw}; do
    known=0
    for candidate in "${allowed[@]}"; do
      [[ "${value}" == "${candidate}" ]] && known=1
    done
    if (( known == 0 )); then
      log "Unsupported ${kind} value '${value}' (allowed: ${allowed[*]})"
      exit 1
    fi
    duplicate=0
    for candidate in "${values[@]}"; do
      [[ "${value}" == "${candidate}" ]] && duplicate=1
    done
    (( duplicate == 0 )) && values+=("${value}")
  done
  if (( ${#values[@]} == 0 )); then
    log "${kind} policy must contain at least one value"
    exit 1
  fi
  printf '%s\n' "${values[*]}"
}

resolve_graphics_policy() {
  VIDEO_CARDS_POLICY="$(normalize_graphics_policy VIDEO_CARDS "${VIDEO_CARDS_POLICY}" \
    intel amdgpu radeon radeonsi nouveau virgl vmware lavapipe)"
  DRM_DRIVERS_POLICY="$(normalize_graphics_policy DRM-driver "${DRM_DRIVERS_POLICY}" \
    intel amdgpu radeon nouveau virtio_gpu vmwgfx)"
  log "Graphics build policy: VIDEO_CARDS='${VIDEO_CARDS_POLICY}', DRM drivers='${DRM_DRIVERS_POLICY}'"
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
  elif [[ "${BUILD_PROFILE}" == "kernel" ]]; then
    # A kernel profile is one publish transaction. Give every run a fresh
    # internal id so partially replaced artifacts cannot form a valid pair.
    BUILD_ID="$(generated_build_id)"
    printf '%s\n' "${BUILD_ID}" > "${BUILD_ID_FILE}"
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
  if [[ "${BUILD_PROFILE}" == "kernel" ]]; then
    # Do not advertise the stable filenames while this run replaces them.
    rm -f "${KERNEL_ARTIFACTS_FILE}"
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
EMERGE_DEFAULT_OPTS="--ask=n --verbose --keep-going=y --with-bdeps=y --jobs=1 --load-average=$(build_jobs) --binpkg-respect-use=y"
PKGDIR="/var/cache/binpkgs"
PORTAGE_BINHOST_HEADER_URI=""
${binhost_line}
# Resolved image policy. Mesa's VDB USE metadata is verified after its merge.
VIDEO_CARDS="${VIDEO_CARDS_POLICY}"
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
media-video/pipewire sound-server pipewire-alsa
gui-wm/niri screencast
gui-shells/noctalia jemalloc
net-misc/networkmanager wifi tools
net-wireless/wpa_supplicant dbus
EOF_USE

  cat > /etc/portage/package.accept_keywords/oxys <<'EOF_KEYWORDS'
gui-wm/niri ~amd64
gui-shells/noctalia **
x11-base/xwayland-satellite ~amd64
gui-apps/waybar ~amd64
media-video/pipewire ~amd64
media-video/wireplumber ~amd64
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

ensure_binpkg_trust() {
  [[ "${BUILD_PROFILE}" == "generic" ]] || return 0

  if ! command -v getuto >/dev/null 2>&1; then
    log "Generic profile requires getuto to initialize Gentoo binpackage trust"
    exit 1
  fi

  log "Initializing Gentoo binpackage signing-key trust"
  getuto

  if [[ ! -s /etc/portage/gnupg/pubring.kbx || \
        ! -s /etc/portage/gnupg/trustdb.gpg ]]; then
    log "Gentoo binpackage trust initialization did not create a usable keyring"
    exit 1
  fi
}

# Reuses the ISO build's prefetched git-r3 bare-repo cache (see
# oxys-iso/scripts/prefetch-git-sources.sh and oxys-iso/git-sources.conf) so
# live-git ebuilds such as gui-shells/noctalia-9999 don't need a fresh GitHub
# clone mid-build. build.sh bind-mounts the sibling oxys-iso/ tree read-only
# at /oxys-iso when present; this is a best-effort optimization, not a hard
# requirement -- if the mount or cache is missing/stale, affected atoms just
# fall back to a normal network fetch during their own src_unpack.
stage_offline_git_sources() {
  local oxys_iso_mount="/oxys-iso"
  local sources_file="${oxys_iso_mount}/git-sources.conf"
  local prefetch_script="${oxys_iso_mount}/scripts/prefetch-git-sources.sh"
  local cache_ref="refs/heads/oxys-source"
  local distdir package_env_dir env_dir rows_raw staged=0

  if [[ ! -f "${sources_file}" || ! -x "${prefetch_script}" ]]; then
    log "No oxys-iso git-source cache mounted; live ebuilds will fetch over the network"
    return 0
  fi

  # This runs before the main package queue, so dev-vcs/git (itself one of
  # the queued atoms) isn't installed yet on the bare stage3 image -- bootstrap
  # it directly rather than waiting for its normal turn in the queue.
  if ! command -v git >/dev/null 2>&1; then
    log "Bootstrapping dev-vcs/git to stage offline Git sources"
    if ! emerge --oneshot --nodeps --quiet dev-vcs/git >/dev/null 2>&1; then
      log "Could not bootstrap git; live ebuilds will fetch over the network"
      return 0
    fi
  fi

  # The bind-mounted cache is owned by the host uid, not this container's;
  # without this, git's ownership safety check makes every operation on it
  # (even a plain `cat-file -e`) fail silently as "dubious ownership" rather
  # than a real missing-commit error. oxys-iso/build.sh sets this for the
  # same reason.
  git config --system --add safe.directory '*'

  if ! rows_raw="$("${prefetch_script}" --list "${sources_file}")"; then
    log "WARNING: could not parse ${sources_file}; live ebuilds will fetch over the network"
    return 0
  fi

  distdir="$(portageq envvar DISTDIR)"
  package_env_dir="/etc/portage/package.env"
  env_dir="/etc/portage/env"
  mkdir -p "${package_env_dir}" "${env_dir}"
  cat > "${env_dir}/prefetched-git-source.conf" <<'EOF_ENV'
EVCS_OFFLINE=1
EOF_ENV
  : > "${package_env_dir}/prefetched-git-sources"

  while IFS=$'\t' read -r package_atom store_name _source_uri _source_ref; do
    local cache_repo="${oxys_iso_mount}/.build/source-cache/git3-src/${store_name}"
    local git3_repo="${distdir}/git3-src/${store_name}"
    if ! git --git-dir="${cache_repo}" cat-file -e "${cache_ref}^{commit}" 2>/dev/null; then
      log "No usable prefetched source for ${package_atom} (${store_name}); it will fetch over the network"
      continue
    fi
    mkdir -p "$(dirname "${git3_repo}")"
    rm -rf "${git3_repo}"
    git clone --bare --no-hardlinks "${cache_repo}" "${git3_repo}" >/dev/null
    chown -R portage:portage "${git3_repo}"
    printf '%s %s\n' "${package_atom}" "prefetched-git-source.conf" >> "${package_env_dir}/prefetched-git-sources"
    log "Staged offline Git source for ${package_atom} at ${git3_repo}"
    ((staged += 1))
  done <<< "${rows_raw}"

  if (( staged > 0 )); then
    log "Staged ${staged} offline Git source(s) from the oxys-iso cache"
  fi
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
      # The v3 repo is the only binhost a v3 target ever queries, so serve the
      # session/desktop stack here too — otherwise pipewire, wireplumber,
      # noctalia, etc. have no binpkg at all on v3 and every ISO build and
      # target update compiles them from source. media-libs/mesa is served
      # here as well, built under the full VIDEO_CARDS_POLICY (virgl
      # included): reuse under a narrower/different policy is prevented by
      # --binpkg-respect-use=y on every consumer, which falls back to a
      # source build instead of accepting a mismatched binpkg, and
      # validate_mesa_build_policy checks the installed driver artifacts. One
      # exception:
      #   - gui-wm/niri, x11-base/xwayland-satellite: these live in the GURU
      #     overlay (see oxys-iso/overlay/etc/portage/repos.conf/guru.conf),
      #     which this container has no repos.conf entry for -- "emerge: there
      #     are no ebuilds to satisfy" if attempted. The ISO build already
      #     clones GURU and builds these from source itself; the target then
      #     gets them via rsync + --update --changed-use skips the rebuild,
      #     so there's no install-time cost to leaving them off this binhost.
      #     Wiring GURU into this container (clone + repos.conf + egencache +
      #     correct ~amd64 keywords) is possible later but untested here.
      #
      # NOTE: the Containerfiles FROM gentoo/stage3:amd64-openrc (not
      # amd64-systemd) specifically so this REAL merge (emerge_package does
      # not use --buildpkgonly) never has to reconcile the container's own
      # init packages against the elogind-flavored session stack it's
      # building. If the desktop USE baseline in BINHOST_BASELINE_USE_FLAGS
      # ever flips back to systemd, the base image must flip with it.
      local -a generic_native_excludes=(
        "gui-wm/niri"
        "x11-base/xwayland-satellite"
      )
      for atom in "${NATIVE_PACKAGES[@]}"; do
        if ! array_contains "${atom}" "${generic_native_excludes[@]}"; then
          printf '%s\n' "${atom}"
        fi
      done
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
  cp "${KERNEL_HARDWARE_FRAGMENT_FILE}" "${KERNEL_CONFIG_STAGE_DIR}/hardware.fragment"
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
  if [[ "${BUILD_PROFILE}" == "kernel" ]]; then
    case "$(package_key "${atom}")" in
      sys-fs/zfs-kmod)
        archive_name="oxys-zfs-kmod-${version}-${ARCH_NAME}.tar.gz"
        ZFS_KMOD_ARCHIVE_NAME="${archive_name}"
        ;;
      sys-fs/zfs)
        archive_name="oxys-zfs-${version}-${ARCH_NAME}.tar.gz"
        ZFS_USERLAND_ARCHIVE_NAME="${archive_name}"
        ;;
    esac
  fi
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
    printf 'video_cards=%s\n' "${VIDEO_CARDS_POLICY}"
    printf 'drm_drivers=%s\n' "${DRM_DRIVERS_POLICY}"
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
  if [[ ! -f "${KERNEL_HARDWARE_FRAGMENT_FILE}" ]]; then
    log "Missing hardware kernel fragment: ${KERNEL_HARDWARE_FRAGMENT_FILE}"
    exit 1
  fi

  # hardware.fragment sits between base.config and the arch fragment: it's
  # the desktop/laptop baseline every image needs regardless of microarch,
  # but an arch fragment (applied last) can still override an individual
  # symbol -- e.g. alderlake.fragment forcing CONFIG_DRM_AMDGPU=n.
  cat "${KERNEL_BASE_CONFIG_FILE}" "${KERNEL_HARDWARE_FRAGMENT_FILE}" "${KERNEL_ARCH_FRAGMENT_FILE}" > "${source_dir}/.config"

  # The generated fragment is last so explicit image policy wins over the
  # architecture baseline. olddefconfig resolves dependencies; the assertion
  # below catches any requested symbol that Kconfig could not retain.
  local driver option
  for driver in intel amdgpu radeon nouveau virtio_gpu vmwgfx; do
    case "${driver}" in
      intel) option=DRM_I915 ;;
      amdgpu) option=DRM_AMDGPU ;;
      radeon) option=DRM_RADEON ;;
      nouveau) option=DRM_NOUVEAU ;;
      virtio_gpu) option=DRM_VIRTIO_GPU ;;
      vmwgfx) option=DRM_VMWGFX ;;
    esac
    if [[ " ${DRM_DRIVERS_POLICY} " == *" ${driver} "* ]]; then
      printf 'CONFIG_%s=y\n' "${option}" >> "${source_dir}/.config"
    else
      printf 'CONFIG_%s=n\n' "${option}" >> "${source_dir}/.config"
    fi
  done
  if [[ " ${DRM_DRIVERS_POLICY} " == *' virtio_gpu '* ]]; then
    cat >> "${source_dir}/.config" <<'EOF_GRAPHICS'
CONFIG_DRM=y
CONFIG_DRM_KMS_HELPER=y
CONFIG_DRM_GEM_SHMEM_HELPER=y
CONFIG_VIRTIO=y
CONFIG_VIRTIO_PCI=y
EOF_GRAPHICS
  fi
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
  for opt in SMP \
             OVERLAY_FS SQUASHFS BLK_DEV_LOOP ISO9660_FS BLK_DEV_DM \
             SWAP ZRAM ZRAM_BACKEND_ZSTD \
             SCSI BLK_DEV_SD BLK_DEV_SR SATA_AHCI BLK_DEV_NVME USB_STORAGE \
             VFAT_FS EFI EFI_STUB INPUT_EVDEV VIRTIO_INPUT \
             PACKET \
             VIRTIO_NET E1000E IGB IGC R8169 TIGON3 BNX2 ALX \
             USB_USBNET USB_NET_CDCETHER USB_RTL8152 \
             SERIAL_8250 SERIAL_8250_CONSOLE NET_9P NET_9P_VIRTIO 9P_FS; do
    grep -qE "^CONFIG_${opt}=[ym]$" "${config}" || missing+=("CONFIG_${opt}")
  done
  # Desktop/laptop hardware baseline (kernel/hardware.fragment) -- a kernel
  # without these boots fine in QEMU but fails the first hour on real
  # hardware (no firewall, no webcam, no Bluetooth, no touchpad, dead DMIC
  # audio, no LUKS). This is exactly the class of bug olddefconfig hides
  # silently (wrong/renamed Kconfig symbol -> dropped, no error) that this
  # assert exists to catch.
  for opt in NETFILTER NF_CONNTRACK NF_TABLES NF_TABLES_INET NFT_CT NFT_REJECT NF_NAT \
             FUSE_FS EXFAT_FS NTFS3_FS \
             BT BT_HCIBTUSB USB_VIDEO_CLASS MEDIA_SUPPORT \
             I2C_HID_ACPI HID_MULTITOUCH RTC_CLASS RTC_DRV_CMOS \
             DM_CRYPT CRYPTO_XTS MEMCG SND_SOC SND_SOC_SOF_TOPLEVEL; do
    grep -qE "^CONFIG_${opt}=[ym]$" "${config}" || missing+=("CONFIG_${opt}")
  done
  if (( ${#missing[@]} > 0 )); then
    log "Required boot/live-hardware kernel options missing after olddefconfig: ${missing[*]}"
    log "Check kernel/base.config, kernel/hardware.fragment (and their Kconfig dependencies) against this kernel version."
    exit 1
  fi
  # At least one cpufreq driver must have survived -- a wrong/renamed Kconfig
  # symbol here (this exact bug shipped: alderlake.fragment requested the
  # nonexistent CONFIG_INTEL_PSTATE instead of CONFIG_X86_INTEL_PSTATE) is
  # silently dropped by olddefconfig rather than erroring, leaving a kernel
  # with no frequency scaling at all.
  grep -qE '^CONFIG_(X86_INTEL_PSTATE|X86_AMD_PSTATE|X86_ACPI_CPUFREQ)=[ym]$' "${config}" \
    || { log "No cpufreq driver survived in kernel config (checked X86_INTEL_PSTATE/X86_AMD_PSTATE/X86_ACPI_CPUFREQ)."; exit 1; }
}

assert_graphics_kernel_config() {
  local config="/usr/src/linux/.config"
  local driver option
  local -a missing=()
  for driver in ${DRM_DRIVERS_POLICY}; do
    case "${driver}" in
      intel) option=DRM_I915 ;;
      amdgpu) option=DRM_AMDGPU ;;
      radeon) option=DRM_RADEON ;;
      nouveau) option=DRM_NOUVEAU ;;
      virtio_gpu) option=DRM_VIRTIO_GPU ;;
      vmwgfx) option=DRM_VMWGFX ;;
    esac
    grep -qE "^CONFIG_${option}=[ym]$" "${config}" || missing+=("CONFIG_${option}")
  done
  if [[ " ${DRM_DRIVERS_POLICY} " == *' virtio_gpu '* ]]; then
    for option in DRM DRM_KMS_HELPER DRM_GEM_SHMEM_HELPER VIRTIO VIRTIO_PCI; do
      grep -qE "^CONFIG_${option}=[ym]$" "${config}" || missing+=("CONFIG_${option}")
    done
  fi
  if (( ${#missing[@]} > 0 )); then
    log "Graphics policy was not satisfied after olddefconfig: ${missing[*]}"
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
  assert_graphics_kernel_config
}

build_kernel_artifacts() {
  local kernel_release kernel_version archive_name archive_path

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
  cp "${KERNEL_CONFIG_STAGE_DIR}/hardware.fragment" "${KERNEL_STAGE_ROOT}/usr/src/oxysos/hardware.fragment"
  cp "${KERNEL_CONFIG_STAGE_DIR}/${ARCH_NAME}.fragment" "${KERNEL_STAGE_ROOT}/usr/src/oxysos/${ARCH_NAME}.fragment"
  cp "${KERNEL_CONFIG_STAGE_DIR}/merged.config" "${KERNEL_STAGE_ROOT}/usr/src/oxysos/kernel.config"
  {
    printf 'build_id=%s\n' "${BUILD_ID}"
    printf 'arch=%s\n' "${ARCH_NAME}"
    printf 'kernel_release=%s\n' "${kernel_release}"
    printf 'kernel_source=sys-kernel/gentoo-sources\n'
    printf 'zfs_module_source=sys-fs/zfs-kmod\n'
    printf 'video_cards=%s\n' "${VIDEO_CARDS_POLICY}"
    printf 'drm_drivers=%s\n' "${DRM_DRIVERS_POLICY}"
  } > "${KERNEL_STAGE_ROOT}/usr/src/oxysos/build-metadata.env"

  kernel_version="${kernel_release%-oxys}"
  archive_name="oxys-kernel-${kernel_version}-${ARCH_NAME}.tar.gz"
  KERNEL_ARCHIVE_NAME="${archive_name}"
  archive_path="${OUTPUT_ROOT}/${ARCH_NAME}/${archive_name}"
  tar -C "${KERNEL_STAGE_ROOT}" -czf "${archive_path}" .
  write_archive_metadata "${archive_path}" "sys-kernel/gentoo-sources" "${kernel_release}" "${kernel_release}"
  log "Created ${archive_name}"
}

publish_kernel_artifacts() {
  local manifest_tmp

  [[ -n "${KERNEL_ARCHIVE_NAME}" ]] || { log "Kernel artifact was not produced"; exit 1; }
  [[ -n "${ZFS_KMOD_ARCHIVE_NAME}" ]] || { log "ZFS kmod artifact was not produced"; exit 1; }
  [[ -n "${ZFS_USERLAND_ARCHIVE_NAME}" ]] || { log "ZFS userland artifact was not produced"; exit 1; }

  manifest_tmp="${KERNEL_ARTIFACTS_FILE}.tmp"
  {
    printf 'build_id=%s\n' "${BUILD_ID}"
    printf 'arch=%s\n' "${ARCH_NAME}"
    printf 'kernel_archive=%s\n' "${KERNEL_ARCHIVE_NAME}"
    printf 'zfs_kmod_archive=%s\n' "${ZFS_KMOD_ARCHIVE_NAME}"
    printf 'zfs_userland_archive=%s\n' "${ZFS_USERLAND_ARCHIVE_NAME}"
  } > "${manifest_tmp}"
  mv "${manifest_tmp}" "${KERNEL_ARTIFACTS_FILE}"
  log "Published kernel artifact set in ${KERNEL_ARTIFACTS_FILE}"
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

validate_mesa_build_policy() {
  local vdb card flag artifact
  local -a missing=()
  vdb="$(find /var/db/pkg/media-libs -mindepth 1 -maxdepth 1 -type d -name 'mesa-*' -print 2>/dev/null | sort -V | tail -n 1)"
  if [[ -z "${vdb}" ]]; then
    log "Mesa policy validation failed: installed Mesa VDB metadata is missing"
    exit 1
  fi

  # Installed driver artifacts are the capability ground truth. Some policy
  # names deliberately do not correspond to a Mesa USE flag: amdgpu is a
  # libdrm/kernel selector whose Mesa userspace driver is radeonsi.
  for card in ${VIDEO_CARDS_POLICY}; do
    flag="video_cards_${card}"
    case "${card}" in
      intel) artifact="$(find /usr/lib64 /usr/lib \( -type f -o -type l \) \( -name iris_dri.so -o -name crocus_dri.so \) -print -quit 2>/dev/null || true)" ;;
      amdgpu|radeonsi) artifact="$(find /usr/lib64 /usr/lib \( -type f -o -type l \) -name radeonsi_dri.so -print -quit 2>/dev/null || true)" ;;
      radeon) artifact="$(find /usr/lib64 /usr/lib \( -type f -o -type l \) \( -name r600_dri.so -o -name radeon_dri.so \) -print -quit 2>/dev/null || true)" ;;
      nouveau) artifact="$(find /usr/lib64 /usr/lib \( -type f -o -type l \) -name nouveau_dri.so -print -quit 2>/dev/null || true)" ;;
      virgl) artifact="$(find /usr/lib64 /usr/lib \( -type f -o -type l \) -name virtio_gpu_dri.so -print -quit 2>/dev/null || true)" ;;
      vmware) artifact="$(find /usr/lib64 /usr/lib \( -type f -o -type l \) -name vmwgfx_dri.so -print -quit 2>/dev/null || true)" ;;
      lavapipe) artifact="$(find /usr/lib64 /usr/lib \( -type f -o -type l \) \( -name libvulkan_lvp.so -o -name libvulkan_lvp.so.1 \) -print -quit 2>/dev/null || true)" ;;
      *) artifact="" ;;
    esac
    [[ -n "${artifact}" ]] || missing+=("${flag}")
  done
  if (( ${#missing[@]} > 0 )); then
    log "Installed Mesa does not satisfy graphics build policy: missing ${missing[*]}"
    log "Reject the cached binpackage and rebuild media-libs/mesa from source."
    exit 1
  fi
  log "Validated installed Mesa driver artifacts for VIDEO_CARDS='${VIDEO_CARDS_POLICY}'"
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

  if [[ "$(package_key "${atom}")" == "media-libs/mesa" ]]; then
    validate_mesa_build_policy
  fi

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

  if [[ "$(package_key "${atom}")" == "sys-fs/zfs-kmod" && "${BUILD_PROFILE}" == "kernel" ]]; then
    validate_zfs_kmod_build
  fi
  if [[ "$(package_key "${atom}")" != "sys-kernel/gentoo-sources" || "${BUILD_PROFILE}" != "kernel" ]]; then
    archive_from_vdb "${atom}" "${suffix}" "${start}"
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

binhost_fail() {
  log "Binhost error: $*"
  exit 1
}

ensure_binhost_log_dirs() {
  mkdir -p "$(dirname "${CONTAINER_LOG}")" "${EMERGE_LOG_DIR}"
  touch "${CONTAINER_LOG}" "${TIMES_LOG}"
}

require_binhost_commands() {
  local command
  for command in "$@"; do
    command -v "${command}" >/dev/null 2>&1 || \
      binhost_fail "required command is unavailable: ${command}"
  done
}

validate_binhost_target() {
  [[ "${ARCH_NAME}" == "v3" ]] || \
    binhost_fail "the binhost target requires OXYS_ARCH=v3, got ${ARCH_NAME}"
  [[ "${MARCH}" == "x86-64-v3" ]] || \
    binhost_fail "the binhost target requires OXYS_MARCH=x86-64-v3, got ${MARCH}"
}

validate_binhost_overlay() {
  local package_dir="${OVERLAY_ROOT}/app-admin/oxys"
  local manifest_payload payload_version
  local -a payloads=() ebuilds=()

  [[ -s "${package_dir}/Manifest" ]] || \
    binhost_fail "missing staged app-admin/oxys Manifest: ${package_dir}/Manifest"
  manifest_payload="$(awk '$1 == "AUX" && $2 ~ /^oxys-[0-9]/ { print $2 }' \
    "${package_dir}/Manifest")"
  if [[ -z "${manifest_payload}" || "${manifest_payload}" == *$'\n'* ]]; then
    binhost_fail "app-admin/oxys Manifest must contain exactly one versioned oxys AUX payload"
  fi
  [[ -x "${package_dir}/files/${manifest_payload}" ]] || \
    binhost_fail "Manifest payload is missing or not executable: files/${manifest_payload}"
  payload_version="${manifest_payload#oxys-}"
  [[ -f "${package_dir}/oxys-${payload_version}.ebuild" ]] || \
    binhost_fail "files/${manifest_payload} has no matching oxys-${payload_version}.ebuild"
  mapfile -d '' payloads < <(
    find "${package_dir}/files" -maxdepth 1 -type f -name 'oxys-*' -print0
  )
  (( ${#payloads[@]} == 1 )) || \
    binhost_fail "app-admin/oxys must contain exactly one versioned files/oxys-* payload"
  [[ "${payloads[0]}" == "${package_dir}/files/${manifest_payload}" ]] || \
    binhost_fail "the sole staged payload does not match the Manifest"
  mapfile -d '' ebuilds < <(
    find "${package_dir}" -maxdepth 1 -type f -name 'oxys-*.ebuild' -print0
  )
  (( ${#ebuilds[@]} == 1 )) || \
    binhost_fail "app-admin/oxys must contain exactly one versioned ebuild"
  [[ "${ebuilds[0]}" == "${package_dir}/oxys-${payload_version}.ebuild" ]] || \
    binhost_fail "the sole ebuild does not match the staged payload version"
}

select_binhost_profile() {
  local profile_link="/etc/portage/make.profile"
  local profile_target="/var/db/repos/gentoo/profiles/${BINHOST_PROFILE_TARGET}"

  [[ -d "${profile_target}" ]] || \
    binhost_fail "Gentoo no-multilib profile is unavailable: ${profile_target}"
  rm -f "${profile_link}"
  ln -s "${profile_target}" "${profile_link}"
  [[ "$(readlink -f "${profile_link}")" == "${profile_target}" ]] || \
    binhost_fail "failed to select ${BINHOST_PROFILE_TARGET}"
}

write_binhost_build_config() {
  rm -rf "${OXYS_OVERLAY_REPO}"
  cp -a "${OVERLAY_ROOT}" "${OXYS_OVERLAY_REPO}"
  mkdir -p /etc/portage/repos.conf
  cat > "${OXYS_REPOS_CONF_FILE}" <<EOF_REPO
[oxys]
location = ${OXYS_OVERLAY_REPO}
masters = gentoo
auto-sync = no
EOF_REPO

  cat > /etc/portage/make.conf <<EOF_MAKE
COMMON_FLAGS="-O2 -pipe -march=x86-64-v3"
CFLAGS="\${COMMON_FLAGS}"
CXXFLAGS="\${COMMON_FLAGS}"
FCFLAGS="\${COMMON_FLAGS}"
FFLAGS="\${COMMON_FLAGS}"
MAKEOPTS="${COMMON_MAKEOPTS}"
FEATURES="${COMMON_FEATURES} buildpkg -binpkg-signing -binpkg-request-signature"
BINPKG_FORMAT="gpkg"
PKGDIR="${BINHOST_UNSIGNED_REPO}"
ACCEPT_KEYWORDS="${ACCEPT_KEYWORDS_VALUE}"
EMERGE_DEFAULT_OPTS="--ask=n --verbose --jobs=1 --load-average=$(build_jobs)"
PORTAGE_BINHOST_HEADER_URI=""
EOF_MAKE
}

assert_binhost_build_config() {
  local abi_x86 chost features format
  abi_x86="$(portageq envvar ABI_X86)"
  chost="$(portageq envvar CHOST)"
  features="$(portageq envvar FEATURES)"
  format="$(portageq envvar BINPKG_FORMAT)"

  [[ "${abi_x86}" == "64" ]] || \
    binhost_fail "no-multilib profile resolved ABI_X86='${abi_x86}', expected '64'"
  [[ "${chost}" == "x86_64-pc-linux-gnu" ]] || \
    binhost_fail "unexpected binhost CHOST: ${chost}"
  [[ "${format}" == "gpkg" ]] || \
    binhost_fail "Portage resolved BINPKG_FORMAT='${format}', expected gpkg"
  [[ " ${features} " == *" buildpkg "* ]] || \
    binhost_fail "Portage FEATURES does not enable buildpkg"
  [[ " ${features} " != *" binpkg-signing "* ]] || \
    binhost_fail "private-key signing must not be enabled in the emerge builder"
  [[ " ${features} " != *" binpkg-request-signature "* ]] || \
    binhost_fail "unsigned build staging unexpectedly requires signatures"
}

verify_gpkg() {
  local package_path="$1"
  local expected_state="$2"

  python3 - "${package_path}" "${expected_state}" <<'PY_VERIFY'
import sys

import portage

portage._internal_caller = True
from portage.gpkg import gpkg

package_path, expected_state = sys.argv[1:]
expect_signed = expected_state == "signed"
package = gpkg(
    settings=portage.settings,
    gpkg_file=package_path,
    verify_signature=expect_signed,
)
package._verify_binpkg()
if bool(package.signature_exist) != expect_signed:
    state = "signed" if package.signature_exist else "unsigned"
    raise SystemExit(f"{package_path} is {state}, expected {expected_state}")
PY_VERIFY
}

validate_binhost_repo() {
  local repo_root="$1"
  local expected_state="$2"
  local package_count indexed_path relative_path repositories
  local indexed_md5 indexed_sha1 indexed_size actual_md5 actual_sha1 actual_size
  local -a packages=() binpkg_files=() cpvs=()

  [[ -s "${repo_root}/Packages" ]] || \
    binhost_fail "native Packages index is missing or empty under ${repo_root}"

  mapfile -d '' packages < <(
    find "${repo_root}" -type f -name '*.gpkg.tar' -print0
  )
  mapfile -d '' binpkg_files < <(
    find "${repo_root}" -type f \
      \( -name '*.gpkg.tar' -o -name '*.tbz2' -o -name '*.xpak' \) -print0
  )
  (( ${#packages[@]} == 1 )) || \
    binhost_fail "expected exactly one GPKG under ${repo_root}, found ${#packages[@]}"
  (( ${#binpkg_files[@]} == 1 )) || \
    binhost_fail "non-GPKG or extra binary packages exist under ${repo_root}"

  package_count="$(awk '$1 == "PACKAGES:" { print $2 }' "${repo_root}/Packages")"
  [[ "${package_count}" == "1" ]] || \
    binhost_fail "Packages must advertise exactly one package, got '${package_count}'"
  mapfile -t cpvs < <(awk '$1 == "CPV:" { print $2 }' "${repo_root}/Packages")
  (( ${#cpvs[@]} == 1 )) || \
    binhost_fail "Packages must contain exactly one CPV entry"
  [[ "${cpvs[0]}" =~ ^app-admin/oxys-[0-9] ]] || \
    binhost_fail "Packages contains unexpected CPV: ${cpvs[0]}"

  indexed_path="$(awk '$1 == "PATH:" { print $2 }' "${repo_root}/Packages")"
  [[ -n "${indexed_path}" && "${indexed_path}" != *$'\n'* ]] || \
    binhost_fail "Packages must contain exactly one PATH entry"
  relative_path="${packages[0]#${repo_root}/}"
  [[ "${indexed_path}" == "${relative_path}" ]] || \
    binhost_fail "Packages PATH '${indexed_path}' does not match '${relative_path}'"

  repositories="$(awk '$1 == "REPO:" { print $2 }' "${repo_root}/Packages")"
  [[ "${repositories}" == "oxys" ]] || \
    binhost_fail "Packages must contain only REPO=oxys, got '${repositories}'"

  indexed_md5="$(awk '$1 == "MD5:" { print $2 }' "${repo_root}/Packages")"
  indexed_sha1="$(awk '$1 == "SHA1:" { print $2 }' "${repo_root}/Packages")"
  indexed_size="$(awk '$1 == "SIZE:" { print $2 }' "${repo_root}/Packages")"
  actual_md5="$(md5sum "${packages[0]}")"
  actual_md5="${actual_md5%% *}"
  actual_sha1="$(sha1sum "${packages[0]}")"
  actual_sha1="${actual_sha1%% *}"
  actual_size="$(stat -c '%s' "${packages[0]}")"
  [[ "${indexed_md5,,}" == "${actual_md5}" ]] || \
    binhost_fail "Packages MD5 does not match the indexed GPKG"
  [[ "${indexed_sha1,,}" == "${actual_sha1}" ]] || \
    binhost_fail "Packages SHA1 does not match the indexed GPKG"
  [[ "${indexed_size}" == "${actual_size}" ]] || \
    binhost_fail "Packages SIZE does not match the indexed GPKG"

  verify_gpkg "${packages[0]}" "${expected_state}" || \
    binhost_fail "${expected_state} GPKG verification failed: ${packages[0]}"
  log "Validated ${expected_state} one-package GPKG repository at ${repo_root}"
}

run_binhost_build() {
  local build_start build_end

  ensure_binhost_log_dirs
  validate_binhost_target
  if [[ -n "${BINHOST_SIGNING_SOURCE}" || -n "${BINHOST_SIGNING_KEY}" ]]; then
    binhost_fail "signing key material must not enter the privileged emerge builder"
  fi
  require_binhost_commands awk emaint emerge find md5sum portageq python3 \
    sha1sum stat tee
  validate_binhost_overlay

  rm -rf "${BINHOST_UNSIGNED_REPO}" "${BINHOST_SIGNED_REPO}"
  mkdir -p "${BINHOST_UNSIGNED_REPO}"
  select_binhost_profile
  write_binhost_build_config
  assert_binhost_build_config

  build_start="$(date +%s)"
  log "Building isolated unsigned ${BINHOST_ATOM} GPKG candidate"
  emerge \
    --ignore-default-opts \
    --ask=n \
    --verbose \
    --oneshot \
    --buildpkgonly \
    --nodeps \
    --usepkg=n \
    "${BINHOST_ATOM}" 2>&1 | tee "${EMERGE_LOG_DIR}/app-admin--oxys-binhost.log"
  emaint binhost --fix 2>&1 | tee -a "${EMERGE_LOG_DIR}/app-admin--oxys-binhost.log"
  emaint binhost --check 2>&1 | tee -a "${EMERGE_LOG_DIR}/app-admin--oxys-binhost.log"
  validate_binhost_repo "${BINHOST_UNSIGNED_REPO}" unsigned
  build_end="$(date +%s)"
  printf '%s\t%s\t%s\n' "${ARCH_NAME}" "app-admin/oxys (unsigned GPKG)" \
    "$((build_end - build_start))" >> "${TIMES_LOG}"
  log "Unsigned binhost candidate is ready for the offline signer"
}

cleanup_binhost_signer() {
  case "${BINHOST_SIGNING_ROOT}" in
    /run/oxys-binpkg-signing.*) rm -rf -- "${BINHOST_SIGNING_ROOT}" ;;
  esac
  case "${BINHOST_VERIFY_HOME}" in
    /run/oxys-binpkg-verify.*) rm -rf -- "${BINHOST_VERIFY_HOME}" ;;
  esac
  case "${BINHOST_PUBLISH_STAGE}" in
    "${BINHOST_PUBLISH_ROOT}"/.x86-64-v3.staging.*)
      rm -rf -- "${BINHOST_PUBLISH_STAGE}"
      ;;
  esac
}

prepare_binhost_signing_homes() {
  local fingerprint public_key probe unsafe_entry

  [[ -d "${BINHOST_SIGNING_SOURCE}" ]] || \
    binhost_fail "signing source is not a directory: ${BINHOST_SIGNING_SOURCE}"
  [[ "${BINHOST_SIGNING_KEY}" =~ ^[[:xdigit:]]{40}$ ]] || \
    binhost_fail "signing key must be a full 40-digit hexadecimal fingerprint"

  mkdir -p /run/lock
  chmod 0755 /run/lock
  BINHOST_SIGNING_ROOT="$(mktemp -d /run/oxys-binpkg-signing.XXXXXX)"
  BINHOST_SIGNING_HOME="${BINHOST_SIGNING_ROOT}/private"
  mkdir -m 0700 "${BINHOST_SIGNING_HOME}"
  # A normal live GNUPGHOME contains agent sockets. Copy only directories and
  # regular files so neither those sockets nor symlinks escape the read-only
  # source mount or make the custody copy fail.
  (
    cd "${BINHOST_SIGNING_SOURCE}"
    find . -xdev \( -type d -o -type f \) -print0 | \
      tar --null --no-recursion --files-from=- -cf -
  ) | tar -C "${BINHOST_SIGNING_HOME}" --no-same-owner -xf -
  unsafe_entry="$(find "${BINHOST_SIGNING_HOME}" -mindepth 1 \
    \( -type l -o -type p -o -type b -o -type c \) -print -quit)"
  [[ -z "${unsafe_entry}" ]] || \
    binhost_fail "copied GnuPG home contains unsupported special entry: ${unsafe_entry}"
  chmod -R go-rwx "${BINHOST_SIGNING_HOME}"
  chmod 0700 "${BINHOST_SIGNING_ROOT}" "${BINHOST_SIGNING_HOME}"

  gpg --homedir "${BINHOST_SIGNING_HOME}" --batch --list-secret-keys \
    "${BINHOST_SIGNING_KEY}" >/dev/null 2>&1 || \
    binhost_fail "requested signing secret key is unavailable"
  fingerprint="$(gpg --homedir "${BINHOST_SIGNING_HOME}" --batch --with-colons \
    --fingerprint "${BINHOST_SIGNING_KEY}" | awk -F: '$1 == "fpr" { print $10; exit }')"
  [[ "${fingerprint}" =~ ^[[:xdigit:]]{40}$ ]] || \
    binhost_fail "could not resolve the signing-key fingerprint"
  [[ "${fingerprint,,}" == "${BINHOST_SIGNING_KEY,,}" ]] || \
    binhost_fail "signing key must identify the primary release-key fingerprint"

  BINHOST_VERIFY_HOME="$(mktemp -d /run/oxys-binpkg-verify.XXXXXX)"
  public_key="${BINHOST_SIGNING_ROOT}/release-key.asc"
  gpg --homedir "${BINHOST_SIGNING_HOME}" --batch --armor \
    --output "${public_key}" --export "${BINHOST_SIGNING_KEY}"
  [[ -s "${public_key}" ]] || binhost_fail "failed to export the signing public key"
  gpg --homedir "${BINHOST_VERIFY_HOME}" --batch --import "${public_key}" >/dev/null 2>&1
  printf '%s:6:\n' "${fingerprint}" | \
    gpg --homedir "${BINHOST_VERIFY_HOME}" --batch --import-ownertrust >/dev/null 2>&1
  gpg --homedir "${BINHOST_VERIFY_HOME}" --batch --check-trustdb >/dev/null 2>&1
  chmod -R a+rX "${BINHOST_VERIFY_HOME}"
  chmod 0755 "${BINHOST_VERIFY_HOME}"

  probe="${BINHOST_SIGNING_ROOT}/probe"
  printf 'OxysOS GPKG signing probe\n' > "${probe}"
  gpg --homedir "${BINHOST_SIGNING_HOME}" --batch --no-tty --yes \
    --local-user "${BINHOST_SIGNING_KEY}" --digest-algo SHA512 --armor \
    --detach-sign --output "${probe}.asc" "${probe}" || \
    binhost_fail "the release key could not sign a noninteractive probe"
  gpg --homedir "${BINHOST_VERIFY_HOME}" --batch --no-tty \
    --verify "${probe}.asc" "${probe}" >/dev/null 2>&1 || \
    binhost_fail "the signing probe could not be verified"
}

write_binhost_signer_config() {
  cat > /etc/portage/make.conf <<EOF_MAKE
FEATURES="binpkg-signing binpkg-request-signature"
BINPKG_FORMAT="gpkg"
PKGDIR="${BINHOST_SIGNED_REPO}"
PORTAGE_GRPNAME="root"
BINPKG_GPG_SIGNING_BASE_COMMAND="/usr/bin/flock /run/lock/portage-binpkg-gpg.lock /usr/bin/gpg --sign --armor [PORTAGE_CONFIG]"
BINPKG_GPG_SIGNING_DIGEST="SHA512"
BINPKG_GPG_SIGNING_GPG_HOME="${BINHOST_SIGNING_HOME}"
BINPKG_GPG_SIGNING_KEY="${BINHOST_SIGNING_KEY}"
BINPKG_GPG_VERIFY_BASE_COMMAND="/usr/bin/gpg --verify --batch --no-tty --no-auto-check-trustdb --status-fd 2 [PORTAGE_CONFIG] [SIGNATURE]"
BINPKG_GPG_VERIFY_GPG_HOME="${BINHOST_VERIFY_HOME}"
# The signer has all capabilities dropped, so Portage cannot setuid/setgid.
# Verification still uses a separate public-only keyring.
GPG_VERIFY_USER_DROP=""
GPG_VERIFY_GROUP_DROP=""
PORTAGE_BINHOST_HEADER_URI=""
EOF_MAKE
}

publish_signed_binhost() {
  local destination

  mkdir -p "${BINHOST_PUBLISH_ROOT}"
  BINHOST_PUBLISH_STAGE="$(mktemp -d \
    "${BINHOST_PUBLISH_ROOT}/.${BINHOST_FINAL_DIRNAME}.staging.XXXXXX")"
  cp -a "${BINHOST_SIGNED_REPO}/." "${BINHOST_PUBLISH_STAGE}/"
  validate_binhost_repo "${BINHOST_PUBLISH_STAGE}" signed

  destination="${BINHOST_PUBLISH_ROOT}/${BINHOST_FINAL_DIRNAME}"
  if [[ -e "${destination}" ]]; then
    mv --help | grep -q -- '--exchange' || \
      binhost_fail "mv lacks atomic directory exchange support"
    # renameat2(RENAME_EXCHANGE) leaves no interval where the public path is
    # absent. After the exchange, the old repository occupies the hidden
    # staging path and can be removed independently.
    mv --exchange -T "${BINHOST_PUBLISH_STAGE}" "${destination}" || \
      binhost_fail "atomic exchange failed for ${destination}"
    rm -rf "${BINHOST_PUBLISH_STAGE}"
  else
    mv -T "${BINHOST_PUBLISH_STAGE}" "${destination}" || \
      binhost_fail "atomic initial publication failed for ${destination}"
  fi
  BINHOST_PUBLISH_STAGE=""
  log "Published signed binhost at ${destination}"
}

run_binhost_signer() {
  local -a packages=()

  ensure_binhost_log_dirs
  validate_binhost_target
  require_binhost_commands awk chmod cp emaint find flock gpg gpkg-sign grep \
    md5sum mktemp mv python3 sha1sum stat tar
  [[ -d "${BINHOST_UNSIGNED_REPO}" ]] || \
    binhost_fail "unsigned builder output is missing: ${BINHOST_UNSIGNED_REPO}"
  validate_binhost_repo "${BINHOST_UNSIGNED_REPO}" unsigned

  trap cleanup_binhost_signer EXIT
  prepare_binhost_signing_homes
  rm -rf "${BINHOST_SIGNED_REPO}"
  mkdir -p "${BINHOST_SIGNED_REPO}"
  cp -a "${BINHOST_UNSIGNED_REPO}/." "${BINHOST_SIGNED_REPO}/"
  write_binhost_signer_config

  mapfile -d '' packages < <(
    find "${BINHOST_SIGNED_REPO}" -type f -name '*.gpkg.tar' -print0
  )
  (( ${#packages[@]} == 1 )) || \
    binhost_fail "signer requires exactly one staged GPKG"
  log "Signing ${packages[0]} with the isolated release key"
  gpkg-sign --allow-unsigned "${packages[0]}"
  emaint binhost --fix
  emaint binhost --check
  validate_binhost_repo "${BINHOST_SIGNED_REPO}" signed
  publish_signed_binhost
}

main() {
  case "${BUILD_PROFILE}" in
    binhost-build)
      run_binhost_build
      return
      ;;
    binhost-sign)
      run_binhost_signer
      return
      ;;
  esac

  ensure_dirs
  resolve_graphics_policy
  ensure_profile
  write_portage_config
  ensure_binpkg_trust
  stage_offline_git_sources

  log "Using builder-provided Portage"
  ensure_elfutils
  ensure_linux_headers

  while IFS= read -r atom; do
    [[ -z "${atom}" ]] && continue
    run_emerge "${atom}"
  done < <(build_package_queue)

  if [[ "${BUILD_PROFILE}" == "kernel" ]]; then
    publish_kernel_artifacts
  fi

  log "Build profile ${BUILD_PROFILE} for ${ARCH_NAME} completed"
}

main "$@"
