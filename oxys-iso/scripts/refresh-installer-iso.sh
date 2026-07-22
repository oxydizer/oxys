#!/usr/bin/env bash
# Replace only oxys-installer and its bundled configs in an existing ISO.
# Catalyst, the kernel, and the live package set are not rebuilt.
set -euo pipefail
ORIGINAL_ARGS=("$@")

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ISO_DIR="$(cd "${SCRIPT_DIR}/.." && pwd)"
MONOREPO_ROOT="$(cd "${ISO_DIR}/.." && pwd)"
INSTALLER_DIR="${MONOREPO_ROOT}/oxys-installer"
TARGET="x86_64-unknown-linux-musl"
CARGO_TARGET_ROOT="${CARGO_TARGET_DIR:-${MONOREPO_ROOT}/target}"
INSTALLER_BIN="${CARGO_TARGET_ROOT}/${TARGET}/release/oxys-installer"

usage() {
	cat <<EOF
Usage: $(basename "$0") [input.iso] [output.iso]

Rebuild only oxys-installer and inject it, plus configs/*.fe2o3, into an
existing OxysOS ISO. If input.iso is omitted, the newest ISO is selected from
OXYS_ISO_DIR, ~/catalyst/builds/23.0-default, or the current directory.

Set OXYS_SQUASHFS_COMPRESSION to gzip, lzo, lz4, xz, or zstd to recompress the
live root instead of preserving the input ISO's compressor.
EOF
}

case "${1:-}" in
	-h|--help) usage; exit 0 ;;
esac

for command in bwrap cargo xorriso unsquashfs mksquashfs sha256sum; do
	command -v "${command}" >/dev/null 2>&1 || {
		echo "ERROR: required command not found: ${command}" >&2
		exit 1
	}
done

input_iso="${1:-}"
if [[ -z "${input_iso}" ]]; then
	search_dirs=(
		"${OXYS_ISO_DIR:-}"
		"${HOME}/catalyst/builds/23.0-default"
		"${PWD}"
	)
	for dir in "${search_dirs[@]}"; do
		[[ -n "${dir}" && -d "${dir}" ]] || continue
		input_iso="$(find "${dir}" -maxdepth 1 -type f -name 'oxysos-*.iso' -printf '%T@ %p\n' 2>/dev/null \
			| sort -rn | head -n1 | cut -d' ' -f2-)"
		[[ -n "${input_iso}" ]] && break
	done
fi

[[ -n "${input_iso}" && -f "${input_iso}" ]] || {
	echo "ERROR: no input ISO found; pass its path explicitly." >&2
	exit 1
}
input_iso="$(realpath "${input_iso}")"

stamp="$(date -u +%Y%m%dT%H%M%SZ)"
output_iso="${2:-${input_iso%.iso}-installer-${stamp}.iso}"
output_iso="$(realpath -m "${output_iso}")"
[[ "${input_iso}" != "${output_iso}" ]] || {
	echo "ERROR: output must differ from input; the source ISO is never modified in place." >&2
	exit 1
}
[[ ! -e "${output_iso}" ]] || {
	echo "ERROR: output already exists: ${output_iso}" >&2
	exit 1
}
mkdir -p "$(dirname "${output_iso}")"

if command -v rustup >/dev/null 2>&1; then
	rustup target add "${TARGET}" >/dev/null
fi

if [[ "${OXYS_INSTALLER_ALREADY_BUILT:-0}" != "1" ]]; then
	echo ">> building oxys-installer (${TARGET})"
	cargo build \
		--manifest-path "${INSTALLER_DIR}/Cargo.toml" \
		--release \
		--target "${TARGET}"
fi
[[ -x "${INSTALLER_BIN}" ]] || {
	echo "ERROR: installer build did not produce ${INSTALLER_BIN}" >&2
	exit 1
}

# Squashfs contains device nodes, root-owned files, and privileged modes.
# fakeroot lets unsquashfs/mksquashfs round-trip that metadata without making
# Cargo or the resulting ISO genuinely root-owned.
if [[ "${EUID}" -ne 0 && -z "${FAKEROOTKEY:-}" ]]; then
	command -v fakeroot >/dev/null 2>&1 || {
		echo "ERROR: fakeroot is required to preserve live-root metadata." >&2
		exit 1
	}
	exec env OXYS_INSTALLER_ALREADY_BUILT=1 \
		CARGO_TARGET_DIR="${CARGO_TARGET_ROOT}" \
		fakeroot -- "$0" "${ORIGINAL_ARGS[@]}"
fi

work="$(mktemp -d "${TMPDIR:-/tmp}/oxys-refresh-installer.XXXXXX")"
cleanup() {
	# ISO9660 extraction preserves read-only directory modes. Restore owner
	# write/search permission so an unprivileged invocation can remove scratch.
	chmod -R u+rwX "${work}" 2>/dev/null || true
	rm -rf "${work}"
}
trap cleanup EXIT

# xorriso writes even `-find ... -exec echo` results to stderr, so merge the
# streams and retain only its single-quoted ISO path result.
squash_path="$(xorriso -indev "${input_iso}" \
	-find / -name image.squashfs -type f -exec echo -- 2>&1 \
	| sed -n "s|^'\(/.*image\.squashfs\)'$|\1|p" \
	| tail -n1)"
[[ -n "${squash_path}" ]] || {
	echo "ERROR: image.squashfs was not found in ${input_iso}" >&2
	exit 1
}

echo ">> extracting ISO filesystem"
xorriso -osirrox on -indev "${input_iso}" \
	-extract / "${work}/iso-root" >/dev/null 2>&1
# ISO9660 directory modes are read-only. Make the scratch copy replaceable;
# this never alters the source ISO.
chmod -R u+rwX "${work}/iso-root"
disk_squash_path="${work}/iso-root${squash_path}"
[[ -f "${disk_squash_path}" ]] || {
	echo "ERROR: extracted squashfs is missing: ${disk_squash_path}" >&2
	exit 1
}
# Device nodes such as /dev/console cannot be recreated by an unprivileged
# caller. They are supplied by devtmpfs at boot, so keep extraction going and
# do not turn those non-fatal warnings into a failed refresh.
unsquashfs -no-exit-code -d "${work}/root" "${disk_squash_path}" >/dev/null

install -D -m 0755 "${INSTALLER_BIN}" "${work}/root/usr/local/bin/oxys-installer"
install -d -m 0755 "${work}/root/root/configs"
for config in base desktop custom; do
	install -m 0644 "${INSTALLER_DIR}/configs/${config}.fe2o3" \
		"${work}/root/root/configs/${config}.fe2o3"
done

# Older images shipped the GURU and Oxys ebuild trees without pregenerated
# metadata. The installer plans from md5-cache rather than executing ebuilds,
# so evaluate the required ebuilds with the Portage environment in the
# extracted live root. Never copy dependency fields from /var/db/pkg: VDB
# records expand := dependencies to built forms such as :0/10=, and those are
# invalid in repository metadata (Portage masks the ebuild at install time).
live_root="${work}/root"
preserved_gentoo="${live_root}/var/db/repos/.oxys-repositories/gentoo"
gentoo_link="${live_root}/var/db/repos/gentoo"
if [[ ! -x ${live_root}/usr/bin/egencache ]]; then
	echo "ERROR: extracted live root has no executable /usr/bin/egencache." >&2
	exit 1
fi
if [[ ! -s ${preserved_gentoo}/profiles/repo_name ]] \
	|| [[ "$(<"${preserved_gentoo}/profiles/repo_name")" != gentoo ]] \
	|| [[ ! -d ${preserved_gentoo}/metadata/md5-cache/sys-apps ]]; then
	echo "ERROR: refreshed live root has no complete preserved Gentoo repository." >&2
	exit 1
fi

remove_gentoo_link=0
if [[ -L ${gentoo_link} ]]; then
	if [[ "$(realpath "${gentoo_link}")" != "$(realpath "${preserved_gentoo}")" ]]; then
		echo "ERROR: ${gentoo_link} does not point at the preserved Gentoo repository." >&2
		exit 1
	fi
elif [[ -e ${gentoo_link} ]]; then
	echo "ERROR: ${gentoo_link} exists but is not the expected repository symlink." >&2
	exit 1
else
	ln -s .oxys-repositories/gentoo "${gentoo_link}"
	remove_gentoo_link=1
fi

# An unprivileged refresh already runs under fakeroot to preserve SquashFS
# ownership. Bubblewrap supplies a real uid-0 user namespace for Portage;
# clear fakeroot's preload before entering it. Override Portage's build user
# only in this temporary config because the namespace maps uid/gid 0 alone.
egencache_make_conf="${work}/egencache-make.conf"
cp "${live_root}/etc/portage/make.conf" "${egencache_make_conf}"
printf '%s\n' \
	'PORTAGE_USERNAME="root"' \
	'PORTAGE_GRPNAME="root"' \
	'FEATURES="${FEATURES} -ipc-sandbox -network-sandbox -pid-sandbox -mount-sandbox -userpriv -usersandbox"' \
	>> "${egencache_make_conf}"

generate_overlay_cache() {
	local repo=$1
	shift
	local atom category package cache_category

	for atom in "$@"; do
		category="${atom%/*}"
		package="${atom#*/}"
		cache_category="${live_root}/var/db/repos/${repo}/metadata/md5-cache/${category}"
		if [[ -d ${cache_category} ]]; then
			find "${cache_category}" -maxdepth 1 -type f -name "${package}-*" -delete
		fi
	done

	if ! env -u LD_PRELOAD -u FAKEROOTKEY -u FAKEROOT_FD_BASE \
		bwrap --die-with-parent --unshare-user --uid 0 --gid 0 \
			--bind "${live_root}" / \
			--ro-bind "${egencache_make_conf}" /etc/portage/make.conf \
			--dev /dev --proc /proc --tmpfs /run --tmpfs /tmp \
			--clearenv --setenv HOME /root \
			--setenv PATH /usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin \
			/usr/bin/egencache --update --repo="${repo}" --jobs=4 "$@"; then
		echo "ERROR: failed to generate ${repo} metadata inside the live root." >&2
		exit 1
	fi
}

