#!/usr/bin/env bash
# Build the versioned Oxys CLI payload consumed by app-admin/oxys, write its
# Manifest hashes, and publish the canonical first-party overlay into the ISO.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
OXYS_BUILD_DIR="$(cd "${SCRIPT_DIR}/.." && pwd)"
MONOREPO_ROOT="$(cd "${OXYS_BUILD_DIR}/.." && pwd)"
CANONICAL_OVERLAY="${OXYS_BUILD_DIR}/oxys-overlay"
ISO_OVERLAY="${MONOREPO_ROOT}/oxys-iso/overlay/var/db/repos/oxys"
ISO_PACKAGE_DIR="${ISO_OVERLAY}/app-admin/oxys"
ISO_CATEGORIES="${ISO_OVERLAY}/profiles/categories"
LEGACY_ISO_PAYLOAD="${MONOREPO_ROOT}/oxys-iso/overlay/usr/local/bin/oxys"
TARGET="x86_64-unknown-linux-musl"
CPU_BASELINE="${OXYS_CPU_BASELINE:-x86-64-v3}"

usage() {
	cat <<EOF
Usage: ${0##*/} [--cpu-baseline BASELINE]

Build and stage app-admin/oxys, then synchronize the canonical Oxys overlay
into the ISO. BASELINE defaults to x86-64-v3 and may be x86-64, x86-64-v2,
x86-64-v3, or x86-64-v4.
EOF
}

while (( $# > 0 )); do
	case "$1" in
		--cpu-baseline)
			(( $# >= 2 )) || { echo "ERROR: --cpu-baseline requires a value" >&2; exit 2; }
			CPU_BASELINE="$2"
			shift 2
			;;
		-h|--help)
			usage
			exit 0
			;;
		*)
			echo "ERROR: unknown argument: $1" >&2
			usage >&2
			exit 2
			;;
	esac
done

case "${CPU_BASELINE}" in
	x86-64|x86-64-v2|x86-64-v3|x86-64-v4) ;;
	*)
		echo "ERROR: unsupported CPU baseline '${CPU_BASELINE}'" >&2
		exit 2
		;;
esac

for command in awk b2sum cargo cp diff file find grep install mkdir mktemp mv rm sha512sum stat; do
	command -v "${command}" >/dev/null 2>&1 || {
		echo "ERROR: required command not found: ${command}" >&2
		exit 1
	}
done

VERSION="$(awk '
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
[[ -n ${VERSION} ]] || { echo "ERROR: could not read the Oxys package version" >&2; exit 1; }

PACKAGE_DIR="${CANONICAL_OVERLAY}/app-admin/oxys"
EBUILD="${PACKAGE_DIR}/oxys-${VERSION}.ebuild"
PAYLOAD="${PACKAGE_DIR}/files/oxys-${VERSION}"
CARGO_TARGET_ROOT="${CARGO_TARGET_DIR:-${MONOREPO_ROOT}/target}"
BUILT_BINARY="${CARGO_TARGET_ROOT}/${TARGET}/release/oxys"
BUILD_RUSTFLAGS="${RUSTFLAGS:+${RUSTFLAGS} }-C target-cpu=${CPU_BASELINE}"

[[ -f ${EBUILD} ]] || {
	echo "ERROR: version ${VERSION} has no matching ebuild at ${EBUILD}" >&2
	exit 1
}
shopt -s nullglob
versioned_ebuilds=("${PACKAGE_DIR}"/oxys-*.ebuild)
shopt -u nullglob
if (( ${#versioned_ebuilds[@]} != 1 )) || [[ ${versioned_ebuilds[0]:-} != "${EBUILD}" ]]; then
	echo "ERROR: app-admin/oxys uses a single-version release policy; expected only ${EBUILD}" >&2
	exit 1
fi

if command -v rustup >/dev/null 2>&1; then
	rustup target add "${TARGET}" >/dev/null
fi

echo ">> building app-admin/oxys-${VERSION} (${TARGET}, cpu=${CPU_BASELINE})"
RUSTFLAGS="${BUILD_RUSTFLAGS}" cargo build \
	--manifest-path "${MONOREPO_ROOT}/oxys/Cargo.toml" \
	--release \
	--locked \
	--target "${TARGET}" \
	--bin oxys

[[ -x ${BUILT_BINARY} ]] || { echo "ERROR: Oxys CLI build produced no ${BUILT_BINARY}" >&2; exit 1; }
version_output="$("${BUILT_BINARY}" --version)"
[[ ${version_output} == "oxys ${VERSION}" ]] || {
	echo "ERROR: built CLI reports '${version_output}', expected 'oxys ${VERSION}'" >&2
	exit 1
}
case "$(file -Lb "${BUILT_BINARY}")" in
	*"static-pie linked"*|*"statically linked"*) ;;
	*)
		echo "ERROR: ${BUILT_BINARY} is not statically linked" >&2
		exit 1
		;;
esac

mkdir -p "${PACKAGE_DIR}/files"
# Generated FILESDIR payloads are not source-controlled. Remove the previous
# version before staging the current one so an ignored, unmanifested binary
# cannot survive a version bump and leak into the ISO or release repository.
find "${PACKAGE_DIR}/files" -mindepth 1 -maxdepth 1 \
	-name 'oxys-*' ! -name "oxys-${VERSION}" -delete
install -D -m 0755 "${BUILT_BINARY}" "${PAYLOAD}"

manifest_tmp="$(mktemp "${PACKAGE_DIR}/.Manifest.XXXXXX")"
trap '[[ -z ${manifest_tmp:-} ]] || rm -f "${manifest_tmp}"' EXIT
size="$(stat -c '%s' "${PAYLOAD}")"
blake2b="$(b2sum "${PAYLOAD}")"
blake2b="${blake2b%% *}"
sha512="$(sha512sum "${PAYLOAD}")"
sha512="${sha512%% *}"
printf 'AUX oxys-%s %s BLAKE2B %s SHA512 %s\n' \
	"${VERSION}" "${size}" "${blake2b}" "${sha512}" > "${manifest_tmp}"
chmod 0644 "${manifest_tmp}"
mv "${manifest_tmp}" "${PACKAGE_DIR}/Manifest"
manifest_tmp=""

# Synchronize only app-admin/oxys. Other packages in these two overlay copies
# predate this release flow and are deliberately outside the updater package's
# staging transaction.
[[ ${ISO_PACKAGE_DIR} == "${MONOREPO_ROOT}/oxys-iso/overlay/var/db/repos/oxys/app-admin/oxys" ]] || {
	echo "ERROR: refusing to replace unexpected ISO package path: ${ISO_PACKAGE_DIR}" >&2
	exit 1
}
if ! grep -Fxq 'app-admin' "${ISO_CATEGORIES}"; then
	echo "ERROR: ISO Oxys overlay does not declare the app-admin category" >&2
	exit 1
fi
rm -rf "${ISO_PACKAGE_DIR}"
mkdir -p "${ISO_PACKAGE_DIR%/*}"
cp -a "${PACKAGE_DIR}" "${ISO_PACKAGE_DIR}"
rm -f "${LEGACY_ISO_PAYLOAD}"
if ! diff -qr "${PACKAGE_DIR}" "${ISO_PACKAGE_DIR}" >/dev/null; then
	echo "ERROR: canonical and ISO app-admin/oxys packages differ after synchronization" >&2
	exit 1
fi

echo ">> staged ${PAYLOAD}"
echo ">> wrote ${PACKAGE_DIR}/Manifest"
echo ">> synchronized app-admin/oxys into ${ISO_PACKAGE_DIR}"
