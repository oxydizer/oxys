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

# Fail-fast self syntax-check. bash parses a directly-invoked script lazily
# (command by command), so a syntax error further down doesn't surface until
# execution reaches it -- which once meant a broken line 373 only erroring
# ~30min into livecd-stage1. Re-parse the whole file up front so any such error
# aborts in ~1s here instead. (`$0` is this script; `|| exit` keeps the message.)
bash -n "${BASH_SOURCE[0]}" || { echo "build.sh: syntax check failed, aborting" >&2; exit 2; }

# The monorepo is bind-mounted into this rootful container with host ownership
# (uid 1000), but build.sh runs as root (uid 0). Git 2.35.2+ refuses to operate
# on the prefetched bare repos under .build/source-cache/git3-src/ because the
# owner differs from the caller ("detected dubious ownership"). Trust every path
# system-wide so it applies regardless of HOME and covers all preseeded repos.
git config --system --add safe.directory '*'

REPO_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
WORK="${REPO_DIR}/.build"
SPECS_SRC="${REPO_DIR}/specs"
GIT_SOURCES_FILE="${REPO_DIR}/git-sources.conf"

# Monorepo root (oxys-iso's parent dir). oxys-build's kernel/zfs-kmod
# output lives at ${MONOREPO_ROOT}/oxys-build/output/<arch>/ — see
# scripts/resolve-kernel-build.sh.
MONOREPO_ROOT="$(cd "${REPO_DIR}/.." && pwd)"
INSTALLER_CONFIG_DIR="${MONOREPO_ROOT}/oxys-installer/configs"
OVERLAY_CONFIG_DIR="${REPO_DIR}/overlay/root/configs"

if [[ -n "${OXYS_GRAPHICS_MANIFEST:-}" ]]; then
	if [[ -n "${OXYS_VIDEO_CARDS:-}" || -n "${OXYS_DRM_DRIVERS:-}" ]]; then
		echo "ERROR: OXYS_GRAPHICS_MANIFEST cannot be combined with explicit OXYS_VIDEO_CARDS/OXYS_DRM_DRIVERS." >&2
		exit 1
	fi
	oxys_bin="${OXYS_BIN:-${MONOREPO_ROOT}/target/debug/oxys}"
	if [[ ! -x "${oxys_bin}" ]] && command -v cargo >/dev/null 2>&1; then
		cargo build --manifest-path "${MONOREPO_ROOT}/Cargo.toml" -p oxys --bin oxys
	fi
	if [[ ! -x "${oxys_bin}" ]]; then
		echo "ERROR: cannot derive graphics policy; Oxys CLI is missing at ${oxys_bin}." >&2
		exit 1
	fi
	policy="$("${oxys_bin}" graphics-build-policy "${OXYS_GRAPHICS_MANIFEST}")"
	eval "${policy}"
	export OXYS_VIDEO_CARDS OXYS_DRM_DRIVERS
	echo ">> resolved graphics build policy from ${OXYS_GRAPHICS_MANIFEST}"
fi

VIDEO_CARDS_POLICY="${OXYS_VIDEO_CARDS:-intel radeon radeonsi amdgpu virgl}"