generate_overlay_cache guru \
	gui-wm/niri \
	gui-apps/xwayland-satellite \
	gui-apps/wlsunset
generate_overlay_cache oxys \
	app-admin/oxys \
	gui-shells/noctalia

if (( remove_gentoo_link )); then
	rm "${gentoo_link}"
fi

required_overlay_caches=(
	"oxys/app-admin/oxys-"
	"oxys/gui-shells/noctalia-"
	"guru/gui-wm/niri-"
	"guru/gui-apps/xwayland-satellite-"
	"guru/gui-apps/wlsunset-"
)
for required in "${required_overlay_caches[@]}"; do
	repo="${required%%/*}"
	remainder="${required#*/}"
	category="${remainder%%/*}"
	prefix="${remainder#*/}"
	cache_category="${live_root}/var/db/repos/${repo}/metadata/md5-cache/${category}"
	mapfile -t cache_files < <(
		find "${cache_category}" -maxdepth 1 -type f -name "${prefix}*" -print 2>/dev/null
	)
	if (( ${#cache_files[@]} == 0 )); then
		echo "ERROR: refreshed live root has no metadata for ${repo}::${category}/${prefix%-}." >&2
		exit 1
	fi
	# A slash between a slot and subslot followed by '=' is the VDB-only
	# built slot-operator form which caused niri to be masked as invalid.
	if grep -EH '^(BDEPEND|DEPEND|IDEPEND|PDEPEND|RDEPEND)=.*:[^[:space:]()/]+/[^[:space:]()]+=' \
		"${cache_files[@]}"; then
		echo "ERROR: ${repo}::${category}/${prefix%-} cache contains a built slot operator." >&2
		exit 1
	fi
done
chown -R 0:0 "${work}/root/var/db/repos"
echo ">> verified installer metadata for GURU and Oxys overlay packages"

squash_info="$(unsquashfs -s "${disk_squash_path}")"
compression="$(awk '/^Compression / {print $2; exit}' <<<"${squash_info}")"
compression="${OXYS_SQUASHFS_COMPRESSION:-${compression}}"
case "${compression}" in
	gzip|lzo|lz4|xz|zstd) ;;
	*)
		echo "ERROR: unsupported SquashFS compressor: ${compression}" >&2
		exit 1
		;;
esac
block_size="$(awk '/^Block size / {print $3; exit}' <<<"${squash_info}")"
[[ "${block_size}" =~ ^[0-9]+$ ]] || block_size=1048576
squash_options=(-noappend -comp "${compression}" -b "${block_size}")
if grep -q '^Filesystem is not exportable' <<<"${squash_info}"; then
	squash_options+=(-no-exports)
fi
if grep -q '^Xattrs are not stored' <<<"${squash_info}"; then
	squash_options+=(-no-xattrs)
fi
if grep -q '^Tailends are packed into fragments' <<<"${squash_info}"; then
	squash_options+=(-always-use-fragments)
fi

echo ">> repacking live filesystem (${compression})"
# The extracted live root is large. Remove the old squashfs before creating
# its replacement so /tmp never has to hold two ~1.8 GiB copies at once.
rm -f "${disk_squash_path}"
mksquashfs "${work}/root" "${disk_squash_path}" \
	"${squash_options[@]}" >/dev/null

echo ">> writing refreshed bootable ISO"
volume_id="$(xorriso -indev "${input_iso}" -pvd_info 2>&1 \
	| sed -n 's/^Volume Id *: *//p' | head -n1)"
volume_id="${volume_id:-OxysOS-refresh}"

# Rebuild the hybrid image from the extracted tree. Importing an existing
# Catalyst ISO and asking xorriso to replay its boot setup loses or overlaps
# the EFI/GPT partitions on xorriso 1.5.x. These are the same hybrid GRUB
# options Catalyst used for the source image, including BIOS + UEFI boot.
xorriso -as mkisofs \
	-o "${output_iso}" \
	-V "${volume_id}" \
	--grub2-mbr "--interval:local_fs:0s-15s:zero_mbrpt,zero_gpt,zero_apm:${input_iso}" \
	--protective-msdos-label \
	-partition_cyl_align off \
	-partition_offset 0 \
	-partition_hd_cyl 114 \
	-partition_sec_hd 32 \
	--mbr-force-bootable \
	-apm-block-size 2048 \
	-hfsplus \
	-efi-boot-part --efi-boot-image \
	-c /boot.catalog \
	-b /boot/grub/i386-pc/eltorito.img \
	-no-emul-boot -boot-load-size 4 -boot-info-table --grub2-boot-info \
	-eltorito-alt-boot \
	-e /efi.img -no-emul-boot \
	"${work}/iso-root" >/dev/null 2>&1

boot_report="$(xorriso -indev "${output_iso}" \
	-report_el_torito plain -report_system_area plain 2>&1)"
if ! grep -q 'El Torito boot img.*BIOS.* y ' <<<"${boot_report}" \
	|| ! grep -q 'El Torito boot img.*UEFI.* y ' <<<"${boot_report}" \
	|| ! grep -q 'GPT partition path.* /efi.img' <<<"${boot_report}"; then
	echo "ERROR: refreshed ISO failed BIOS/UEFI boot validation; removing it." >&2
	rm -f "${output_iso}"
	exit 1
fi

output_volume_id="$(xorriso -indev "${output_iso}" -pvd_info 2>&1 \
	| sed -n 's/^Volume Id *: *//p' | head -n1)"
if [[ "${output_volume_id}" != "${volume_id}" ]]; then
	echo "ERROR: refreshed ISO label '${output_volume_id}' does not match source label '${volume_id}'; removing it." >&2
	rm -f "${output_iso}"
	exit 1
fi

input_size="$(stat -c%s "${input_iso}")"
output_size="$(stat -c%s "${output_iso}")"
if (( output_size < input_size / 2 )); then
	echo "ERROR: refreshed ISO is implausibly small; removing it." >&2
	rm -f "${output_iso}"
	exit 1
fi

sha256sum "${output_iso}" > "${output_iso}.sha256"
echo ">> done: ${output_iso}"
echo "   sha256: $(cut -d' ' -f1 "${output_iso}.sha256")"
