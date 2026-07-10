#!/bin/bash
# OxysOS ISO build driver — run this INSIDE the catalyst environment
# (the Gentoo container/chroot described in README.md), not on Arch directly.
# ---------------------------------------------------------------------------
# It:
#   1. derives a single build timestamp shared by both stages,
#   2. substitutes @TOKENS@ in the spec templates into a work dir,
#   3. runs livecd-stage1 then livecd-stage2 in order.
#
# Catalyst storage (storedir) is assumed at /var/tmp/catalyst (set in
# /etc/catalyst/catalyst.conf). The seed stage3 must already be placed — see
# the "SEED" step in README.md.
# ---------------------------------------------------------------------------
set -euo pipefail

REPO_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
WORK="${REPO_DIR}/.build"
SPECS_SRC="${REPO_DIR}/specs"

# Monorepo root (oxys-iso's parent dir). oxys-build's tagged kernel/zfs-kmod
# output lives at ${MONOREPO_ROOT}/oxys-build/output/<arch>/ — see
# scripts/resolve-kernel-build.sh.
MONOREPO_ROOT="$(cd "${REPO_DIR}/.." && pwd)"
INSTALLER_CONFIG_DIR="${MONOREPO_ROOT}/oxys-installer/configs"
OVERLAY_CONFIG_DIR="${REPO_DIR}/overlay/root/configs"

# --- tunables ---------------------------------------------------------------
STOREDIR="${OXYS_STOREDIR:-/var/tmp/catalyst}"
# Where catalyst drops the finished ISO + its sidecars (and where the seed and
# stage tarballs live). rel_type/subarch are fixed by our specs to 23.0-default.
BUILDS_DIR="${STOREDIR}/builds/23.0-default"

# A gentoo repo snapshot id. Create it first (see README): `catalyst -s stable`.
# catalyst names the resulting file after the RESOLVED commit hash, not the
# treeish string you passed to `-s` -- so "stable" itself is never a real
# snapshot_treeish value once a snapshot exists (there is no gentoo-stable.sqfs).
# Default to whichever gentoo-*.sqfs is newest under snapshots/ (i.e. the most
# recent `catalyst -s` run); set OXYS_TREEISH to pin a specific/older one.
if [[ -n "${OXYS_TREEISH:-}" ]]; then
	TREEISH="${OXYS_TREEISH}"
else
	LATEST_SNAPSHOT="$(find "${STOREDIR}/snapshots" -maxdepth 1 -name 'gentoo-*.sqfs' -printf '%T@ %f\n' 2>/dev/null \
		| sort -rn | head -n1 | cut -d' ' -f2-)"
	if [[ -z "${LATEST_SNAPSHOT}" ]]; then
		echo "ERROR: no gentoo-*.sqfs snapshot found under ${STOREDIR}/snapshots." >&2
		echo "       Run 'catalyst -s stable' first (see README), or set OXYS_TREEISH explicitly." >&2
		exit 1
	fi
	TREEISH="${LATEST_SNAPSHOT#gentoo-}"
	TREEISH="${TREEISH%.sqfs}"
	echo ">> auto-resolved snapshot treeish: ${TREEISH} (from ${LATEST_SNAPSHOT})"
fi

# Which oxys-build arch output to pull the prebuilt kernel+zfs-kmod from.
# Required, no default: this is a hardware-targeted kernel build, and
# silently picking one would be exactly the kind of implicit-default footgun
# this project is otherwise trying to avoid (see doc.md). Default build-id
# within that arch is oxys-build's own "current build" pointer, see
# scripts/resolve-kernel-build.sh.
if [[ -z "${OXYS_ARCH:-}" ]]; then
	echo "ERROR: OXYS_ARCH is required (e.g. OXYS_ARCH=alderlake)." >&2
	echo "       Available arches: $(find "${MONOREPO_ROOT}/oxys-build/output" -mindepth 1 -maxdepth 1 -type d -printf '%f ' 2>/dev/null || echo '(none -- run oxys-build first)')" >&2
	exit 1
fi

# Single timestamp shared by both stages so stage2 can locate stage1 output.
TIMESTAMP="$(date -u +%Y%m%dT%H%M%SZ)"
DATESTAMP="$(date -u +%Y%m%d)"

