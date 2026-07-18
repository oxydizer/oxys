#!/bin/bash
# oxys-iso override of catalyst's targets/support/kmerge.sh.
#
# Forked from gentoo/catalyst (github.com/gentoo/catalyst), master branch,
# targets/support/kmerge.sh, as read on 2026-07-06. Re-diff against upstream
# when bumping the catalyst version pinned in ../Containerfile -- this file
# intentionally keeps upstream's variable-sourcing preamble and output
# contract so a diff stays meaningful.
#
# WHY THIS EXISTS: catalyst's stock kmerge.sh always `emerge`s a real kernel
# package (sys-kernel/gentoo-kernel[-bin] or gentoo-sources, plus any
# boot/kernel/<label>/packages) for every boot/kernel label, on every run --
# there is no spec-level flag to skip that. For OxysOS we need the *exact*
# kernel + zfs-kmod tarball pair that oxys-build already built and
# vermagic-verified (see
# ../../oxys-build/podman/scripts/oxys-build-packages.sh), not a second,
# independently-(re)built kernel -- that's precisely the kernel/zfs-kmod
# version-skew bug the artifact pairing checks exist to prevent, one layer up
# (ISO kernel vs. post-install package-pipeline kernel silently diverging).
#
# This script replaces catalyst's kmerge.sh entirely (installed over it by
# ../Containerfile) but preserves its EXACT output contract: it still
# produces the same two tarballs under /tmp/kerncache/, named and shaped
# exactly like upstream's distkernel path, so every downstream consumer
# (targets/support/functions.sh's extract_kernels/extract_modules,
# targets/support/iso-bootloader-setup.sh's grub.cfg generation) needs zero
# changes and stays 100% stock catalyst.
#
# DATA PATH: catalyst unconditionally bind-mounts the host DISTDIR into the
# chroot before any controller action runs (see catalyst/base/stagebase.py,
# self.mount['distdir']) -- this is the only host->chroot bridge available
# before build_kernel() executes (livecd/overlay and livecd/root_overlay only
# land in target_path, and only *after* build_kernel already ran). So
# build.sh stages the resolved kernel + zfs-kmod tarballs, their .metadata
# sidecars, and a manifest.env into DISTDIR/oxys-kernel-cache/ on the host
# before invoking catalyst; we read them back out from there here.

source /tmp/chroot-functions.sh

install -d /tmp/kerncache

[ -n "${clst_ENVSCRIPT}" ] && source /tmp/envscript

# Set the timezone for the kernel build (parity with upstream kmerge.sh).
rm -f /etc/localtime
cp -f /usr/share/zoneinfo/UTC /etc/localtime

eval "eval kernel_dracut_kernargs=( \$clst_boot_kernel_${kname}_dracut_args )"
eval "distkernel=\$clst_boot_kernel_${kname}_distkernel"

[[ "${distkernel}" == "yes" ]] || die "oxys kmerge.sh override: only the distkernel path is supported (expected boot/kernel/${kname}/distkernel: yes, got '${distkernel}')."

# --- locate the staged cache via catalyst's real DISTDIR, not a hardcoded
#     path -- Portage's default DISTDIR location has moved over the years,
#     and this must work regardless of what this container's Portage uses. --
DISTDIR="$(portageq envvar DISTDIR)"
[[ -n "${DISTDIR}" && -d "${DISTDIR}" ]] || die "oxys kmerge.sh override: 'portageq envvar DISTDIR' returned nothing usable ('${DISTDIR}')."

CACHE_DIR="${DISTDIR}/oxys-kernel-cache"
MANIFEST="${CACHE_DIR}/manifest.env"
[[ -f "${MANIFEST}" ]] || die "oxys kmerge.sh override: no ${MANIFEST}. build.sh should have staged the resolved kernel/zfs-kmod tarballs there before invoking catalyst -- this means that staging step didn't run, or DISTDIR isn't what build.sh expected. Never falling back to emerging a real kernel here."

# shellcheck disable=SC1090
source "${MANIFEST}"

for var in OXYS_BUILD_ID OXYS_KERNEL_RELEASE OXYS_KERNEL_TARBALL OXYS_KERNEL_METADATA OXYS_ZFS_KMOD_TARBALL OXYS_ZFS_KMOD_METADATA; do
	[[ -n "${!var:-}" ]] || die "oxys kmerge.sh override: ${MANIFEST} is missing ${var}."
done

# metadata_field FILE KEY - value of the first "KEY=..." line, first '=' only.
# Same helper/discipline as scripts/resolve-kernel-build.sh on the host side:
# trust metadata contents, never filenames.
metadata_field() {
	local file="$1" key="$2"
	awk -F'=' -v k="${key}" '$1 == k { sub(/^[^=]*=/, ""); print; exit }' "${file}"
}

# --- re-verify the pairing inside the chroot too. build.sh already verified
#     this on the host in scripts/resolve-kernel-build.sh; this is a cheap
#     belt-and-suspenders re-check that catches any staging/transport issue,
#     same discipline as the build-time vermagic check in oxys-build. ---
kernel_meta="${CACHE_DIR}/${OXYS_KERNEL_METADATA}"
zfs_kmod_meta="${CACHE_DIR}/${OXYS_ZFS_KMOD_METADATA}"
[[ -f "${kernel_meta}" ]] || die "oxys kmerge.sh override: manifest names kernel metadata ${OXYS_KERNEL_METADATA}, not found at ${kernel_meta}."
[[ -f "${zfs_kmod_meta}" ]] || die "oxys kmerge.sh override: manifest names zfs-kmod metadata ${OXYS_ZFS_KMOD_METADATA}, not found at ${zfs_kmod_meta}."

