# Copyright 2026 Oxys
# Distributed under the terms of the GNU General Public License v2

EAPI=8

inherit git-r3 meson

DESCRIPTION="Native Wayland desktop shell for compositor-based desktops"
HOMEPAGE="https://github.com/noctalia-dev/noctalia https://docs.noctalia.dev"
EGIT_REPO_URI="https://github.com/noctalia-dev/noctalia.git"

LICENSE="MIT"
SLOT="0"
KEYWORDS=""
IUSE="jemalloc"

BDEPEND="
	virtual/pkgconfig
"
DEPEND="
	dev-libs/glib:2
	dev-libs/libxml2
	dev-libs/sdbus-c++
	gui-libs/wayland
	gui-libs/wayland-protocols
	gnome-base/librsvg:2
	media-libs/fontconfig
	media-libs/freetype
	media-libs/harfbuzz
	media-libs/libglvnd
	media-libs/libwebp
	media-video/pipewire
	net-misc/curl
	sci-libs/libqalculate
	sys-auth/polkit
	sys-libs/pam
	x11-libs/cairo
	x11-libs/libxkbcommon
	x11-libs/pango
	jemalloc? ( dev-libs/jemalloc )
"
RDEPEND="${DEPEND}"

src_configure() {
	local emesonargs=(
		--buildtype=debugoptimized
		-Db_ndebug=true
		-Dtests=disabled
		$(meson_feature jemalloc)
	)

	meson_src_configure
}