normalize_video_cards() {
	local raw="${1//,/ }" card known existing
	local -a allowed=(intel amdgpu radeon radeonsi nouveau virgl vmware lavapipe)
	local -a resolved=()
	for card in ${raw}; do
		known=0
		for existing in "${allowed[@]}"; do
			[[ "${card}" == "${existing}" ]] && known=1
		done
		if (( known == 0 )); then
			echo "ERROR: unsupported OXYS_VIDEO_CARDS value '${card}' (allowed: ${allowed[*]})." >&2
			exit 1
		fi
		if [[ " ${resolved[*]} " != *" ${card} "* ]]; then
			resolved+=("${card}")
		fi
	done
	if (( ${#resolved[@]} == 0 )); then
		echo "ERROR: OXYS_VIDEO_CARDS must contain at least one value." >&2
		exit 1
	fi
	printf '%s\n' "${resolved[*]}"
}

VIDEO_CARDS_POLICY="$(normalize_video_cards "${VIDEO_CARDS_POLICY}")"

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
# this project is otherwise trying to avoid (see doc.md). The artifact set for
# that arch is published atomically by oxys-build; see resolve-kernel-build.sh.
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
install -m 0644 "${INSTALLER_CONFIG_DIR}/base.fe2o3" "${OVERLAY_CONFIG_DIR}/base.fe2o3"
install -m 0644 "${INSTALLER_CONFIG_DIR}/desktop.fe2o3" "${OVERLAY_CONFIG_DIR}/desktop.fe2o3"
install -m 0644 "${INSTALLER_CONFIG_DIR}/custom.fe2o3" "${OVERLAY_CONFIG_DIR}/custom.fe2o3"
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

# --- sanity: Portage-owned oxys CLI package present? ------------------------
# The host wrapper stages a static CLI in the canonical app-admin/oxys ebuild
# and mirrors that package into the ISO before catalyst starts. Stage1 emerges
# it into /usr/bin, making the updater VDB-owned instead of leaving a raw binary
# in /usr/local/bin where it would shadow future package updates.
OXYS_PACKAGE_VERSION="$(awk '
	$0 == "[package]" { package = 1; next }
	package && /^\[/ { exit }
	package && /^version[[:space:]]*=/ {
		value = $0
		sub(/^[^=]*=[[:space:]]*"/, "", value)
		sub(/"[[:space:]]*$/, "", value)
		print value
		exit
	}
' "${MONOREPO_ROOT}/oxys/Cargo.toml")"
if [[ -z ${OXYS_PACKAGE_VERSION} ]]; then
	echo "ERROR: could not read the Oxys package version from oxys/Cargo.toml." >&2
	exit 1
fi
OXYS_PACKAGE_DIR="${MONOREPO_ROOT}/oxys-build/oxys-overlay/app-admin/oxys"
OXYS_PACKAGE_PAYLOAD="${OXYS_PACKAGE_DIR}/files/oxys-${OXYS_PACKAGE_VERSION}"
OXYS_PACKAGE_EBUILD="${OXYS_PACKAGE_DIR}/oxys-${OXYS_PACKAGE_VERSION}.ebuild"
OXYS_ISO_PACKAGE_DIR="${REPO_DIR}/overlay/var/db/repos/oxys/app-admin/oxys"

for required in "${OXYS_PACKAGE_PAYLOAD}" "${OXYS_PACKAGE_EBUILD}" "${OXYS_PACKAGE_DIR}/Manifest"; do
	if [[ ! -f "${required}" ]]; then
		echo "ERROR: staged app-admin/oxys input is missing: ${required}" >&2
		echo "       Run scripts/enter-container.sh build from the Rust-capable host." >&2
		exit 1
	fi
done
shopt -s nullglob
oxys_package_ebuilds=("${OXYS_PACKAGE_DIR}"/oxys-*.ebuild)
oxys_package_payloads=("${OXYS_PACKAGE_DIR}"/files/oxys-*)
shopt -u nullglob
if (( ${#oxys_package_ebuilds[@]} != 1 )) || \
   [[ ${oxys_package_ebuilds[0]:-} != "${OXYS_PACKAGE_EBUILD}" ]]; then
	echo "ERROR: app-admin/oxys must contain exactly one ebuild for ${OXYS_PACKAGE_VERSION}." >&2
	exit 1
fi
if (( ${#oxys_package_payloads[@]} != 1 )) || \
   [[ ${oxys_package_payloads[0]:-} != "${OXYS_PACKAGE_PAYLOAD}" ]]; then
	echo "ERROR: app-admin/oxys must contain exactly one staged payload for ${OXYS_PACKAGE_VERSION}." >&2
	exit 1
fi
if [[ -e "${REPO_DIR}/overlay/usr/local/bin/oxys" ]]; then
	echo "ERROR: obsolete overlay/usr/local/bin/oxys would shadow /usr/bin/oxys." >&2
	echo "       Remove it and rerun scripts/build-installer-overlay.sh." >&2
	exit 1
fi
if ! diff -qr "${OXYS_PACKAGE_DIR}" "${OXYS_ISO_PACKAGE_DIR}" >/dev/null; then
	echo "ERROR: ISO app-admin/oxys package differs from its canonical build-overlay copy." >&2
	echo "       Rerun scripts/build-installer-overlay.sh to synchronize it." >&2
	exit 1
fi
if [[ "$("${OXYS_PACKAGE_PAYLOAD}" --version)" != "oxys ${OXYS_PACKAGE_VERSION}" ]]; then
	echo "ERROR: staged app-admin/oxys payload version does not match its ebuild." >&2
	exit 1
fi
echo ">> app-admin/oxys-${OXYS_PACKAGE_VERSION} payload: sha256 $(sha256sum "${OXYS_PACKAGE_PAYLOAD}" | cut -c1-16)… ($(stat -c%s "${OXYS_PACKAGE_PAYLOAD}") bytes)"

# --- sanity: does a valid, paired kernel+zfs-kmod build exist for the
#     requested arch? Fail fast here, before catalyst even starts,
#     rather than failing deep inside a stage2 run (or worse, silently
#     letting catalyst build its own kernel). ---------------------------------
if ! KERNEL_BUILD_VARS="$("${REPO_DIR}/scripts/resolve-kernel-build.sh")"; then
	echo "ERROR: no valid prebuilt kernel+zfs-kmod build found for OXYS_ARCH=${OXYS_ARCH}." >&2
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

# Build an isolated Portage config for this catalyst run. The static config is
# copied verbatim, then package.env entries for every prefetched live Git source
# are generated from the same manifest used by the host prefetcher.
GENERATED_PORTAGE_CONFDIR="${WORK}/portage_confdir"
GIT_OFFLINE_ENV="prefetched-git-source.conf"
mkdir -p "${WORK}"
rm -rf "${GENERATED_PORTAGE_CONFDIR}"
cp -a "${REPO_DIR}/portage_confdir" "${GENERATED_PORTAGE_CONFDIR}"
sed -i -E "s|^VIDEO_CARDS=.*$|VIDEO_CARDS=\"${VIDEO_CARDS_POLICY}\"|" \
	"${GENERATED_PORTAGE_CONFDIR}/make.conf"
echo ">> graphics build policy: VIDEO_CARDS='${VIDEO_CARDS_POLICY}'${OXYS_DRM_DRIVERS:+, required kernel DRM='${OXYS_DRM_DRIVERS}'}"
mkdir -p "${GENERATED_PORTAGE_CONFDIR}/package.env"
GENERATED_GIT_PACKAGE_ENV="${GENERATED_PORTAGE_CONFDIR}/package.env/prefetched-git-sources"
: > "${GENERATED_GIT_PACKAGE_ENV}"
if [[ ! -f "${GENERATED_PORTAGE_CONFDIR}/env/${GIT_OFFLINE_ENV}" ]]; then
	echo "ERROR: missing Portage environment file: env/${GIT_OFFLINE_ENV}" >&2
	exit 1
fi

declare -A offline_git_atoms=()
git_source_count=0
if ! git_source_rows="$("${REPO_DIR}/scripts/prefetch-git-sources.sh" \
	--list "${GIT_SOURCES_FILE}")"; then
	echo "ERROR: invalid Git source manifest: ${GIT_SOURCES_FILE}" >&2
	exit 1
fi

while IFS=$'\t' read -r package_atom store_name _source_uri _source_ref; do
	cache_repo="${WORK}/source-cache/git3-src/${store_name}"
	git3_repo="${DISTDIR}/git3-src/${store_name}"
	if ! git --git-dir="${cache_repo}" \
		cat-file -e 'refs/heads/oxys-source^{commit}' 2>/dev/null; then
		echo "ERROR: prefetched Git source is missing or incomplete for ${package_atom}:" >&2
		echo "       ${cache_repo}" >&2
		echo "       Run scripts/enter-container.sh build from the host; it prefetches" >&2
		echo "       every entry in ${GIT_SOURCES_FILE}." >&2
		exit 1
	fi

	mkdir -p "$(dirname "${git3_repo}")"
	rm -rf "${git3_repo}"
	git clone --bare --no-hardlinks "${cache_repo}" "${git3_repo}" >/dev/null
	# git-r3 runs src_unpack as Portage's build user. The preseeded clone must
	# be writable so it can create package-local refs before checking out source.
	chown -R portage:portage "${git3_repo}"
	commit="$(git --git-dir="${git3_repo}" rev-parse 'refs/heads/oxys-source^{commit}')"
	echo ">> staged offline Git source for ${package_atom} at ${git3_repo} (${commit:0:12})"

	if [[ -z "${offline_git_atoms[$package_atom]:-}" ]]; then
		printf '%s %s\n' "${package_atom}" "${GIT_OFFLINE_ENV}" >> "${GENERATED_GIT_PACKAGE_ENV}"
		offline_git_atoms["${package_atom}"]=1
	fi
	((git_source_count += 1))
done <<< "${git_source_rows}"

if (( git_source_count == 0 )); then
	echo "ERROR: no Git sources configured in ${GIT_SOURCES_FILE}." >&2
	exit 1
fi
echo ">> staged ${git_source_count} offline Git source(s) for catalyst"

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

# --- stage a root-owned copy of the committed overlay -----------------------
# overlay/ lives in the git checkout owned by the invoking user (uid 1000), but
# catalyst applies livecd/root_overlay with plain `rsync -a`, which preserves
# source ownership. Shipping overlay/ directly therefore stamps uid 1000 onto
# every path it carries -- including /var, since the overlay ships
# var/db/repos/guru. On the live ISO that leaves /var owned by uid 1000 while
# /var/tmp (from the stage3, not in the overlay) stays root:root. The installer
# then rsyncs the live root to the target with --numeric-ids, so the target
# inherits the same split and the first real user (uid 1000) appears to own
# /var -- tripping systemd's "unsafe path transition /var -> /var/tmp" check.
# Copy to a scratch dir and normalise to root:root; build.sh runs as root inside
# the catalyst container, so the baked overlay then carries correct ownership.
OVERLAY_DIR="${WORK}/root-overlay"
rm -rf "${OVERLAY_DIR}"
cp -a "${REPO_DIR}/overlay" "${OVERLAY_DIR}"

# Ship the *actual* graphics build policy into the image so fsscript can verify
# the built Mesa against what we asked for. It cannot use `portageq envvar
# VIDEO_CARDS` for this: our VIDEO_CARDS policy lives only in the build-time
# portage_confdir, which catalyst discards -- inside the finished chroot portageq
# returns the seed stage3's *profile-default* VIDEO_CARDS (fbdev/vesa/dummy/...),
# which Mesa was deliberately not built for, tripping a false FATAL. This file is
# the single source of truth both sides agree on.
GRAPHICS_POLICY_DIR="${OVERLAY_DIR}/usr/share/oxys"
mkdir -p "${GRAPHICS_POLICY_DIR}"
cat > "${GRAPHICS_POLICY_DIR}/graphics-build-policy.env" <<EOF_POLICY
# Generated by oxys-iso/build.sh -- the graphics policy this image was built for.
VIDEO_CARDS="${VIDEO_CARDS_POLICY}"
DRM_DRIVERS="${OXYS_DRM_DRIVERS:-}"
EOF_POLICY

chown -R 0:0 "${OVERLAY_DIR}"
echo ">> staged root-owned overlay at ${OVERLAY_DIR}"

# --- render specs from templates --------------------------------------------
mkdir -p "${WORK}"
render() {
	local in="$1" out="$2"
	sed -e "s|@TIMESTAMP@|${TIMESTAMP}|g" \
	    -e "s|@DATESTAMP@|${DATESTAMP}|g" \
	    -e "s|@TREEISH@|${TREEISH}|g" \
	    -e "s|@REPO_DIR@|${REPO_DIR}|g" \
	    -e "s|@OVERLAY_DIR@|${OVERLAY_DIR}|g" \
	    -e "s|@PORTAGE_CONFDIR@|${GENERATED_PORTAGE_CONFDIR}|g" \
	    -e "s|@ZFS_OVERLAY@|${ZFS_OVERLAY}|g" \
	    "${in}" > "${out}"
}
render "${SPECS_SRC}/installcd-stage1.spec" "${WORK}/stage1.spec"
render "${SPECS_SRC}/installcd-stage2.spec" "${WORK}/stage2.spec"

# --- fail-fast overrides (iteration only) -----------------------------------
# OXYS_STAGE1_PACKAGES="cat/pkg ..."  Replace the whole livecd/packages list
#   with just these atoms, so a single package (plus the deps Portage pulls for
#   it) builds instead of the full ~50-package live set. Lets a live/git-sourced
#   package like gui-shells/noctalia surface build errors in minutes rather than
#   after everything else emerges first. Combine with OXYS_STAGE1_ONLY=1.
# OXYS_STAGE1_ONLY=1  Stop after livecd-stage1; skip the kernel/squashfs/ISO
#   stage2. The result is not a bootable ISO -- it's a compile smoke-test.
if [[ -n "${OXYS_STAGE1_PACKAGES:-}" ]]; then
	# Rewrite the livecd/packages: block in the rendered spec. The value block
	# is the key line plus every following whitespace-indented (atom/comment)
	# line; it ends at the next column-0 line (the next spec key or comment).
	awk -v pkgs="${OXYS_STAGE1_PACKAGES}" '
		/^livecd\/packages:/ {
			print
			n = split(pkgs, a, /[[:space:],]+/)
			for (i = 1; i <= n; i++) if (a[i] != "") printf "\t%s\n", a[i]
			skip = 1
			next
		}
		skip && /^[ \t]/ { next }        # continuation line: drop
		skip && /^[ \t]*$/ { next }      # blank line inside block: drop
		{ skip = 0; print }
	' "${WORK}/stage1.spec" > "${WORK}/stage1.spec.tmp"
	mv "${WORK}/stage1.spec.tmp" "${WORK}/stage1.spec"
	echo ">> OXYS_STAGE1_PACKAGES set: stage1 will build only: ${OXYS_STAGE1_PACKAGES}"
fi

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

# --- prune accumulated per-run scratch (disk hygiene) -----------------------
# catalyst never cleans up after itself: every run leaves behind a full chroot
# under tmp/<rel>/, a leftover squashfs staging dir + a stage tarball under
# builds/<rel>/, and an .autoresume-* marker -- each 1-6 GB. Because build.sh
# stamps a fresh TIMESTAMP every run, NONE of these timestamped artifacts is
# ever reused; left alone they had grown to >150 GB here. The genuine cross-run
# caches -- packages/ (binpkgs), distfiles/, kerncache/, snapshots/ -- live in
# sibling directories and are deliberately NOT touched, so this doesn't force a
# from-scratch rebuild. We keep only the single newest livecd-stage1 tarball (so
# stage2 can be hand-re-run against it) and the ISO we're about to replace.
# Runs before catalyst starts, so the current run's own dirs don't exist yet.
# Set OXYS_NO_PRUNE=1 to skip.
if [[ "${OXYS_NO_PRUNE:-}" != "1" ]]; then
	before_kb="$(df -Pk "${STOREDIR}" | awk 'NR==2{print $4}')"
	tmpd="${STOREDIR}/tmp/23.0-default"
	# Pure scratch: per-run chroots + their autoresume markers. None survives.
	rm -rf "${tmpd}"/*/ "${tmpd}"/.autoresume-* 2>/dev/null || true
	# Leftover per-run squashfs staging dirs (image.squashfs is already in the ISO).
	rm -rf "${BUILDS_DIR}"/livecd-stage2-amd64-*/ 2>/dev/null || true
	# Stage tarballs: keep the newest livecd-stage1 tarball, drop the rest and
	# every stage2 tarball (its final output is captured as the ISO).
	keep_stage1="$(ls -1t "${BUILDS_DIR}"/livecd-stage1-amd64-*.tar.xz 2>/dev/null | head -1 || true)"
	for f in "${BUILDS_DIR}"/livecd-stage1-amd64-*.tar.xz "${BUILDS_DIR}"/livecd-stage2-amd64-*.tar.bz2; do
		[[ -e "${f}" ]] || continue                 # unmatched glob -> skip
		[[ "${f}" == "${keep_stage1}" ]] && continue
		rm -f "${f}" "${f}.CONTENTS.gz" "${f}.DIGESTS"
	done
	after_kb="$(df -Pk "${STOREDIR}" | awk 'NR==2{print $4}')"
	echo ">> pruned catalyst scratch; reclaimed ~$(awk -v a="${before_kb}" -v b="${after_kb}" \
		'BEGIN{printf "%.1f", (b-a)/1048576}') GB (free now: $(df -Ph "${STOREDIR}" | awk 'NR==2{print $4}'), set OXYS_NO_PRUNE=1 to skip)"
fi

# --- stage 1: build the live package set ------------------------------------
echo ">> livecd-stage1"
catalyst -af "${WORK}/stage1.spec"

# --- stage 2: kernel + initramfs + overlay + squashfs + ISO -----------------
if [[ "${OXYS_STAGE1_ONLY:-}" == "1" ]]; then
	echo ">> OXYS_STAGE1_ONLY=1: stopping after livecd-stage1 (no ISO produced)."
	exit 0
fi
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
