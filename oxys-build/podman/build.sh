#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"
OUTPUT_DIR="${REPO_ROOT}/output"
BUILD_LOG="${OUTPUT_DIR}/build.log"

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

prepare_output() {
  mkdir -p "${OUTPUT_DIR}"
  : > "${BUILD_LOG}"
}

build_image() {
  local arch="$1"
  log "Building image oxys-build:${arch}"
  podman build \
    --tag "oxys-build:${arch}" \
    --file "${SCRIPT_DIR}/Containerfile.${arch}" \
    "${SCRIPT_DIR}"
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

  podman run \
    --rm \
    --privileged \
    "${env_args[@]}" \
    --volume "${REPO_ROOT}:/src:ro,Z" \
    --volume "${OUTPUT_DIR}:/out:Z" \
    "oxys-build:${arch}"
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
                  ISO consumes (profiles: kernel, native, generic, pgo)
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

main() {
  local -a targets=("$@")
  local target arch profile

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
      build_image "${arch}"
      run_image "${arch}" "${profile}"
      summarise_times "${arch}"
    else
      build_arch "${target}"
    fi
  done

  log "All requested targets completed"
}

main "$@"
