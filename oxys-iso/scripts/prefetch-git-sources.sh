#!/usr/bin/env bash
# Validate and prefetch persistent host-side caches for live git-r3 sources.
#
# Each configured repository is fetched before the container/build starts.
# build.sh consumes --list output, copies the bare repos into git-r3's exact
# DISTDIR stores, and generates package-scoped EVCS_OFFLINE mappings.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_DIR="$(cd "${SCRIPT_DIR}/.." && pwd)"

mode="prefetch"
if [[ "${1:-}" == "--list" ]]; then
	mode="list"
	shift
fi
if (( $# > 1 )); then
	echo "Usage: $0 [--list] [git-sources.conf]" >&2
	exit 2
fi

SOURCES_FILE="${1:-${REPO_DIR}/git-sources.conf}"
CACHE_ROOT="${REPO_DIR}/.build/source-cache/git3-src"
CACHE_REF="refs/heads/oxys-source"
tmp_repo=""

cleanup() {
	if [[ -n "${tmp_repo}" ]]; then
		rm -rf -- "${tmp_repo}"
	fi
}
trap cleanup EXIT

# Keep this identical to git-r3.eclass::_git-r3_set_gitdir so the preseeded
# directory is exactly the one the eclass will look for in offline mode.
git_r3_store_name() {
	local repo_uri="$1" repo_name
	[[ "${repo_uri}" == *://*/* ]] || return 1
	repo_name=${repo_uri#*://*/}
	repo_name=${repo_name%/}
	case "${repo_name}" in
		browse/*) repo_name=${repo_name#browse/} ;;
		cgit/*) repo_name=${repo_name#cgit/} ;;
		git/*) repo_name=${repo_name#git/} ;;
		gitroot/*) repo_name=${repo_name#gitroot/} ;;
		p/*) repo_name=${repo_name#p/} ;;
		pub/scm/*) repo_name=${repo_name#pub/scm/} ;;
	esac
	repo_name=${repo_name%.git}.git
	repo_name=${repo_name//\//_}
	printf '%s\n' "${repo_name}"
}

emit_sources() {
	local package_atom source_uri source_ref extra store_name
	local line_no=0 source_count=0
	declare -A seen_stores=()

	if [[ ! -f "${SOURCES_FILE}" ]]; then
		echo "ERROR: Git source manifest not found: ${SOURCES_FILE}" >&2
		return 1
	fi

	while read -r package_atom source_uri source_ref extra \
		|| [[ -n "${package_atom:-}" ]]; do
		((line_no += 1))
		[[ -z "${package_atom}" || "${package_atom}" == \#* ]] && continue

		if [[ -z "${source_uri:-}" || -n "${extra:-}" ]]; then
			echo "ERROR: malformed ${SOURCES_FILE}:${line_no}; expected 2 or 3 fields." >&2
			return 1
		fi
		source_ref="${source_ref:-HEAD}"
		if [[ "${package_atom}" != */* ]]; then
			echo "ERROR: invalid package atom '${package_atom}' at ${SOURCES_FILE}:${line_no}." >&2
			return 1
		fi
		if [[ "${source_ref}" != "HEAD" && "${source_ref}" != refs/* \
			&& ! "${source_ref}" =~ ^[0-9a-fA-F]{40}$ \
			&& ! "${source_ref}" =~ ^[0-9a-fA-F]{64}$ ]]; then
			echo "ERROR: ref '${source_ref}' at ${SOURCES_FILE}:${line_no} must be" >&2
			echo "       HEAD, a full refs/... name, or a full commit ID." >&2
			return 1
		fi
		if ! store_name="$(git_r3_store_name "${source_uri}")"; then
			echo "ERROR: '${source_uri}' is not a canonical scheme://host/path Git URI" >&2
			echo "       (${SOURCES_FILE}:${line_no})." >&2
			return 1
		fi
		if [[ ! "${store_name}" =~ ^[A-Za-z0-9._+-]+[.]git$ ]]; then
			echo "ERROR: unsafe derived git-r3 store '${store_name}' at ${SOURCES_FILE}:${line_no}." >&2
			return 1
		fi
		if [[ -n "${seen_stores[$store_name]:-}" ]]; then
			echo "ERROR: duplicate git-r3 store '${store_name}' at ${SOURCES_FILE}:${line_no}." >&2
			return 1
		fi
		seen_stores["${store_name}"]=1

		printf '%s\t%s\t%s\t%s\n' \
			"${package_atom}" "${store_name}" "${source_uri}" "${source_ref}"
		((source_count += 1))
	done < "${SOURCES_FILE}"

	if (( source_count == 0 )); then
		echo "ERROR: no Git sources configured in ${SOURCES_FILE}." >&2
		return 1
	fi
}

if ! source_rows="$(emit_sources)"; then
	exit 1
fi
if [[ "${mode}" == "list" ]]; then
	printf '%s\n' "${source_rows}"
	trap - EXIT
	exit 0
fi

cache_is_valid() {
	local repo="$1" source_ref="$2"
	git --git-dir="${repo}" cat-file -e "${CACHE_REF}^{commit}" 2>/dev/null \
		|| return 1
	if [[ "${source_ref}" == refs/* \
		|| "${source_ref}" =~ ^[0-9a-fA-F]{40}$ \
		|| "${source_ref}" =~ ^[0-9a-fA-F]{64}$ ]]; then
		git --git-dir="${repo}" cat-file -e "${source_ref}^{commit}" 2>/dev/null \
			|| return 1
	fi
}

fetch_ref() {
	local repo="$1" source_uri="$2" source_ref="$3" attempt
	local -a fetch_specs=( "+${source_ref}:${CACHE_REF}" )
	if [[ "${source_ref}" == refs/* && "${source_ref}" != "${CACHE_REF}" ]]; then
		# Preserve the ebuild-requested branch/tag as well as our stable HEAD.
		fetch_specs+=( "+${source_ref}:${source_ref}" )
	fi
	for attempt in 1 2 3; do
		# Detached auto-maintenance can race an atomic temp-repo rename and
		# recreate the old path. The build cache is maintained explicitly.
		if git -c maintenance.auto=false -c gc.auto=0 \
			--git-dir="${repo}" fetch --no-tags "${source_uri}" \
			"${fetch_specs[@]}"; then
			git --git-dir="${repo}" symbolic-ref HEAD "${CACHE_REF}" || return 1
			return 0
		fi
		if (( attempt < 3 )); then
			echo ">> Git source fetch attempt ${attempt} failed; retrying..." >&2
			sleep $((attempt * 2))
		fi
	done
	return 1
}

mkdir -p "${CACHE_ROOT}"
source_count=0
while IFS=$'\t' read -r package_atom store_name source_uri source_ref; do
	cache_repo="${CACHE_ROOT}/${store_name}"
	if cache_is_valid "${cache_repo}" "${source_ref}"; then
		if [[ "${OXYS_GIT_REFRESH:-1}" == "0" ]]; then
			echo ">> using cached Git source for ${package_atom} (refresh disabled)"
		elif ! fetch_ref "${cache_repo}" "${source_uri}" "${source_ref}"; then
			# A failed refresh never changes CACHE_REF. Keeping the last complete
			# checkout is preferable to making an otherwise-offline rebuild fail.
			echo "WARNING: could not refresh ${package_atom}; using its existing cached commit." >&2
		fi
	else
		if [[ "${OXYS_GIT_REFRESH:-1}" == "0" ]]; then
			echo "ERROR: no usable cached Git source exists for ${package_atom}." >&2
			echo "       Re-run without OXYS_GIT_REFRESH=0 to fetch it first." >&2
			exit 1
		fi
		# Build a first-time cache atomically so an interrupted fetch cannot be
		# mistaken for a usable repository on the next run.
		tmp_repo="$(mktemp -d "${CACHE_ROOT}/.${store_name}.XXXXXX")"
		git init --bare -b oxys-source "${tmp_repo}" >/dev/null
		if ! fetch_ref "${tmp_repo}" "${source_uri}" "${source_ref}"; then
			cleanup
			tmp_repo=""
			echo "ERROR: unable to prefetch ${package_atom} from ${source_uri}." >&2
			echo "       Fix DNS/networking and retry; catalyst has not started." >&2
			exit 1
		fi
		rm -rf -- "${cache_repo}"
		mv "${tmp_repo}" "${cache_repo}"
		tmp_repo=""
	fi

	commit="$(git --git-dir="${cache_repo}" rev-parse "${CACHE_REF}^{commit}")"
	echo ">> cached ${package_atom} at ${cache_repo} (${commit:0:12})"
	((source_count += 1))
done <<< "${source_rows}"

trap - EXIT
echo ">> ${source_count} Git source cache(s) ready"
