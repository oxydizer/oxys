# Copyright 2024 Gentoo Authors
# Distributed under the terms of the GNU General Public License v2

EAPI=8

inherit meson git-r3 xdg

DESCRIPTION="Lightweight Wayland shell built directly on Wayland and OpenGL ES"
HOMEPAGE="https://github.com/noctalia-dev/noctalia"
EGIT_REPO_URI="https://github.com/noctalia-dev/noctalia.git"

# noctalia has no final v5 tag yet (latest is v5.0.0-beta1) and upstream moves
# fast, so this tracks git like the reference AUR `noctalia-git` package. The
# empty KEYWORDS marks it a live ebuild -- the config must request it with the
# `**` keyword (Package::new("gui-shells/noctalia").keywords(["**"])).
LICENSE="MIT"
SLOT="0"
KEYWORDS=""

# jemalloc is glibc-only per upstream meson.build; Oxys is glibc, so default on.
IUSE="+jemalloc"

# System (non-vendored) dependencies, mirroring upstream's own dependency set
# (verified against the working noctalia-git Arch package). Everything else
# (Luau, nlohmann_json, md4c, toml++, dr_wav, fzy, Material Color Utilities) is
# vendored in the source tree and built from the checkout. libglvnd provides the
# EGL/GLESv2 that the shell renders against.
RDEPEND="
	dev-libs/glib:2
	dev-libs/libxml2:=
	dev-cpp/sdbus-c++
	dev-libs/wayland
	gnome-base/librsvg
	media-libs/fontconfig
	media-libs/freetype
	media-libs/libglvnd
	media-libs/libwebp:=
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
DEPEND="${RDEPEND}"
BDEPEND="
	dev-libs/wayland-protocols
	virtual/pkgconfig
"

src_configure() {
	# meson.eclass builds with --buildtype plain and honours the profile's
	# CFLAGS, so upstream's release-only -march=native is not applied -- good,
	# because Oxys ships multi-arch artifacts and must not bake in native.
	local emesonargs=(
		-Djemalloc=$(usex jemalloc enabled disabled)
		-Dtests=disabled
	)
	meson_src_configure
}
