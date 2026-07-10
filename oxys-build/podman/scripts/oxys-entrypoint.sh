#!/usr/bin/env bash
set -euo pipefail

WORK_ROOT="${OXYS_WORK_ROOT:-/work}"
SOURCE_ROOT="${OXYS_SOURCE_ROOT:-/src}"
PACKAGE_LIST_FILE="${WORK_ROOT}/oxys-packages.txt"
if [[ ! -f "${PACKAGE_LIST_FILE}" ]]; then
  PACKAGE_LIST_FILE="${SOURCE_ROOT}/oxys-packages.txt"
fi
OVERLAY_ROOT="${WORK_ROOT}/oxys-overlay"
if [[ ! -d "${OVERLAY_ROOT}" ]]; then
  OVERLAY_ROOT="${SOURCE_ROOT}/oxys-overlay"
fi

readonly REPO_ROOT="/var/db/repos/gentoo"
readonly OXYS_OVERLAY_REPO="/var/db/repos/oxys"
readonly PROFILE_TARGET="default/linux/amd64/23.0"
readonly REPOS_CONF_FILE="/etc/portage/repos.conf/gentoo.conf"
readonly OXYS_REPOS_CONF_FILE="/etc/portage/repos.conf/oxys.conf"
readonly KERNEL_MASK_FILE="/etc/portage/package.mask/no-kernel"

log() {
  printf '[entrypoint] %s\n' "$*"
}

ensure_binutils_header_links() {
  local include_root="/usr/lib64/binutils/x86_64-pc-linux-gnu"
  local preferred="${include_root}/2.45.1/include"
  local selected=""

  if [[ -d "${preferred}" ]]; then
    selected="${preferred}"
  else
    selected="$(find "${include_root}" -maxdepth 2 -type d -path '*/include' | sort -V | tail -n 1)"
  fi

  if [[ -z "${selected}" ]]; then
    log "Skipping binutils header symlinks; no binutils include dir found"
    return 0
  fi

  ln -sf "${selected}/bfd.h" /usr/include/bfd.h
  ln -sf "${selected}/bfdver.h" /usr/include/bfdver.h
  ln -sf "${selected}/ansidecl.h" /usr/include/ansidecl.h
}

ensure_dirs() {
  mkdir -p /etc/portage/repos.conf /etc/portage/package.mask "${REPO_ROOT}"
}

ensure_repos_conf() {
  cat > "${REPOS_CONF_FILE}" <<'EOF_REPO'
[gentoo]
location = /var/db/repos/gentoo
sync-type = rsync
sync-uri = rsync://rsync.gentoo.org/gentoo-portage
auto-sync = yes
EOF_REPO

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
}

ensure_repo_synced() {
  if [[ ! -d "${REPO_ROOT}/profiles" ]]; then
    log "Syncing Gentoo repository"
    emerge-webrsync
  fi
}

ensure_profile() {
  local current_target
  current_target="$(readlink /etc/portage/make.profile || true)"
  if [[ "${current_target}" != "${REPO_ROOT}/profiles/${PROFILE_TARGET}" ]]; then
    log "Selecting profile ${PROFILE_TARGET}"
    rm -f /etc/portage/make.profile
    ln -s "${REPO_ROOT}/profiles/${PROFILE_TARGET}" /etc/portage/make.profile
  fi
}

ensure_package_list() {
  if [[ ! -f "${PACKAGE_LIST_FILE}" ]]; then
    log "Missing package list: ${PACKAGE_LIST_FILE}"
    exit 1
  fi
}

ensure_kernel_mask() {
  if [[ ! -f "${KERNEL_MASK_FILE}" ]]; then
    printf '%s\n' "sys-kernel/gentoo-sources" > "${KERNEL_MASK_FILE}"
  fi
}

ensure_package_use_bootstrap() {
  mkdir -p /etc/portage/package.use /etc/portage/package.accept_keywords
  if ! grep -qxF ">=net-wireless/wpa_supplicant-2.11 dbus" /etc/portage/package.use/oxys 2>/dev/null; then
    printf '%s\n' ">=net-wireless/wpa_supplicant-2.11 dbus" >> /etc/portage/package.use/oxys
  fi
  if ! grep -qxF "gui-shells/noctalia jemalloc" /etc/portage/package.use/oxys 2>/dev/null; then
    printf '%s\n' "gui-shells/noctalia jemalloc" >> /etc/portage/package.use/oxys
  fi
  if ! grep -qxF "gui-shells/noctalia **" /etc/portage/package.accept_keywords/oxys 2>/dev/null; then
    printf '%s\n' "gui-shells/noctalia **" >> /etc/portage/package.accept_keywords/oxys
  fi
}

command_targets_kernel_sources() {
  local arg
  for arg in "$@"; do
    if [[ "${arg}" == "sys-kernel/gentoo-sources" ]]; then
      return 0
    fi
  done
  return 1
}

command_is_emerge() {
  [[ "$#" -gt 0 && "$1" == "emerge" ]]
}

command_targets_linux_headers() {
  local arg
  for arg in "$@"; do
    if [[ "${arg}" == "sys-kernel/linux-headers" ]]; then
      return 0
    fi
  done
  return 1
}

temporarily_unmask_kernel_sources() {
  if [[ -f "${KERNEL_MASK_FILE}" ]]; then
    log "Temporarily unmasking sys-kernel/gentoo-sources"
    rm -f "${KERNEL_MASK_FILE}"
  fi
}

ensure_linux_headers() {
  log "Installing sys-kernel/linux-headers"
  emerge --autounmask=y --autounmask-write=y --verbose sys-kernel/linux-headers || true
  etc-update --automode -5
  emerge --verbose sys-kernel/linux-headers
}

ensure_elfutils() {
  log "Installing dev-libs/elfutils"
  emerge --autounmask=y --autounmask-write=y --verbose dev-libs/elfutils || true
  etc-update --automode -5
  emerge --verbose dev-libs/elfutils
}

main() {
  ensure_dirs
  ensure_repos_conf
  ensure_repo_synced
  ensure_profile
  ensure_package_list
  ensure_kernel_mask
  ensure_binutils_header_links
  ensure_package_use_bootstrap

  if [[ "$#" -gt 0 ]]; then
    if command_targets_kernel_sources "$@"; then
      temporarily_unmask_kernel_sources
    fi
    if command_is_emerge "$@" && ! command_targets_linux_headers "$@"; then
      ensure_elfutils
      ensure_linux_headers
    fi
    exec "$@"
  fi

  exec /work/scripts/oxys-build-packages.sh
}

main "$@"
