#!/usr/bin/env bash
# enter-container.sh - build (once) and enter the OxysOS catalyst container.
#
#   ./scripts/enter-container.sh            # interactive shell in the build env
#   ./scripts/enter-container.sh build      # run /oxys/oxys-iso/build.sh, then exit
#   ./scripts/enter-container.sh -- <cmd>   # run an arbitrary command
#
# Runs ROOTFUL (via sudo) with loop-device passthrough. catalyst mounts its
# squashfs snapshot and builds the ISO via loop devices, which rootless podman
# cannot do even with --privileged (loop setup needs CAP_SYS_ADMIN in the
# initial user namespace). Symptom of getting this wrong:
#   "Couldn't mount .../gentoo-*.sqfs, Loopdev setup failed".
#
# Environment:
#   OXYS_CATALYST_DIR   Host catalyst storage, default: ~/catalyst
#   OXYS_IMAGE          Image tag, default: localhost/oxys-catalyst:latest
#   OXYS_PODMAN         podman invocation, default: "sudo podman"
#   OXYS_REBUILD=1      Force rebuild of the image before entering.
#   OXYS_ARCH           Forwarded into the container for build.sh (required by
#                        build.sh; no default here — see oxys-iso/README.md).
#   OXYS_KERNEL_BUILD_ID Forwarded into the container for build.sh (optional).
#   OXYS_TREEISH        Forwarded into the container for build.sh (optional).

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_DIR="$(cd "${SCRIPT_DIR}/.." && pwd)"
# Monorepo root, bind-mounted whole at /oxys so build.sh can also reach
# ../oxys-build/output/<arch>/ for the prebuilt kernel/zfs-kmod tarballs.
MONOREPO_ROOT="$(cd "${REPO_DIR}/.." && pwd)"

CATALYST_DIR="${OXYS_CATALYST_DIR:-${HOME}/catalyst}"
IMAGE="${OXYS_IMAGE:-localhost/oxys-catalyst:latest}"
# shellcheck disable=SC2206  # intentional word-splitting of the podman command
PODMAN=(${OXYS_PODMAN:-sudo podman})

# --- parse the requested in-container command -------------------------------
CMD=(bash)
case "${1:-}" in
    build)      CMD=(/oxys/oxys-iso/build.sh) ;;
    --)         shift; CMD=("$@") ;;
    "")         ;;                       # default interactive shell
    -h|--help)  grep '^#' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
    *)          CMD=("$@") ;;
esac

# --- refresh generated payloads before the container sees the repo -----------
if [[ "${CMD[0]}" == "/oxys/oxys-iso/build.sh" ]]; then
    "${REPO_DIR}/scripts/build-installer-overlay.sh"
fi

# --- ensure host catalyst storage exists (podman won't create mount sources) -
mkdir -p "${CATALYST_DIR}"/{builds,packages,snapshots,tmp,kerncache}

# --- build the image once (or when forced / missing) ------------------------
if [[ "${OXYS_REBUILD:-}" == "1" ]] || ! "${PODMAN[@]}" image exists "${IMAGE}"; then
    echo ">> building ${IMAGE} (one-time; runs emerge-webrsync + emerge catalyst)"
    # --network=host: rootful bridge networking copies the host's
    # /etc/resolv.conf verbatim into the build namespace, but a link-local
    # IPv6 nameserver scoped to a host interface (e.g. `%wlan0`) isn't usable
    # there, breaking emerge-webrsync's DNS lookups. Host networking sidesteps
    # that since the real scoped interface exists.
    "${PODMAN[@]}" build --network=host -t "${IMAGE}" -f "${REPO_DIR}/Containerfile" "${REPO_DIR}"
fi

# --- enter: rootful, privileged, loop passthrough, repo + storage bind mounts
echo ">> entering ${IMAGE}  (cmd: ${CMD[*]})"
exec "${PODMAN[@]}" run --privileged --rm -it \
    --network=host \
    --device /dev/loop-control \
    -v /dev:/dev \
    -v "${MONOREPO_ROOT}:/oxys:Z" \
    -v "${CATALYST_DIR}:/var/tmp/catalyst:Z" \
    ${OXYS_ARCH:+-e OXYS_ARCH="${OXYS_ARCH}"} \
    ${OXYS_KERNEL_BUILD_ID:+-e OXYS_KERNEL_BUILD_ID="${OXYS_KERNEL_BUILD_ID}"} \
    ${OXYS_TREEISH:+-e OXYS_TREEISH="${OXYS_TREEISH}"} \
    "${IMAGE}" "${CMD[@]}"
