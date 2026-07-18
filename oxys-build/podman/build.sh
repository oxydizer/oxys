#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"
OUTPUT_DIR="${REPO_ROOT}/output"
BUILD_LOG="${OUTPUT_DIR}/build.log"
MONOREPO_ROOT="$(cd "${REPO_ROOT}/.." && pwd)"
BINPKG_SIGNING_SOURCE_MOUNT="/run/oxys-binpkg-signing-source"
OXYS_APP_EBUILD_DIR="${REPO_ROOT}/oxys-overlay/app-admin/oxys"
OXYS_APP_STAGE_HELPER="${REPO_ROOT}/scripts/stage-oxys-package.sh"
BINHOST_WORK_DIR="${OUTPUT_DIR}/.binhost-work/v3"
BINHOST_PUBLISH_DIR="${OUTPUT_DIR}/binpackages"

readonly -a NATIVE_ARCHES=(
  "alderlake"
  "znver3"
  "znver4"
  "znver5"
)

timestamp() {
  date -u +"%Y-%m-%dT%H:%M:%SZ"
}

log() {
  printf '[%s] %s\n' "$(timestamp)" "$*" | tee -a "${BUILD_LOG}"
}

resolve_manifest_graphics_policy() {
  [[ -n "${OXYS_GRAPHICS_MANIFEST:-}" ]] || return 0
  if [[ -n "${OXYS_VIDEO_CARDS:-}" || -n "${OXYS_DRM_DRIVERS:-}" ]]; then
    printf 'OXYS_GRAPHICS_MANIFEST cannot be combined with explicit OXYS_VIDEO_CARDS/OXYS_DRM_DRIVERS\n' >&2
    exit 1
  fi

  local oxys_bin="${OXYS_BIN:-${MONOREPO_ROOT}/target/debug/oxys}"
  if [[ ! -x "${oxys_bin}" ]]; then
    cargo build --manifest-path "${MONOREPO_ROOT}/Cargo.toml" -p oxys --bin oxys
  fi
  [[ -x "${oxys_bin}" ]] || { printf 'Oxys CLI not found at %s\n' "${oxys_bin}" >&2; exit 1; }

  local policy
  policy="$("${oxys_bin}" graphics-build-policy "${OXYS_GRAPHICS_MANIFEST}")"
  eval "${policy}"
  export OXYS_VIDEO_CARDS OXYS_DRM_DRIVERS
  printf 'Resolved graphics build policy from %s\n' "${OXYS_GRAPHICS_MANIFEST}"
}

prepare_output() {
  mkdir -p "${OUTPUT_DIR}"
  : > "${BUILD_LOG}"
}

build_image() {
  local arch="$1"
  log "Building image oxys-build:${arch}"
  if ! podman build \
    --tag "oxys-build:${arch}" \
    --file "${SCRIPT_DIR}/Containerfile.${arch}" \
    "${SCRIPT_DIR}"; then
    log "Image build failed for ${arch}"
    return 1
  fi
}

run_image() {
  local arch="$1"
  local profile="${2:-}"
  log "Running build container for ${arch}${profile:+ (${profile})}"
  mkdir -p "${OUTPUT_DIR}/${arch}"

  local -a env_args=()
  if [[ -n "${profile}" ]]; then
    env_args+=(--env "OXYS_BUILD_PROFILE=${profile}")
  fi
  if [[ -n "${OXYS_BORE_PATCH_URL:-}" ]]; then
    env_args+=(--env "OXYS_BORE_PATCH_URL=${OXYS_BORE_PATCH_URL}")
  fi
  if [[ -n "${OXYS_APPLY_BORE:-}" ]]; then
    env_args+=(--env "OXYS_APPLY_BORE=${OXYS_APPLY_BORE}")
  fi
  if [[ -n "${OXYS_VIDEO_CARDS:-}" ]]; then
    env_args+=(--env "OXYS_VIDEO_CARDS=${OXYS_VIDEO_CARDS}")
  fi
  if [[ -n "${OXYS_DRM_DRIVERS:-}" ]]; then
    env_args+=(--env "OXYS_DRM_DRIVERS=${OXYS_DRM_DRIVERS}")
  fi

  if ! podman run \
    --rm \
    --privileged \
    "${env_args[@]}" \
    --volume "${REPO_ROOT}:/src:ro,Z" \
    --volume "${OUTPUT_DIR}:/out:Z" \
    "oxys-build:${arch}"; then
    log "Build container failed for ${arch}${profile:+ (${profile})}"
    return 1
  fi
  log "Completed ${arch}${profile:+ (${profile})}"
}