# --- sanity: catalyst available? --------------------------------------------
if ! command -v catalyst >/dev/null 2>&1; then
	echo "ERROR: catalyst is not installed or not on PATH." >&2
	echo "       Run this script inside the Gentoo catalyst container/chroot," >&2
	echo "       then install catalyst there if needed:" >&2
	echo "         emerge-webrsync" >&2
	echo "         emerge -j dev-util/catalyst" >&2
	echo "" >&2
	echo "       From the Arch host, create the storage dir (podman won't" >&2
	echo "       auto-create bind-mount sources), then enter the container." >&2
	echo "       Run it ROOTFUL (sudo) with loop passthrough: catalyst mounts" >&2
	echo "       its squashfs snapshot via a loop device, which rootless podman" >&2
	echo "       cannot do even with --privileged." >&2
	echo "         mkdir -p ${HOME}/catalyst/{builds,packages,snapshots,tmp,kerncache}" >&2
	echo "         sudo podman run --privileged --rm -it \\" >&2
	echo "           --device /dev/loop-control -v /dev:/dev \\" >&2
	echo "           -v ${MONOREPO_ROOT}:/oxys:Z \\" >&2
	echo "           -v ${HOME}/catalyst:/var/tmp/catalyst:Z \\" >&2
	echo "           docker.io/gentoo/stage3:amd64-openrc bash" >&2
	exit 1
fi

# --- refresh installer when Rust tooling is available -----------------------
# The usual host wrapper (scripts/enter-container.sh build) always runs this
# before entering the catalyst container. Keep this best-effort path for manual
# runs from Rust-capable chroots/containers; a minimal catalyst image may not
# have cargo installed.
install -d -m 0755 "${OVERLAY_CONFIG_DIR}"
install -m 0644 "${INSTALLER_CONFIG_DIR}/base.rs" "${OVERLAY_CONFIG_DIR}/base.rs"
install -m 0644 "${INSTALLER_CONFIG_DIR}/desktop.rs" "${OVERLAY_CONFIG_DIR}/desktop.rs"
install -m 0644 "${INSTALLER_CONFIG_DIR}/custom.rs" "${OVERLAY_CONFIG_DIR}/custom.rs"
echo ">> refreshed ISO config overlay: ${OVERLAY_CONFIG_DIR}"

# Only rebuild if this environment's Rust can actually target musl. The host
# wrapper already built the binary before entering the catalyst container, whose
# system Rust has no rustup and no musl std -- attempting the build there just
# fails on "can't find crate for `std`". rustup counts as buildable because
# build-installer-overlay.sh will `rustup target add` the target itself; a
# rustup-less system Rust only qualifies if the musl std is already installed.
installer_target=x86_64-unknown-linux-musl
if ! command -v cargo >/dev/null 2>&1; then
	echo ">> cargo not found; using existing installer overlay binary"
elif command -v rustup >/dev/null 2>&1; then
	"${REPO_DIR}/scripts/build-installer-overlay.sh"
elif libdir="$(rustc --print target-libdir --target "${installer_target}" 2>/dev/null)" \
     && compgen -G "${libdir}/libstd-*.rlib" >/dev/null 2>&1; then
	"${REPO_DIR}/scripts/build-installer-overlay.sh"
else
	echo ">> Rust cannot target ${installer_target} here (no rustup, no musl std);" \
	     "using existing installer overlay binary"
fi

# --- sanity: installer binary present? --------------------------------------
# This is the exact binary catalyst bakes into the squashfs (stage2 root_overlay
# rsyncs ${REPO_DIR}/overlay verbatim). It was (re)built + copied here moments
# ago by build-installer-overlay.sh above -- print its identity so a stale bake
# is obvious in the build log without cracking open the ISO afterwards.
INSTALLER_BIN="${REPO_DIR}/overlay/usr/local/bin/oxys-installer"
if [[ ! -f "${INSTALLER_BIN}" ]]; then
	echo "ERROR: overlay/usr/local/bin/oxys-installer is missing." >&2
	echo "       Run scripts/enter-container.sh build from the host, or install Rust tooling in this environment." >&2
	exit 1
fi
echo ">> installer overlay binary to be baked in: sha256 $(sha256sum "${INSTALLER_BIN}" | cut -c1-16)… ($(stat -c%s "${INSTALLER_BIN}") bytes)"

# --- sanity: does a valid, paired kernel+zfs-kmod build exist for the
#     requested arch/build-id? Fail fast here, before catalyst even starts,
#     rather than failing deep inside a stage2 run (or worse, silently
#     letting catalyst build its own kernel). ---------------------------------
if ! KERNEL_BUILD_VARS="$("${REPO_DIR}/scripts/resolve-kernel-build.sh")"; then
	echo "ERROR: no valid prebuilt kernel+zfs-kmod build found for OXYS_ARCH=${OXYS_ARCH}${OXYS_KERNEL_BUILD_ID:+ OXYS_KERNEL_BUILD_ID=${OXYS_KERNEL_BUILD_ID}}." >&2
	echo "       (see the resolve-kernel-build.sh error above). Run oxys-build for" >&2
	echo "       this arch first: ${MONOREPO_ROOT}/oxys-build/" >&2
	exit 1
fi
eval "${KERNEL_BUILD_VARS}"
echo ">> using prebuilt kernel ${OXYS_RESOLVED_KERNEL_RELEASE} (arch=${OXYS_RESOLVED_ARCH}, build_id=${OXYS_RESOLVED_BUILD_ID})"

