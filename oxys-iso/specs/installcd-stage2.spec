# OxysOS — catalyst livecd-stage2 spec
# ---------------------------------------------------------------------------
# Purpose: take the livecd-stage1 chroot, inject oxys-build's own tagged
# kernel + zfs-kmod build, build a dracut initramfs, inject the installer +
# autologin wiring, then pack everything into a squashfs and emit a bootable
# ISO.
#
# KERNEL: previously this used catalyst's own prebuilt-dist-kernel machinery
# (boot/kernel/oxys/sources: sys-kernel/gentoo-kernel-bin, plus
# boot/kernel/oxys/packages: sys-fs/zfs to build zfs-kmod against it) --
# i.e. catalyst emerged its OWN kernel and its OWN zfs-kmod, completely
# independent of oxys-build's. That's exactly the kernel/zfs-kmod
# version-skew bug oxys-build's build-id/vermagic pairing exists to prevent,
# recreated one layer up (ISO kernel vs. post-install package-pipeline
# kernel diverging).
#
# Catalyst has no spec-level "use this exact prebuilt kernel tarball, skip
# emerge entirely" flag (verified against catalyst's targets/support/kmerge.sh
# and catalyst/base/stagebase.py -- boot/kernel is a required key and
# build_kernel() unconditionally shells out to emerge something). So instead,
# ../Containerfile installs ../catalyst-overrides/kmerge.sh over catalyst's
# own copy: for this "oxys" label, it skips emerge/genkernel entirely and
# injects the exact kernel + zfs-kmod tarball pair build.sh resolved via
# scripts/resolve-kernel-build.sh (see that script and
# ../../oxys-build/podman/scripts/oxys-build-packages.sh for how those
# tarballs are tagged and vermagic-verified). Everything downstream of
# kmerge.sh (extract_kernels/extract_modules, grub.cfg generation) is
# untouched stock catalyst -- our override still produces the same kerncache
# tarball contract upstream's distkernel path does.
#
# zfs USERLAND (zpool/zfs CLI) has no kernel-version coupling, so it isn't
# emerged here either -- it rides in via the second livecd/overlay dir below,
# which build.sh populates from oxys-build's own zfs tarball.
#
# Verify against upstream installcd-stage2-minimal.spec:
#   https://github.com/gentoo/releng/tree/master/releases/specs/amd64
# ---------------------------------------------------------------------------

subarch: amd64
version_stamp: @TIMESTAMP@
target: livecd-stage2
rel_type: 23.0-default
profile: default/linux/amd64/23.0/no-multilib
snapshot_treeish: @TREEISH@

# Consume stage1's output. @TIMESTAMP@ matches stage1 (build.sh keeps them equal).
source_subpath: 23.0-default/livecd-stage1-amd64-@TIMESTAMP@

portage_confdir: @REPO_DIR@/portage_confdir

# ---- live medium settings ----
livecd/fstype: squashfs
# Push the live root image toward size over build speed. Catalyst passes this
# through to gensquashfs as `${clst_fsops}` when creating image.squashfs.
livecd/fsops: --compressor xz --block-size 1048576
livecd/iso: oxysos-amd64-@TIMESTAMP@.iso
livecd/volid: OxysOS-amd64-@DATESTAMP@
# dokeymap = prompt/allow keymap selection. Do not pass nodhcp: the live
# environment starts NetworkManager below, and suppressing DHCP here makes the
# boot-time networking story ambiguous.
livecd/bootargs: dokeymap
# No livecd/cdtar: this catalyst install doesn't ship the frosted grub theme
# tarball catalyst's stock spec examples reference. It's purely cosmetic
# (bootloader background/theme) -- catalyst falls back to a plain default
# grub.cfg without it, so the ISO still boots fine.

# ---- inject the installer + zfs userland + wire up autologin/autostart ----
# root_overlay: rsync'd verbatim onto the live root filesystem (the chroot),
# BEFORE fsscript runs. Do NOT use livecd/overlay for this: that key is applied
# by setup_overlay() in the finish sequence onto target_path -- the ISO's own
# directory tree next to the squashfs -- so files never reach the live root and
# fsscript can't see them. Two dirs (split on whitespace): the committed
# overlay/ (carries the oxys-installer binary) and a per-build scratch dir
# build.sh populates from oxys-build's zfs userland tarball (zpool/zfs CLI +
# libs -- no kernel-version coupling, so it doesn't need to go through the
# kmerge.sh/kerncache path below).
livecd/root_overlay: @REPO_DIR@/overlay @ZFS_OVERLAY@
# fsscript: shell script run *inside* the chroot after root_overlay is applied.
# It does the autologin + zfs-autoload wiring (see fsscript/fsscript.sh).
livecd/fsscript: @REPO_DIR@/fsscript/fsscript.sh