kernel_build_id="$(metadata_field "${kernel_meta}" build_id)"
zfs_kmod_build_id="$(metadata_field "${zfs_kmod_meta}" build_id)"
kernel_release_check="$(metadata_field "${kernel_meta}" kernel_release)"
zfs_kmod_kernel_release="$(metadata_field "${zfs_kmod_meta}" kernel_release)"

[[ "${kernel_build_id}" == "${OXYS_BUILD_ID}" ]] || die "oxys kmerge.sh override: staged kernel metadata build_id (${kernel_build_id}) doesn't match manifest (${OXYS_BUILD_ID})."
[[ "${zfs_kmod_build_id}" == "${OXYS_BUILD_ID}" ]] || die "oxys kmerge.sh override: staged zfs-kmod metadata build_id (${zfs_kmod_build_id}) doesn't match manifest (${OXYS_BUILD_ID})."
[[ "${kernel_release_check}" == "${OXYS_KERNEL_RELEASE}" ]] || die "oxys kmerge.sh override: staged kernel metadata kernel_release (${kernel_release_check}) doesn't match manifest (${OXYS_KERNEL_RELEASE})."
[[ "${zfs_kmod_kernel_release}" == "${OXYS_KERNEL_RELEASE}" ]] || die "oxys kmerge.sh override: kernel/zfs-kmod pairing check failed inside chroot (kernel=${OXYS_KERNEL_RELEASE}, zfs-kmod=${zfs_kmod_kernel_release}). Refusing to boot a mismatched pair."

echo "oxys kmerge.sh override: injecting prebuilt kernel ${OXYS_KERNEL_RELEASE} (build_id=${OXYS_BUILD_ID}) for label '${kname}'"

# --- unpack the kernel tarball (boot/vmlinuz-*, boot/System.map-*,
#     boot/config-*, lib/modules/<kernel_release>/... -- see
#     oxys-build-packages.sh's build_kernel_artifacts) and the zfs-kmod
#     tarball (a full-path tar of installed vdb CONTENTS -- see
#     archive_from_vdb) directly into the chroot's real filesystem. ---
# --keep-directory-symlink: both tarballs were staged from a plain (non
# merged-usr) root on the oxys-build side, so they carry literal directory
# entries for paths like "lib" and "usr". Without this flag, GNU tar's
# default on extracting a directory entry over an existing symlink-to-directory
# is to unlink the symlink and replace it with a real directory -- which would
# destroy this chroot's merged-usr /lib -> usr/lib symlink and orphan
# everything reachable through it (e.g. /lib/gentoo/functions.sh,
# /usr/lib/dracut's modules via /lib/dracut), rather than writing through it.
tar -C / --keep-directory-symlink -xzf "${CACHE_DIR}/${OXYS_KERNEL_TARBALL}" || die "oxys kmerge.sh override: failed to extract ${OXYS_KERNEL_TARBALL}."
tar -C / --keep-directory-symlink -xzf "${CACHE_DIR}/${OXYS_ZFS_KMOD_TARBALL}" || die "oxys kmerge.sh override: failed to extract ${OXYS_ZFS_KMOD_TARBALL}."

[[ -e "/boot/vmlinuz-${OXYS_KERNEL_RELEASE}" ]] || die "oxys kmerge.sh override: expected /boot/vmlinuz-${OXYS_KERNEL_RELEASE} after extracting ${OXYS_KERNEL_TARBALL}, not found."
[[ -d "/lib/modules/${OXYS_KERNEL_RELEASE}" ]] || die "oxys kmerge.sh override: expected /lib/modules/${OXYS_KERNEL_RELEASE} after extracting kernel+zfs-kmod tarballs, not found."

depmod -a "${OXYS_KERNEL_RELEASE}" || die "oxys kmerge.sh override: depmod failed for ${OXYS_KERNEL_RELEASE}."

# --- initramfs for the LIVE ISO itself (dmsquash-live etc). This is
#     ISO-specific and can't be pre-baked by oxys-build (it has no notion of
#     "live medium"), so we still run dracut here, same as upstream's
#     distkernel path -- just pointed at the injected kernel instead of one
#     catalyst just emerged. ---
DRACUT_ARGS=(
	"${kernel_dracut_kernargs[@]}"
	--force
	--kernel-image="/boot/vmlinuz-${OXYS_KERNEL_RELEASE}"
	--kver="${OXYS_KERNEL_RELEASE}"
)
dracut "${DRACUT_ARGS[@]}" || exit 1

# --- produce the exact same two kerncache tarballs upstream's distkernel
#     path produces (System.map*, config*, initramfs*, vmlinuz*, vmlinux* at
#     the tar root; lib/modules for the modules tarball), so
#     extract_kernels/extract_modules/grub.cfg generation downstream need
#     zero changes. ---
cd /boot || die "oxys kmerge.sh override: /boot missing."
tar jcvf "/tmp/kerncache/${kname}-kernel-initrd-${clst_version_stamp}.tar.bz2" System.map* config* initramfs* vmlinuz* vmlinux*
cd /
tar jcvf "/tmp/kerncache/${kname}-modules-${clst_version_stamp}.tar.bz2" lib/modules

echo "oxys kmerge.sh override: done for label '${kname}'"
