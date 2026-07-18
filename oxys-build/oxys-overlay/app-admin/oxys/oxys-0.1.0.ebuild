# Copyright 2026 OxysOS Authors
# Distributed under the terms of the GNU General Public License v2

EAPI=8

DESCRIPTION="OxysOS declarative system manager"
HOMEPAGE="https://github.com/oxydizer/oxys"
LICENSE="|| ( Apache-2.0 MIT )"
SLOT="0"
KEYWORDS="amd64"

# The release helper builds this as a static musl PIE before catalyst starts.
# Keeping the prepared binary in FILESDIR makes the ebuild deterministic and
# gives Portage ownership of /usr/bin/oxys without adding a Rust toolchain to
# the package's runtime dependency graph.
RESTRICT="strip"
QA_PREBUILT="usr/bin/oxys"
S="${WORKDIR}"

src_install() {
	local payload="${FILESDIR}/oxys-${PV}"
	local version_output

	[[ -x ${payload} ]] || die "missing staged Oxys CLI payload: ${payload}"
	version_output="$("${payload}" --version)" || die "staged Oxys CLI cannot report its version"
	[[ ${version_output} == "oxys ${PV}" ]] ||
		die "staged Oxys CLI reports '${version_output}', expected 'oxys ${PV}'"

	newbin "${payload}" oxys
}
