#!/usr/bin/env bash
# Build the installer and refresh the ISO root overlay payload.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_DIR="$(cd "${SCRIPT_DIR}/.." && pwd)"
MONOREPO_ROOT="$(cd "${REPO_DIR}/.." && pwd)"
INSTALLER_DIR="${MONOREPO_ROOT}/oxys-installer"
TARGET="x86_64-unknown-linux-musl"
# oxys-installer is a workspace member, so cargo writes its artifacts to the
# WORKSPACE target dir at the monorepo root -- NOT oxys-installer/target/. That
# per-crate path is a stale leftover from a pre-workspace standalone build;
# copying from it silently ships an old installer in every ISO. Honor
# CARGO_TARGET_DIR if the caller overrides the target dir.
CARGO_TARGET_ROOT="${CARGO_TARGET_DIR:-${MONOREPO_ROOT}/target}"
BIN="${CARGO_TARGET_ROOT}/${TARGET}/release/oxys-installer"
OVERLAY_BIN="${REPO_DIR}/overlay/usr/local/bin/oxys-installer"
OVERLAY_CONFIG_DIR="${REPO_DIR}/overlay/root/configs"

if ! command -v cargo >/dev/null 2>&1; then
	echo "ERROR: cargo is required to build oxys-installer." >&2
	exit 1
fi

if command -v rustup >/dev/null 2>&1; then
	rustup target add "${TARGET}" >/dev/null
fi

echo ">> building oxys-installer (${TARGET})"
cargo build \
	--manifest-path "${INSTALLER_DIR}/Cargo.toml" \
	--release \
	--target "${TARGET}"

install -D -m 0755 "${BIN}" "${OVERLAY_BIN}"
install -d -m 0755 "${OVERLAY_CONFIG_DIR}"
install -m 0644 "${INSTALLER_DIR}/configs/base.fe2o3" "${OVERLAY_CONFIG_DIR}/base.fe2o3"
install -m 0644 "${INSTALLER_DIR}/configs/desktop.fe2o3" "${OVERLAY_CONFIG_DIR}/desktop.fe2o3"
install -m 0644 "${INSTALLER_DIR}/configs/custom.fe2o3" "${OVERLAY_CONFIG_DIR}/custom.fe2o3"
echo ">> refreshed ISO installer overlay: ${OVERLAY_BIN}"
echo ">> refreshed ISO config overlay: ${OVERLAY_CONFIG_DIR}"

# --- on-target config compiler payload -------------------------------------
# The installer compiles the user's Rust config into manifest.toml *on the live
# system* (oxys::compile). That needs the oxys crate source, its dependencies
# vendored for offline builds, and a cargo config that redirects crates.io to
# the vendor dir. Everything lands under /usr/src/oxys + /root/.cargo so the
# defaults in oxys::compile (OXYS crate at /usr/src/oxys, HOME=/root) just work.
OXYS_CRATE_SRC="${MONOREPO_ROOT}/oxys"
OVERLAY_OXYS_SRC="${REPO_DIR}/overlay/usr/src/oxys"
OVERLAY_CARGO_CFG="${REPO_DIR}/overlay/root/.cargo"

echo ">> staging oxys crate source for on-target compile"
rm -rf "${OVERLAY_OXYS_SRC}"
mkdir -p "${OVERLAY_OXYS_SRC}/src"
cp -a "${OXYS_CRATE_SRC}/src/." "${OVERLAY_OXYS_SRC}/src/"

# oxys/Cargo.toml already pins its own dependency versions literally (no
# `{ workspace = true }`) specifically so it can be copied as-is here. The only
# thing it can't carry over is workspace membership: appending an empty
# [workspace] table marks this staged copy as its own workspace root, so cargo
# doesn't walk upward looking for the real monorepo workspace.
cp "${OXYS_CRATE_SRC}/Cargo.toml" "${OVERLAY_OXYS_SRC}/Cargo.toml"
printf '\n[workspace]\n' >> "${OVERLAY_OXYS_SRC}/Cargo.toml"

echo ">> vendoring oxys dependencies for offline builds"
( cd "${OVERLAY_OXYS_SRC}" && cargo generate-lockfile && cargo vendor --versioned-dirs vendor >/dev/null )

mkdir -p "${OVERLAY_CARGO_CFG}"
cat > "${OVERLAY_CARGO_CFG}/config.toml" <<'CARGOCFG'
# OxysOS: compile the user's config against vendored crates, fully offline.
# A global (per-HOME) config so it also applies to the scaffold crate the
# installer builds under ~/.cache/oxys/build, which lives outside /usr/src/oxys.
[source.crates-io]
replace-with = "vendored-sources"

[source.vendored-sources]
directory = "/usr/src/oxys/vendor"

[net]
offline = true
CARGOCFG

echo ">> staged /usr/src/oxys (+vendor) and /root/.cargo/config.toml"

# --- oxys-login: graphical console login (PAM auth -> niri --session) --------
# Prebuilt here and dropped into the overlay exactly like the installer, so the
# ISO ships a ready-to-run binary (the installer rsyncs it onto the target where
# SetupLogin points tty1 at /usr/local/bin/oxys-login). Unlike the installer it
# is a glibc DYNAMIC binary, not static musl: PAM requires dynamic linking (it
# dlopen's pam_unix.so et al.), so a static build cannot authenticate.
#
# CAVEAT: this links the BUILD HOST's glibc/libpam. If the host glibc is newer
# than the installed system's, the binary can fail to start there ("version
# `GLIBC_2.xx' not found"). If that happens, build oxys-login in an environment
# matching the target glibc (e.g. inside the catalyst container) instead.
LOGIN_BIN="${CARGO_TARGET_ROOT}/release/oxys-login"
OVERLAY_LOGIN_BIN="${REPO_DIR}/overlay/usr/local/bin/oxys-login"

echo ">> building oxys-login (glibc dynamic, links libpam)"
cargo build --manifest-path "${MONOREPO_ROOT}/oxys-login/Cargo.toml" --release
install -D -m 0755 "${LOGIN_BIN}" "${OVERLAY_LOGIN_BIN}"
echo ">> refreshed ISO oxys-login overlay: ${OVERLAY_LOGIN_BIN}"

# --- GURU overlay: niri + small Wayland tools -------------------------------
# Cloned into the ISO root overlay so it serves two purposes from one checkout:
#   1. catalyst mounts it as a build repo (specs `repos:`) to BAKE niri et al.;
#   2. the root overlay rsyncs it to the target so on-target emerge can build
#      GURU packages a user later adds.
# Shallow + no .git to keep it smaller; refreshed each build. Gitignored.
OVERLAY_GURU_DIR="${REPO_DIR}/overlay/var/db/repos/guru"
if [ ! -d "${OVERLAY_GURU_DIR}/profiles" ]; then
	echo ">> cloning GURU overlay into ${OVERLAY_GURU_DIR}"
	rm -rf "${OVERLAY_GURU_DIR}"
	git clone --depth=1 https://github.com/gentoo/guru.git "${OVERLAY_GURU_DIR}"
	rm -rf "${OVERLAY_GURU_DIR}/.git"
else
	echo ">> GURU overlay already present at ${OVERLAY_GURU_DIR} (delete to refresh)"
fi