# --- stage the prebuilt kernel+zfs-kmod into catalyst's DISTDIR -------------
# catalyst unconditionally bind-mounts DISTDIR into the build chroot before
# any controller action runs (verified from catalyst/base/stagebase.py --
# self.mount['distdir'], not gated by an options: flag like pkgcache/kerncache
# are). That's the only host->chroot bridge available before build_kernel()
# executes, so this is how catalyst-overrides/kmerge.sh gets the tarballs it
# needs. DISTDIR is wherever THIS container's Portage/catalyst.conf resolves
# it to -- NOT necessarily ${STOREDIR}/distfiles (a default catalyst.conf
# leaves "distdir" commented out, which falls through to Portage's own
# default, normally /var/cache/distfiles). Query portageq directly, the exact
# same way catalyst-overrides/kmerge.sh does on the chroot side, so both ends
# agree regardless of this container's actual config.
DISTDIR="${OXYS_DISTDIR:-$(portageq envvar DISTDIR)}"
[[ -n "${DISTDIR}" ]] || { echo "ERROR: 'portageq envvar DISTDIR' returned nothing usable; set OXYS_DISTDIR explicitly." >&2; exit 1; }
KERNEL_CACHE_DIR="${DISTDIR}/oxys-kernel-cache"
mkdir -p "${KERNEL_CACHE_DIR}"
rm -f "${KERNEL_CACHE_DIR}"/*.tar.gz "${KERNEL_CACHE_DIR}"/*.metadata "${KERNEL_CACHE_DIR}/manifest.env"
cp "${OXYS_KERNEL_TARBALL}" "${OXYS_KERNEL_METADATA}" "${KERNEL_CACHE_DIR}/"
cp "${OXYS_ZFS_KMOD_TARBALL}" "${OXYS_ZFS_KMOD_METADATA}" "${KERNEL_CACHE_DIR}/"
cat > "${KERNEL_CACHE_DIR}/manifest.env" <<EOF
OXYS_BUILD_ID=${OXYS_RESOLVED_BUILD_ID}
OXYS_ARCH=${OXYS_RESOLVED_ARCH}
OXYS_KERNEL_RELEASE=${OXYS_RESOLVED_KERNEL_RELEASE}
OXYS_KERNEL_TARBALL=$(basename "${OXYS_KERNEL_TARBALL}")
OXYS_KERNEL_METADATA=$(basename "${OXYS_KERNEL_METADATA}")
OXYS_ZFS_KMOD_TARBALL=$(basename "${OXYS_ZFS_KMOD_TARBALL}")
OXYS_ZFS_KMOD_METADATA=$(basename "${OXYS_ZFS_KMOD_METADATA}")
EOF
echo ">> staged kernel+zfs-kmod cache at ${KERNEL_CACHE_DIR}"

# --- zfs userland (zpool/zfs CLI) has no kernel-version coupling, so it
#     doesn't need to go through the kernel-cache/kmerge.sh contract above --
#     it rides the same livecd/root_overlay mechanism that already delivers
#     oxys-installer, unpacked fresh into a per-build scratch dir. ----------
ZFS_OVERLAY="${WORK}/zfs-overlay"
rm -rf "${ZFS_OVERLAY}"
mkdir -p "${ZFS_OVERLAY}"
# Pre-seed a skeleton matching the seed stage3's fully merged layout
# (/bin and /sbin -> usr/bin, /usr/sbin -> bin, /lib -> usr/lib, /lib64 ->
# usr/lib64) so the tarball's top-level bin/, sbin/, lib64/ paths (the
# oxys-build container uses the older split-sbin layout) all funnel into
# usr/bin, usr/lib64, etc. The symlinks are extraction plumbing ONLY and are
# removed again below: catalyst applies root_overlay with plain `rsync -a`,
# and rsync replaces a destination symlink with a real directory when the
# overlay ships one at the same path. Shipping a real usr/sbin here once
# clobbered the chroot's /usr/sbin -> bin symlink, leaving a usr/sbin that
# held only zfs tools and no init => "cannot find init" at boot. The overlay
# must contain real payload dirs only, no layout opinions.
mkdir -p "${ZFS_OVERLAY}"/usr/{bin,lib,lib64}
ln -s usr/bin "${ZFS_OVERLAY}/bin"
ln -s usr/bin "${ZFS_OVERLAY}/sbin"
ln -s usr/lib "${ZFS_OVERLAY}/lib"
ln -s usr/lib64 "${ZFS_OVERLAY}/lib64"
ln -s bin "${ZFS_OVERLAY}/usr/sbin"
tar -C "${ZFS_OVERLAY}" --keep-directory-symlink -xzf "${OXYS_ZFS_USERLAND_TARBALL}"
# Plain rm (not -f): if any of these is unexpectedly a real dir, the layout
# assumption above broke and the build must fail here, not at boot.
rm "${ZFS_OVERLAY}"/{bin,sbin,lib,lib64} "${ZFS_OVERLAY}/usr/sbin"
echo ">> staged zfs userland overlay at ${ZFS_OVERLAY}"

# --- render specs from templates --------------------------------------------
mkdir -p "${WORK}"
render() {
	local in="$1" out="$2"
	sed -e "s|@TIMESTAMP@|${TIMESTAMP}|g" \
	    -e "s|@DATESTAMP@|${DATESTAMP}|g" \
	    -e "s|@TREEISH@|${TREEISH}|g" \
	    -e "s|@REPO_DIR@|${REPO_DIR}|g" \
	    -e "s|@ZFS_OVERLAY@|${ZFS_OVERLAY}|g" \
	    "${in}" > "${out}"
}
render "${SPECS_SRC}/installcd-stage1.spec" "${WORK}/stage1.spec"
render "${SPECS_SRC}/installcd-stage2.spec" "${WORK}/stage2.spec"

echo ">> OxysOS build ${TIMESTAMP} (treeish=${TREEISH})"

# --- seed: ensure the stage3-openrc seed tarball is present -----------------
# stage1's source_subpath is 23.0-default/stage3-amd64-openrc-seed, so catalyst
# looks for <storedir>/builds/23.0-default/stage3-amd64-openrc-seed.tar.xz.
# Download the current upstream stage3-openrc and drop it there if missing.
SEED="${BUILDS_DIR}/stage3-amd64-openrc-seed.tar.xz"
if [[ ! -f "${SEED}" ]]; then
	echo ">> seed missing; fetching current stage3-amd64-openrc"
	BASE="https://distfiles.gentoo.org/releases/amd64/autobuilds"
	# The pointer file is PGP-signed, so it is wrapped in
	# -----BEGIN PGP SIGNED MESSAGE----- / signature blocks and has comment
	# lines. Match only the payload line (the one naming the stage3 tarball)
	# and take its first field (the relative path).
	REL="$(wget -qO- "${BASE}/latest-stage3-amd64-openrc.txt" \
		| awk '/stage3-amd64-openrc.*\.tar\.xz/{print $1; exit}')"
	if [[ -z "${REL}" ]]; then
		echo "ERROR: could not resolve latest stage3-amd64-openrc from mirror." >&2
		exit 1
	fi
	mkdir -p "$(dirname "${SEED}")"
	# download to a temp file; rename only on success so an aborted fetch
	# doesn't leave a truncated seed that the next run would silently reuse.
	wget -O "${SEED}.part" "${BASE}/${REL}"
	mv "${SEED}.part" "${SEED}"
else
	echo ">> seed present: ${SEED}"
fi

# --- (optional) create the snapshot if missing ------------------------------
# Comment out if you manage snapshots separately. `-s` form may differ by
# catalyst version — VERIFY with `catalyst --help`.
# catalyst -s "${TREEISH}"

# --- clean slate: drop previously built ISOs --------------------------------
# scripts/run-qemu.sh boots the NEWEST *.iso by mtime, so a stale image left
# next to a fresh one is a boot-the-wrong-thing footgun. Removing them here
# (after all cheap preflight passed, before the expensive stages) also means a
# failed build leaves NO iso -- a clear signal -- instead of silently falling
# back to the previous one. Only our own oxysos-*.iso are touched.
if compgen -G "${BUILDS_DIR}/oxysos-*.iso" >/dev/null 2>&1; then
	echo ">> removing previous ISOs under ${BUILDS_DIR}"
	rm -f "${BUILDS_DIR}"/oxysos-*.iso \
	      "${BUILDS_DIR}"/oxysos-*.iso.CONTENTS.gz \
	      "${BUILDS_DIR}"/oxysos-*.iso.DIGESTS \
	      "${BUILDS_DIR}"/oxysos-*.iso.sha256
fi

# --- stage 1: build the live package set ------------------------------------
echo ">> livecd-stage1"
catalyst -af "${WORK}/stage1.spec"

# --- stage 2: kernel + initramfs + overlay + squashfs + ISO -----------------
echo ">> livecd-stage2"
catalyst -af "${WORK}/stage2.spec"

BUILT_ISO="${BUILDS_DIR}/oxysos-amd64-${TIMESTAMP}.iso"
if [[ -f "${BUILT_ISO}" ]]; then
	echo ">> Done. ISO: ${BUILT_ISO} ($(stat -c%s "${BUILT_ISO}") bytes)"
	echo "   (this is now the only oxysos-*.iso here, so run-qemu.sh will boot it)"
else
	echo "ERROR: build finished but expected ISO is missing: ${BUILT_ISO}" >&2
	exit 1
fi
