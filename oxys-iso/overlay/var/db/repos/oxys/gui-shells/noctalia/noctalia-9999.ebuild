# Copyright 2024 Gentoo Authors
# Distributed under the terms of the GNU General Public License v2

EAPI=8

inherit meson git-r3 xdg flag-o-matic

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

# System (non-vendored) dependencies. This list is derived from the unconditional
# dependency('...') calls in the pinned commit's meson.build -- that revision has
# NO system_* toggles (only `tests` and `jemalloc` options), so every dep below
# must be present or configure dies with `Dependency "<x>" not found`. Only Luau,
# dr_wav, fzy and Material Color Utilities remain vendored in the checkout; md4c,
# toml++, nlohmann_json, harfbuzz and wireplumber are resolved via pkg-config.
# libglvnd provides the EGL/GLESv2 the shell renders against (satisfying the
# egl/glesv2 lookups so the epoxy fallback branch is never taken); dev-libs/wayland
# provides wayland-egl.
RDEPEND="
	dev-cpp/nlohmann_json
	dev-cpp/sdbus-c++
	dev-cpp/tomlplusplus
	dev-libs/glib:2
	dev-libs/libxml2:=
	dev-libs/md4c
	dev-libs/wayland
	gnome-base/librsvg
	media-libs/fontconfig
	media-libs/freetype
	media-libs/harfbuzz
	media-libs/libglvnd
	media-libs/libwebp:=
	media-video/pipewire
	media-video/wireplumber
	net-misc/curl
	sci-libs/libqalculate
	sys-auth/polkit
	sys-libs/pam
	x11-libs/cairo
	x11-libs/libxkbcommon
	x11-libs/pango
	jemalloc? ( dev-libs/jemalloc )
"
# meson gates on `cc.has_header('stb/stb_image_resize2.h')` and stb_image_write.h
# (upstream expects SYSTEM stb -- there is no .gitmodules and third_party/ has no
# stb). Gentoo ships NO stb package, so the two public-domain headers are vendored
# under files/stb/ (pinned to nothings/stb 31c1ad3, 2026-04-15) and put on the
# include path in src_configure. No system dep, no distfile digest, no network.
DEPEND="${RDEPEND}"
# wayland-scanner is find_program()'d at configure time; on modern Gentoo it is
# split out of dev-libs/wayland into its own dev-util/wayland-scanner package.
BDEPEND="
	dev-libs/wayland-protocols
	dev-util/wayland-scanner
	virtual/pkgconfig
"

src_configure() {
	# meson.eclass builds with --buildtype plain and honours the profile's
	# CFLAGS, so upstream's release-only -march=native is not applied -- good,
	# because Oxys ships multi-arch artifacts and must not bake in native.
	#
	# Put the vendored stb headers on the include path. meson feeds CFLAGS/CXXFLAGS
	# to the compiler for both cc.has_header() probes and the real C++ build, so an
	# -I at ${FILESDIR} makes `#include <stb/...>` resolve to files/stb/*.h. (meson
	# ignores CPPFLAGS, so this must go in C/CXXFLAGS via append-flags.)
	append-flags "-I${FILESDIR}"
	local emesonargs=(
		-Djemalloc=$(usex jemalloc enabled disabled)
		-Dtests=disabled
	)
	meson_src_configure
}