summarise_times() {
  local arch="$1"
  local time_file="${OUTPUT_DIR}/${arch}/build-times.tsv"
  if [[ -f "${time_file}" ]]; then
    log "Package timings for ${arch}:"
    while IFS=$'\t' read -r built_arch atom seconds; do
      printf '[%s] %s %s %ss\n' "$(timestamp)" "${built_arch}" "${atom}" "${seconds}" | tee -a "${BUILD_LOG}"
    done < "${time_file}"
  fi
}

usage() {
  cat <<EOF
Usage: ${0##*/} [target ...]

Targets:
  v3              full v3 pipeline: kernel, generic, pgo  (DEFAULT)
  ${NATIVE_ARCHES[*]}
                  full native pipeline for that arch: kernel, native
  all             every native arch, then v3 (the old no-arg behaviour)
  <arch>:<profile>
                  a single profile for one arch, e.g. "v3:kernel" to
                  rebuild just the kernel + zfs-kmod + zfs tarballs the
                  ISO consumes (profiles: kernel, native, generic, pgo;
                  v3 also supports the isolated binhost profile)

Signed app-admin/oxys binhost (opt-in; never part of a full/default target):
  v3:binhost      build a signed one-package GPKG repository under
                  output/binpackages/x86-64-v3/
  OXYS_BINPKG_SIGNING_GPG_HOME
                  host GnuPG home containing the release private key
  OXYS_BINPKG_SIGNING_KEY
                  full 40-digit GnuPG signing-key fingerprint

Graphics policy (space- or comma-separated environment values):
  OXYS_GRAPHICS_MANIFEST
                      Generated manifest.toml used to derive both values below
  OXYS_VIDEO_CARDS    Mesa VIDEO_CARDS build input
  OXYS_DRM_DRIVERS    Kernel DRM drivers (intel, amdgpu, radeon, nouveau,
                      virtio_gpu, vmwgfx)
EOF
}

# Full pipeline for one arch (image build + every profile that arch ships).
build_arch() {
  local arch="$1"
  build_image "${arch}"
  if [[ "${arch}" == "v3" ]]; then
    run_image "v3" "kernel"
    run_image "v3" "generic"
    run_image "v3" "pgo"
  else
    run_image "${arch}" "kernel"
    run_image "${arch}" "native"
  fi
  summarise_times "${arch}"
}

known_arch() {
  local candidate="$1" arch
  [[ "${candidate}" == "v3" ]] && return 0
  for arch in "${NATIVE_ARCHES[@]}"; do
    [[ "${candidate}" == "${arch}" ]] && return 0
  done
  return 1
}

validate_binhost_signing_inputs() {
  if [[ -z "${OXYS_BINPKG_SIGNING_GPG_HOME:-}" ]]; then
    printf 'v3:binhost requires OXYS_BINPKG_SIGNING_GPG_HOME\n' >&2
    exit 1
  fi
  if [[ ! -d "${OXYS_BINPKG_SIGNING_GPG_HOME}" ]]; then
    printf 'OXYS_BINPKG_SIGNING_GPG_HOME is not a directory: %s\n' \
      "${OXYS_BINPKG_SIGNING_GPG_HOME}" >&2
    exit 1
  fi
  if [[ -z "${OXYS_BINPKG_SIGNING_KEY:-}" ]]; then
    printf 'v3:binhost requires OXYS_BINPKG_SIGNING_KEY\n' >&2
    exit 1
  fi
  if [[ ! "${OXYS_BINPKG_SIGNING_KEY}" =~ ^[[:xdigit:]]{40}$ ]]; then
    printf 'OXYS_BINPKG_SIGNING_KEY must be a full 40-digit hexadecimal fingerprint\n' >&2
    exit 1
  fi
}

stage_and_validate_binhost_payload() {
  if [[ ! -x "${OXYS_APP_STAGE_HELPER}" ]]; then
    printf 'v3:binhost requires the release staging helper at %s\n' \
      "${OXYS_APP_STAGE_HELPER}" >&2
    exit 1
  fi

  "${OXYS_APP_STAGE_HELPER}" --cpu-baseline x86-64-v3

  if [[ ! -s "${OXYS_APP_EBUILD_DIR}/Manifest" ]]; then
    printf 'v3:binhost requires the staged app-admin/oxys Manifest at %s\n' \
      "${OXYS_APP_EBUILD_DIR}/Manifest" >&2
    exit 1
  fi
  local payload manifest_payload payload_version
  local -a payloads=() ebuilds=()
  manifest_payload="$(awk '$1 == "AUX" && $2 ~ /^oxys-[0-9]/ { print $2 }' \
    "${OXYS_APP_EBUILD_DIR}/Manifest")"
  if [[ -z "${manifest_payload}" || "${manifest_payload}" == *$'\n'* ]]; then
    printf 'app-admin/oxys Manifest must cover exactly one versioned oxys payload: %s\n' \
      "${OXYS_APP_EBUILD_DIR}/Manifest" >&2
    exit 1
  fi
  payload="${OXYS_APP_EBUILD_DIR}/files/${manifest_payload}"
  if [[ ! -x "${payload}" ]]; then
    printf 'v3:binhost requires the executable Manifest payload at %s\n' \
      "${payload}" >&2
    exit 1
  fi
  payload_version="${manifest_payload#oxys-}"
  if [[ ! -f "${OXYS_APP_EBUILD_DIR}/oxys-${payload_version}.ebuild" ]]; then
    printf 'staged payload %s has no matching versioned ebuild\n' \
      "${manifest_payload}" >&2
    exit 1
  fi
  mapfile -d '' payloads < <(
    find "${OXYS_APP_EBUILD_DIR}/files" -maxdepth 1 -type f -name 'oxys-*' -print0
  )
  if (( ${#payloads[@]} != 1 )) || [[ "${payloads[0]:-}" != "${payload}" ]]; then
    printf 'v3:binhost requires exactly one current files/oxys-* payload\n' >&2
    exit 1
  fi
  mapfile -d '' ebuilds < <(
    find "${OXYS_APP_EBUILD_DIR}" -maxdepth 1 -type f -name 'oxys-*.ebuild' -print0
  )
  if (( ${#ebuilds[@]} != 1 )) || \
      [[ "${ebuilds[0]:-}" != "${OXYS_APP_EBUILD_DIR}/oxys-${payload_version}.ebuild" ]]; then
    printf 'v3:binhost requires exactly one ebuild matching the staged payload\n' >&2
    exit 1
  fi
}

run_binhost_signer() {
  local signing_home status=0
  signing_home="$(realpath -e -- "${OXYS_BINPKG_SIGNING_GPG_HOME}")"

  log "Signing and validating the v3 binhost in an offline, unprivileged container"
  podman run \
    --rm \
    --network=none \
    --cap-drop=all \
    --security-opt=no-new-privileges \
    --security-opt=label=disable \
    --tmpfs /run:rw,nosuid,nodev,noexec,size=16m,mode=0755 \
    --env "OXYS_BUILD_PROFILE=binhost-sign" \
    --env "OXYS_BINPKG_SIGNING_GPG_HOME=${BINPKG_SIGNING_SOURCE_MOUNT}" \
    --env "OXYS_BINPKG_SIGNING_KEY=${OXYS_BINPKG_SIGNING_KEY}" \
    --env "OXYS_BINHOST_WORK_ROOT=/binhost-work" \
    --env "OXYS_BINHOST_PUBLISH_ROOT=/binhost-publish" \
    --env "OXYS_OUTPUT_ROOT=/binhost-work" \
    --volume "${signing_home}:${BINPKG_SIGNING_SOURCE_MOUNT}:ro" \
    --volume "${BINHOST_WORK_DIR}:/binhost-work:rw,Z" \
    --volume "${BINHOST_PUBLISH_DIR}:/binhost-publish:rw,Z" \
    --entrypoint /work/scripts/oxys-build-packages.sh \
    "oxys-build:v3" 2>&1 | \
    tee "${OUTPUT_DIR}/v3/binhost-signer.console.log" || status=$?

  if [[ -f "${BINHOST_WORK_DIR}/v3/container.log" ]]; then
    cp "${BINHOST_WORK_DIR}/v3/container.log" \
      "${OUTPUT_DIR}/v3/binhost-signer.log"
  fi
  return "${status}"
}

build_binhost_target() {
  rm -rf "${BINHOST_WORK_DIR}"
  mkdir -p "${BINHOST_WORK_DIR}" "${BINHOST_PUBLISH_DIR}"

  if ! build_image "v3"; then
    rm -rf "${BINHOST_WORK_DIR}"
    return 1
  fi
  if ! run_image "v3" "binhost-build"; then
    rm -rf "${BINHOST_WORK_DIR}"
    return 1
  fi
  if ! run_binhost_signer; then
    rm -rf "${BINHOST_WORK_DIR}"
    return 1
  fi

  rm -rf "${BINHOST_WORK_DIR}"
  summarise_times "v3"
}

main() {
  local -a targets=("$@")
  local target arch profile
  local binhost_requested=0

  if (( ${#targets[@]} == 0 )); then
    targets=("v3")
  fi

  # Validate every target before prepare_output: prepare_output truncates
  # the previous build.log, and a typo'd invocation must not cost it.
  for target in "${targets[@]}"; do
    case "${target}" in
      -h|--help)
        usage
        exit 0
        ;;
      all)
        ;;
      *:*)
        arch="${target%%:*}"
        profile="${target#*:}"
        if ! known_arch "${arch}"; then
          printf 'Unknown arch %s in target %s\n' "${arch}" "${target}" >&2
          usage >&2
          exit 1
        fi
        case "${profile}" in
          kernel|native|generic|pgo) ;;
          binhost)
            if [[ "${arch}" != "v3" ]]; then
              printf 'The binhost profile is supported only by the v3 target\n' >&2
              usage >&2
              exit 1
            fi
            validate_binhost_signing_inputs
            binhost_requested=1
            ;;
          *)
            printf 'Unknown profile %s in target %s\n' "${profile}" "${target}" >&2
            usage >&2
            exit 1
            ;;
        esac
        ;;
      *)
        if ! known_arch "${target}"; then
          printf 'Unknown target %s\n' "${target}" >&2
          usage >&2
          exit 1
        fi
        ;;
    esac
  done

  if (( binhost_requested == 1 )); then
    # Build the static, versioned payload and refresh its Manifest immediately
    # before building the image, so the read-only /src mount is self-contained.
    stage_and_validate_binhost_payload
  fi
  resolve_manifest_graphics_policy
  prepare_output
  log "Starting OxysOS Podman package build (targets: ${targets[*]})"

  for target in "${targets[@]}"; do
    if [[ "${target}" == "all" ]]; then
      for arch in "${NATIVE_ARCHES[@]}"; do
        build_arch "${arch}"
      done
      build_arch "v3"
    elif [[ "${target}" == *:* ]]; then
      arch="${target%%:*}"
      profile="${target#*:}"
      if [[ "${profile}" == "binhost" ]]; then
        build_binhost_target
      else
        build_image "${arch}"
        run_image "${arch}" "${profile}"
        summarise_times "${arch}"
      fi
    else
      build_arch "${target}"
    fi
  done

  log "All requested targets completed"
}

main "$@"
