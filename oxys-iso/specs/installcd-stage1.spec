# OxysOS — catalyst livecd-stage1 spec
# ---------------------------------------------------------------------------
# Purpose: take a Gentoo stage3 seed and merge the package set we want present
# in the live environment, producing a "livecd-stage1" chroot that stage2 then
# turns into a bootable ISO.
#
# Tokens (@TIMESTAMP@ etc.) are NOT understood by catalyst itself — they are
# substituted by build.sh before catalyst runs (mirrors how gentoo/releng's
# tooling templates these). build.sh keeps @TIMESTAMP@ identical across both
# specs so stage2 can find stage1's output.
#
# Verify field semantics against:
#   https://github.com/gentoo/releng/tree/master/releases/specs/amd64
#   (installcd-stage1.spec is the upstream analogue of this file)
# ---------------------------------------------------------------------------

subarch: amd64
version_stamp: @TIMESTAMP@
target: livecd-stage1
rel_type: 23.0-default

# glibc, OpenRC, no-multilib. This is exactly what the official minimal
# install CD uses, so the whole package set is known-good.
profile: default/linux/amd64/23.0/no-multilib

# A gentoo repo snapshot id. Create it once with `catalyst -s <treeish>` and
# pass the same value here. build.sh substitutes @TREEISH@.
# VERIFY: exact snapshot semantics vary by catalyst version (treeish vs named
# snapshot). See `man catalyst` on your installed version.
snapshot_treeish: @TREEISH@

# The seed stage3. build.sh downloads the current stage3-openrc tarball and
# drops it here as ...-seed.tar.xz so this path is stable (no timestamp churn).
source_subpath: 23.0-default/stage3-amd64-openrc-seed

compression_mode: pixz

# Our portage config overlay: enables binhost (getbinpkg) + ZFS USE flags.
# build.sh copies the static config, adds generated offline Git-source mappings,
# and substitutes @PORTAGE_CONFDIR@ with that per-build directory.
portage_confdir: @PORTAGE_CONFDIR@

# Extra ebuild repositories mounted into the build chroot so we can BAKE the
# default desktop into the image (rsync'd to the target -- the flawless path;
# on-target emerge is then only for packages a user adds). GURU carries niri +
# the small Wayland tools; the oxys overlay carries gui-shells/noctalia and the
# Portage-owned app-admin/oxys CLI. Both also ship to the target via the root
# overlay (see /var/db/repos in overlay/) so user-added overlay packages can
# build there too. GURU is cloned into the overlay by
# build-installer-overlay.sh before catalyst runs.
repos: @REPO_DIR@/overlay/var/db/repos/guru @REPO_DIR@/overlay/var/db/repos/oxys

# USE flags for everything merged into the live env.
livecd/use:
	livecd
	unicode
	fbcon
	# Catalyst composes THIS list into the make.conf USE for the stage1 package
	# merge, and it OVERRIDES portage_confdir/make.conf's USE -- so the global
	# binhost-friendly desktop baseline from portage_confdir/make.conf must be
	# repeated here. OpenRC is the init system (profile + sysvinit from the base
	# stage3). `elogind` (NOT `systemd`) MUST be the session/logind backend:
	# `USE=systemd` is not merely "library integration" -- it makes logind
	# consumers (dbus, polkit, NetworkManager) depend on sys-apps/systemd, which
	# then owns /sbin/init and boots systemd instead of OpenRC (killing the
	# /etc/inittab autologin and every OpenRC rcadd service). `-systemd elogind`
	# keeps sys-apps/systemd out and OpenRC in charge.
	# Keep Qt package-local: global qt6 pulls KDE Frameworks through unrelated
	# packages such as pinentry, which the niri desktop does not need.
	X
	wayland
	dbus
	-systemd
	elogind
	policykit
	alsa
	pipewire
	pulseaudio
	vulkan
	opengl
	gtk
	jpeg
	png
	webp
	svg
	fontconfig
	harfbuzz
	udev
	ssl
	threads