# Start the live networking stack. This is ISO-specific OpenRC wiring; it is
# separate from whatever services the installed-system manifest enables later.
# NetworkManager provides `nmtui` on the console for WiFi SSID/password entry.
livecd/rcadd: dbus|default NetworkManager|default

# ---- kernel: oxys-build's own tagged kernel + zfs-kmod, injected by
#      ../catalyst-overrides/kmerge.sh (zero compilation, zero emerge) ----
boot/kernel: oxys

# Required by catalyst even though our override doesn't emerge anything for
# this label: it's what makes iso-bootloader-setup.sh generate the
# dracut-style grub.cfg stanza (search+linux+initrd via dmsquash-live),
# matching how our override's dracut invocation actually builds the
# initramfs below.
boot/kernel/oxys/distkernel: yes

# No boot/kernel/oxys/sources or /packages here on purpose -- our
# kmerge.sh override never emerges anything for this label; the kernel and
# zfs-kmod are unpacked from oxys-build's own tagged tarballs instead (see
# scripts/resolve-kernel-build.sh and catalyst-overrides/kmerge.sh).

# dracut initramfs args -- still needed and still built here: this is the
# LIVE ISO's boot initramfs (dmsquash-live etc.), which is inherently
# ISO-specific and isn't something oxys-build has any notion of. Our
# kmerge.sh override reads this same variable and runs dracut against the
# injected kernel. dmsquash-live = boot the squashfs live image. We do NOT
# add `-a zfs`: root is squashfs, not a ZFS pool, so zfs is not needed in the
# initramfs. The zfs *module* is loaded later in userspace, before the
# installer starts (see fsscript.sh).
boot/kernel/oxys/dracut_args: --xz --no-hostonly -a dmsquash-live -o btrfs -o crypt -o i18n -o qemu -o qemu-net -o nvdimm -o multipath

# ---- size trim: drop *most* of the build toolchain from the live image ----
# This is a trimmed subset of the upstream installcd unmerge list. It keeps the
# ISO small. The standalone installer is built before this stage and copied in
# via overlay.
#
# DO NOT add sys-devel/gcc or sys-devel/binutils here. Unlike the upstream
# installcd, OxysOS compiles the user's Rust config into a manifest ON the live
# system (oxys::compile -> cargo build), and rustc shells out to `cc` (gcc) as
# its linker driver, which in turn calls `ld`/`as` (binutils). Unmerging either
# breaks that on-target build with `error: linker `cc` not found`. The pure-Rust
# vendored crates (see build-installer-overlay.sh) need no C headers/make, so the
# rest of the toolchain below can still go -- only the linker must stay.
livecd/unmerge:
	dev-build/autoconf
	dev-build/automake
	dev-build/make
	sys-devel/m4
	sys-devel/patch
	sys-kernel/linux-headers
	app-portage/gentoolkit
	sys-apps/texinfo
	sys-apps/man-db
	sys-apps/man-pages

livecd/empty:
	/var/cache
	/var/tmp
	/tmp
	/usr/include
	/usr/share/man
	/usr/share/doc
	/usr/share/info
	/var/log

# Do NOT blanket-remove /usr/lib*/*.a here. On glibc >=2.34, libpthread, librt,
# libdl and libutil were folded into libc; glibc keeps shipping them as *empty
# stub archives* (libpthread.a etc.) purely so `-lpthread`/`-lrt`/`-ldl`/`-lutil`
# still resolve. rustc's std passes all four of those `-l` flags unconditionally,
# and no matching `.so` dev files exist anymore -- so deleting the stub archives
# breaks the on-target Rust link with `unable to find library -lutil` (and rt,
# pthread, dl). The stubs are empty, so keeping them costs essentially nothing.
# .la (libtool) archives are unrelated to Rust and still safe to drop.
livecd/rm:
	/usr/lib*/*.la
	/root/.bash_history
	/etc/resolv.conf
