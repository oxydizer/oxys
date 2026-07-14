#!/usr/bin/env bash
# resolve-kernel-build.sh - locate and verify a build-id-tagged kernel +
# zfs-kmod + zfs (userland) tarball set produced by oxys-build, for a given
# arch.
#
# This is the single place that decides "which kernel does the ISO ship."
# oxys-build/podman/scripts/oxys-build-packages.sh writes one .metadata
# sidecar per archive (build_id, arch, atom, version, kernel_release, ...).
# We trust ONLY those metadata contents to pair artifacts together, never
# filenames -- same discipline as the vermagic check in oxys-build-packages.sh.
#
# Usage:
#   OXYS_ARCH=alderlake ./resolve-kernel-build.sh
#   OXYS_ARCH=alderlake OXYS_KERNEL_BUILD_ID=20260706T011218Z-gentoo-20260706T003749Z ./resolve-kernel-build.sh
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

if [[ -n "${OXYS_KERNEL_BUILD_ID:-}" ]]; then
	BUILD_ID="${OXYS_KERNEL_BUILD_ID}"
else
	BUILD_ID_FILE="${ARCH_DIR}/build-id"
	[[ -f "${BUILD_ID_FILE}" ]] || die "OXYS_KERNEL_BUILD_ID not set and no ${BUILD_ID_FILE} to default from."
	BUILD_ID="$(<"${BUILD_ID_FILE}")"
	[[ -n "${BUILD_ID}" ]] || die "${BUILD_ID_FILE} is empty."
fi

# metadata_field FILE KEY - value of the first "KEY=..." line, first '=' only.
metadata_field() {
	local file="$1" key="$2"
	awk -F'=' -v k="${key}" '$1 == k { sub(/^[^=]*=/, ""); print; exit }' "${file}"
}

KERNEL_METADATA="" KERNEL_TARBALL=""
ZFS_KMOD_METADATA="" ZFS_KMOD_TARBALL=""
ZFS_USERLAND_METADATA="" ZFS_USERLAND_TARBALL=""
KERNEL_CREATED="" ZFS_KMOD_CREATED="" ZFS_USERLAND_CREATED=""

shopt -s nullglob
for meta in "${ARCH_DIR}"/*.metadata; do
	found_build_id="$(metadata_field "${meta}" build_id)"
	[[ "${found_build_id}" == "${BUILD_ID}" ]] || continue

	# Strip a possible leading '=' (pre-existing quirk in older metadata:
	# atoms carrying a Portage version pin, e.g. "=sys-fs/zfs-kmod-2.3.8",
	# were written verbatim as "atom==sys-fs/zfs-kmod-2.3.8"). Don't trust
	# the filename to disambiguate -- only the atom field.
	atom="$(metadata_field "${meta}" atom)"
	atom="${atom#=}"
	archive_name="$(metadata_field "${meta}" archive)"
	[[ -n "${archive_name}" ]] || die "malformed metadata (no archive= line): ${meta}"
	archive_path="${ARCH_DIR}/${archive_name}"
	[[ -f "${archive_path}" ]] || die "metadata ${meta} names archive ${archive_name}, but it doesn't exist at ${archive_path}"
	created_utc="$(metadata_field "${meta}" created_utc)"
	[[ -n "${created_utc}" ]] || die "malformed metadata (no created_utc= line): ${meta}"

	case "${atom}" in
	sys-kernel/gentoo-sources*)
		if [[ -z "${KERNEL_CREATED}" || "${created_utc}" > "${KERNEL_CREATED}" ]]; then
			KERNEL_METADATA="${meta}"
			KERNEL_TARBALL="${archive_path}"
			KERNEL_CREATED="${created_utc}"
		elif [[ "${created_utc}" == "${KERNEL_CREATED}" && "${meta}" != "${KERNEL_METADATA}" ]]; then
			die "ambiguous kernel metadata with identical created_utc=${created_utc}: ${KERNEL_METADATA} and ${meta}"
		fi
		;;
	sys-fs/zfs-kmod-*)
		if [[ -z "${ZFS_KMOD_CREATED}" || "${created_utc}" > "${ZFS_KMOD_CREATED}" ]]; then
			ZFS_KMOD_METADATA="${meta}"
			ZFS_KMOD_TARBALL="${archive_path}"
			ZFS_KMOD_CREATED="${created_utc}"
		elif [[ "${created_utc}" == "${ZFS_KMOD_CREATED}" && "${meta}" != "${ZFS_KMOD_METADATA}" ]]; then
			die "ambiguous zfs-kmod metadata with identical created_utc=${created_utc}: ${ZFS_KMOD_METADATA} and ${meta}"
		fi
		;;
	sys-fs/zfs-[0-9]*)
		if [[ -z "${ZFS_USERLAND_CREATED}" || "${created_utc}" > "${ZFS_USERLAND_CREATED}" ]]; then
			ZFS_USERLAND_METADATA="${meta}"
			ZFS_USERLAND_TARBALL="${archive_path}"
			ZFS_USERLAND_CREATED="${created_utc}"
		elif [[ "${created_utc}" == "${ZFS_USERLAND_CREATED}" && "${meta}" != "${ZFS_USERLAND_METADATA}" ]]; then
			die "ambiguous zfs metadata with identical created_utc=${created_utc}: ${ZFS_USERLAND_METADATA} and ${meta}"
		fi
		;;
	esac
done
shopt -u nullglob

[[ -n "${KERNEL_TARBALL}" ]] || die "no kernel (sys-kernel/gentoo-sources) archive found for arch=${ARCH} build_id=${BUILD_ID} under ${ARCH_DIR}. Run oxys-build for this arch/build-id first."
[[ -n "${ZFS_KMOD_TARBALL}" ]] || die "no zfs-kmod (sys-fs/zfs-kmod) archive found for arch=${ARCH} build_id=${BUILD_ID} under ${ARCH_DIR}. Run oxys-build for this arch/build-id first."
[[ -n "${ZFS_USERLAND_TARBALL}" ]] || die "no zfs userland (sys-fs/zfs) archive found for arch=${ARCH} build_id=${BUILD_ID} under ${ARCH_DIR}. Run oxys-build for this arch/build-id first."

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
		[[ "${available}" == *" ${driver} "* ]] || die "kernel build ${BUILD_ID} lacks requested DRM driver '${driver}' (built: ${KERNEL_DRM_DRIVERS:-unrecorded}). Rebuild oxys-build with matching OXYS_DRM_DRIVERS."
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