# Packages merged into the live environment.
# Binary packages are pulled where available via FEATURES="getbinpkg" set in
# portage_confdir/make.conf (catalyst has no per-spec --getbinpkg flag; it is
# a portage FEATURE — see README).
livecd/packages:
	# --- partitioning / filesystem creation (the installer drives these) ---
	sys-block/parted
	sys-apps/gptfdisk
	sys-apps/util-linux
	sys-block/zram-init
	sys-fs/dosfstools
	sys-fs/e2fsprogs

	# --- bootloader install: the installer runs `grub-install
	#     --target=x86_64-efi` from the LIVE env against the mounted target,
	#     so grub-install must exist here. GRUB is the default (and only
	#     non-systemd) bootloader. Requires GRUB_PLATFORMS="efi-64" in
	#     portage_confdir/make.conf so the x86_64-efi modules are actually
	#     built -- without that, grub-install has no platform to install.
	#     --removable writes the fallback \EFI\BOOT\BOOTX64.EFI path, so no
	#     NVRAM writes and efibootmgr is not required. ---
	sys-boot/grub

	# --- device-mapper: dracut's dmsquash-live module (the live squashfs
	#     root) depends on the "dm" module, which dracut can only install if
	#     dmsetup/libdevicemapper are present. Without this, stage2's dracut
	#     run fails with "Module 'dmsquash-live' depends on module 'dm',
	#     which can't be installed." ---
	sys-fs/lvm2

	# --- initramfs builder: stage2 invokes dracut to wrap the dist-kernel ---
	sys-kernel/dracut

	# --- ZFS userland runtime dep: the zfs CLI (zpool/zfs) is NOT emerged on
	#     the ISO -- it's injected as a per-package tarball from oxys-build (see
	#     installcd-stage2.spec livecd/root_overlay). That tarball is scoped to
	#     sys-fs/zfs's OWN files only, so its runtime dependencies do not ride
	#     along. libnvpair.so.3 links against libtirpc.so.3 (glibc dropped its
	#     sunrpc/XDR implementation in 2.34, so ZFS uses libtirpc for XDR), and
	#     nothing else on the live image pulls it -- without this, `zpool` dies
	#     at first use with "libtirpc.so.3: cannot open shared object file" and
	#     the install aborts on the first `zpool create`. libtirpc has no kernel
	#     coupling, so unlike zfs-kmod it belongs here in stage1. ---
	net-libs/libtirpc

	# --- config compiler: the installer builds the user's Rust config into a
	#     manifest ON the live system (oxys::compile), so the ISO must ship a
	#     Rust toolchain plus a C linker. rust-bin is the prebuilt toolchain,
	#     which avoids compiling rust itself inside catalyst. Deps are vendored
	#     into /usr/src/oxys/vendor by build-installer-overlay.sh for fully
	#     offline builds on the target. ---
	dev-lang/rust-bin
	sys-devel/gcc

	# --- OxysOS system manager: built as a static musl CLI and staged in the
	#     canonical first-party overlay before catalyst starts. Emerging it here
	#     installs /usr/bin/oxys with a real app-admin/oxys VDB entry, which is
	#     required for updater self-bootstrapping and safe re-exec. ---
	app-admin/oxys

	# --- user provisioning: the installer creates the configured user
	#     accounts on the target and writes /etc/sudoers.d/wheel for any
	#     wheel member. sudo must therefore be present in the live env, since
	#     the target root is an rsync copy of it. ---
	app-admin/sudo

	# --- live networking: NetworkManager + nmtui gives the installer console
	#     a real SSID/password path, while dhcpcd stays available as a small
	#     manual fallback. NetworkManager pulls the right backend pieces in most
	#     cases, but dbus and wpa_supplicant are explicit because live boot must
	#     not depend on an implicit dependency shape. ---
	sys-apps/dbus
	net-misc/networkmanager
	net-wireless/wpa_supplicant
	net-wireless/iw
	net-misc/dhcpcd

	# --- firmware: mandatory for real live-install hardware. Intel iwlwifi,
	#     Realtek, Broadcom, MediaTek, storage, GPU, and many USB adapters need
	#     blobs even when the kernel driver itself is present. ---
	sys-kernel/linux-firmware

	# --- installer-invoked host tools (the installer shells out to these from
	#     the LIVE env). rsync copies the live system onto the target;
	#     app-editors/nano backs the TUI's "edit config" action
	#     (oxys-installer suspends the TUI and runs `nano`). Both usually ride
	#     along in the stage3 seed, but pin them so a seed change can't silently
	#     drop them. ---
	net-misc/rsync
	app-editors/nano

	# --- EFI bootloader extras: the installer uses `grub-install --removable`
	#     (fallback \EFI\BOOT path, no NVRAM) so efibootmgr is not strictly
	#     required, but shipping it allows non-removable installs and manual
	#     NVRAM boot-entry repair from the live env. ---
	sys-boot/efibootmgr

	# --- filesystem tooling for the disk layouts / encryption the config model
	#     exposes and for rescue use. cryptsetup covers the Encryption/LUKS
	#     paths; btrfs/xfs progs cover those DiskLayout variants and the kernel's
	#     enabled filesystems. ---
	sys-fs/cryptsetup
	sys-fs/btrfs-progs
	sys-fs/xfsprogs

	# --- live-environment quality-of-life / remote + hardware debugging.
	#     openssh lets you drive an install remotely (and the target enables
	#     sshd); pciutils/usbutils identify hardware; gentoolkit gives equery et
	#     al; git + curl fetch configs/assets. ---
	net-misc/openssh
	sys-apps/pciutils
	sys-apps/usbutils
	app-portage/gentoolkit
	dev-vcs/git
	net-misc/curl
	app-misc/fastfetch

	# --- default desktop stack, BAKED into the image (rsync'd to the target, so
	#     the default install needs no live emerge). Built here against the
	#     wayland-enabled make.conf, using the GURU + oxys overlays (see `repos:`).
	#     KEEP IN SYNC with oxys-installer/configs/desktop.fe2o3. ---
	# core apps / browser + audio server
	www-client/firefox-bin
	media-video/pipewire
	media-video/wireplumber
	# wayland compositor (GURU) + shell (oxys overlay)
	gui-wm/niri
	gui-shells/noctalia
	# session plumbing
	sys-auth/seatd
	sys-auth/polkit
	app-crypt/p11-kit
	sys-apps/xdg-desktop-portal
	sys-apps/xdg-desktop-portal-gtk
	x11-misc/xdg-user-dirs
	sys-fs/udisks
	gnome-base/gvfs
	# terminals + wayland tools
	gui-apps/foot
	gui-apps/wl-clipboard
	gui-apps/xwayland-satellite
	gui-apps/wlsunset
	x11-base/xwayland
	# power / hardware
	sys-power/power-profiles-daemon
	app-misc/ddcutil
	# fonts + icon theme
	media-fonts/noto
	media-fonts/noto-emoji
	x11-themes/papirus-icon-theme

# NOTE on ZFS: userspace tools + kmod are NOT merged here. There is no kernel
# in stage1, so zfs-kmod (USE=dist-kernel) cannot build yet. ZFS is added in
# stage2 via boot/kernel/oxys/packages, where the dist-kernel exists. This
# matches the upstream installcd, which also installs ZFS only in stage2.
