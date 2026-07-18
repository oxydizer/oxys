#!/usr/bin/env bash
# resolve-kernel-build.sh - locate and verify the kernel + zfs-kmod + zfs
# (userland) artifact set produced by oxys-build for a given arch.
#
# This is the single place that decides "which kernel does the ISO ship."
# oxys-build publishes the exact stable archive names in kernel-artifacts.env
# only after all three archives have completed. Each archive also has a
# .metadata sidecar (build_id, arch, atom, version, kernel_release, ...).
# Metadata remains authoritative for pairing and branding checks.
#
# Usage:
#   OXYS_ARCH=alderlake ./resolve-kernel-build.sh
#
# On success: prints resolved paths as KEY=value lines on stdout (meant to be
# `source`d) and exits 0. On any failure: prints a clear error to stderr and
# exits 1. Never falls back to "no kernel" -- callers must fail the build.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
MONOREPO_ROOT="$(cd "${SCRIPT_DIR}/../.." && pwd)"
OUTPUT_ROOT="${MONOREPO_ROOT}/oxys-build/output"

die() {
	printf 'resolve-kernel-build.sh: ERROR: %s\n' "$1" >&2
	exit 1
}

[[ -n "${OXYS_ARCH:-}" ]] || die "OXYS_ARCH is required (no default). Available arches: $(
	find "${OUTPUT_ROOT}" -mindepth 1 -maxdepth 1 -type d -printf '%f ' 2>/dev/null || printf '(none -- run oxys-build first)'
)"

ARCH="${OXYS_ARCH}"
ARCH_DIR="${OUTPUT_ROOT}/${ARCH}"

[[ -d "${ARCH_DIR}" ]] || die "no oxys-build output for arch '${ARCH}' at ${ARCH_DIR}. Run oxys-build for this arch first."

# metadata_field FILE KEY - value of the first "KEY=..." line, first '=' only.
metadata_field() {
	local file="$1" key="$2"
	awk -F'=' -v k="${key}" '$1 == k { sub(/^[^=]*=/, ""); print; exit }' "${file}"
}

ARTIFACTS_FILE="${ARCH_DIR}/kernel-artifacts.env"
[[ -f "${ARTIFACTS_FILE}" ]] || die "no ${ARTIFACTS_FILE}; run the oxys-build kernel profile for arch=${ARCH}."

BUILD_ID="$(metadata_field "${ARTIFACTS_FILE}" build_id)"
MANIFEST_ARCH="$(metadata_field "${ARTIFACTS_FILE}" arch)"
KERNEL_ARCHIVE="$(metadata_field "${ARTIFACTS_FILE}" kernel_archive)"
ZFS_KMOD_ARCHIVE="$(metadata_field "${ARTIFACTS_FILE}" zfs_kmod_archive)"
ZFS_USERLAND_ARCHIVE="$(metadata_field "${ARTIFACTS_FILE}" zfs_userland_archive)"
[[ -n "${BUILD_ID}" ]] || die "${ARTIFACTS_FILE} has no build_id field."
[[ "${MANIFEST_ARCH}" == "${ARCH}" ]] || die "${ARTIFACTS_FILE} records arch=${MANIFEST_ARCH}, expected ${ARCH}."

for archive in "${KERNEL_ARCHIVE}" "${ZFS_KMOD_ARCHIVE}" "${ZFS_USERLAND_ARCHIVE}"; do
	[[ -n "${archive}" && "${archive}" == "$(basename "${archive}")" && "${archive}" == *.tar.gz ]] || die "invalid archive name '${archive}' in ${ARTIFACTS_FILE}."
	[[ -f "${ARCH_DIR}/${archive}" ]] || die "published archive is missing: ${ARCH_DIR}/${archive}"
	[[ -f "${ARCH_DIR}/${archive%.tar.gz}.metadata" ]] || die "published metadata is missing: ${ARCH_DIR}/${archive%.tar.gz}.metadata"
done

KERNEL_TARBALL="${ARCH_DIR}/${KERNEL_ARCHIVE}"
ZFS_KMOD_TARBALL="${ARCH_DIR}/${ZFS_KMOD_ARCHIVE}"
ZFS_USERLAND_TARBALL="${ARCH_DIR}/${ZFS_USERLAND_ARCHIVE}"
KERNEL_METADATA="${ARCH_DIR}/${KERNEL_ARCHIVE%.tar.gz}.metadata"
ZFS_KMOD_METADATA="${ARCH_DIR}/${ZFS_KMOD_ARCHIVE%.tar.gz}.metadata"
ZFS_USERLAND_METADATA="${ARCH_DIR}/${ZFS_USERLAND_ARCHIVE%.tar.gz}.metadata"

for metadata in "${KERNEL_METADATA}" "${ZFS_KMOD_METADATA}" "${ZFS_USERLAND_METADATA}"; do
	[[ "$(metadata_field "${metadata}" build_id)" == "${BUILD_ID}" ]] || die "artifact set is incomplete: ${metadata} does not match build_id=${BUILD_ID}."
	[[ "$(metadata_field "${metadata}" arch)" == "${ARCH}" ]] || die "artifact architecture mismatch in ${metadata}."
done

[[ "$(metadata_field "${KERNEL_METADATA}" archive)" == "${KERNEL_ARCHIVE}" ]] || die "kernel metadata does not name ${KERNEL_ARCHIVE}."
[[ "$(metadata_field "${ZFS_KMOD_METADATA}" archive)" == "${ZFS_KMOD_ARCHIVE}" ]] || die "zfs-kmod metadata does not name ${ZFS_KMOD_ARCHIVE}."
[[ "$(metadata_field "${ZFS_USERLAND_METADATA}" archive)" == "${ZFS_USERLAND_ARCHIVE}" ]] || die "zfs metadata does not name ${ZFS_USERLAND_ARCHIVE}."

[[ "$(metadata_field "${KERNEL_METADATA}" atom)" == sys-kernel/gentoo-sources* ]] || die "kernel metadata has the wrong atom: ${KERNEL_METADATA}"
[[ "$(metadata_field "${ZFS_KMOD_METADATA}" atom)" == sys-fs/zfs-kmod-* ]] || die "zfs-kmod metadata has the wrong atom: ${ZFS_KMOD_METADATA}"
[[ "$(metadata_field "${ZFS_USERLAND_METADATA}" atom)" == sys-fs/zfs-[0-9]* ]] || die "zfs metadata has the wrong atom: ${ZFS_USERLAND_METADATA}"

KERNEL_RELEASE="$(metadata_field "${KERNEL_METADATA}" kernel_release)"
ZFS_KMOD_KERNEL_RELEASE="$(metadata_field "${ZFS_KMOD_METADATA}" kernel_release)"
KERNEL_DRM_DRIVERS="$(metadata_field "${KERNEL_METADATA}" drm_drivers)"

[[ -n "${KERNEL_RELEASE}" ]] || die "kernel metadata ${KERNEL_METADATA} has no kernel_release field."
[[ "${KERNEL_RELEASE}" == "${ZFS_KMOD_KERNEL_RELEASE}" ]] || die "kernel/zfs-kmod pairing check failed: kernel_release mismatch (kernel=${KERNEL_RELEASE}, zfs-kmod=${ZFS_KMOD_KERNEL_RELEASE}) for build_id=${BUILD_ID}. This should be impossible for a single build_id -- do not use these artifacts."
[[ "${KERNEL_RELEASE}" == *-oxys ]] || die "kernel release '${KERNEL_RELEASE}' is not OxysOS-branded (expected an -oxys suffix). Rebuild the kernel before building the ISO."
[[ "${KERNEL_RELEASE}" != *gentoo* ]] || die "kernel release '${KERNEL_RELEASE}' still contains 'gentoo'. Rebuild the kernel before building the ISO."

if [[ -n "${OXYS_DRM_DRIVERS:-}" ]]; then
	requested=" ${OXYS_DRM_DRIVERS//,/ } "
	available=" ${KERNEL_DRM_DRIVERS} "
	for driver in ${requested}; do
		[[ "${available}" == *" ${driver} "* ]] || die "kernel artifact lacks requested DRM driver '${driver}' (built: ${KERNEL_DRM_DRIVERS:-unrecorded}). Rebuild oxys-build with matching OXYS_DRM_DRIVERS."
	done
fi

cat <<EOF
OXYS_RESOLVED_ARCH=${ARCH}
OXYS_RESOLVED_BUILD_ID=${BUILD_ID}
OXYS_RESOLVED_KERNEL_RELEASE=${KERNEL_RELEASE}
OXYS_KERNEL_TARBALL=${KERNEL_TARBALL}
OXYS_KERNEL_METADATA=${KERNEL_METADATA}
OXYS_ZFS_KMOD_TARBALL=${ZFS_KMOD_TARBALL}
OXYS_ZFS_KMOD_METADATA=${ZFS_KMOD_METADATA}
OXYS_ZFS_USERLAND_TARBALL=${ZFS_USERLAND_TARBALL}
OXYS_ZFS_USERLAND_METADATA=${ZFS_USERLAND_METADATA}
EOF
