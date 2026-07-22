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
# version-skew bug oxys-build's metadata/vermagic pairing exists to prevent,
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

portage_confdir: @PORTAGE_CONFDIR@

# ---- live medium settings ----
livecd/fstype: squashfs
# Keep the live root on XZ until the injected kernel's BMI2-optimized Zstandard
# decoder is safe on x86-64-v3/KVM. CONFIG_SQUASHFS_ZSTD alone is not enough:
# the 6.18.38-oxys decoder currently oopses in FSE_decompress_wksp_body_bmi2
# while mounting a Zstandard image, after which dracut misleadingly reports
# that it could not find image.squashfs. Catalyst passes these options through
# to gensquashfs as `${clst_fsops}`.
livecd/fsops: --compressor xz --block-size 1048576
livecd/iso: oxysos-amd64-@TIMESTAMP@.iso
livecd/volid: OxysOS-amd64-@DATESTAMP@
# dokeymap = prompt/allow keymap selection. Do not pass nodhcp: the live
# environment starts NetworkManager below, and suppressing DHCP here makes the
# boot-time networking story ambiguous.
# Keep tty0 as the primary console for the graphical boot while also emitting
# kernel/OpenRC output to ttyS0, where fsscript enables a recovery getty for the
# QEMU runner's host terminal.
livecd/bootargs: dokeymap console=ttyS0,115200 console=tty0
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
# kmerge.sh/kerncache path below). The first dir is a root:root-normalised copy
# of overlay/ that build.sh stages (@OVERLAY_DIR@), NOT the uid-1000 git
# checkout -- `rsync -a` preserves ownership, and shipping the checkout directly
# leaves /var owned by uid 1000 on the live root (see build.sh for the details).
livecd/root_overlay: @OVERLAY_DIR@ @ZFS_OVERLAY@
# fsscript: shell script run *inside* the chroot after root_overlay is applied.
# It does the autologin + zfs-autoload wiring (see fsscript/fsscript.sh).
livecd/fsscript: @REPO_DIR@/fsscript/fsscript.sh

# Apply the OxysOS nodename and expose the preserved Gentoo snapshot during the
# boot runlevel, then start the live networking stack. This is ISO-specific
# OpenRC wiring; it is separate from whatever services the installed-system
# manifest enables later.
# NetworkManager provides `nmtui` on the console for WiFi SSID/password entry.
# Start ModemManager explicitly so its conf.d ordering and log redirection take
# effect instead of letting D-Bus activate it with the graphical tty attached.
# The live installer needs D-Bus and NetworkManager, and starts ModemManager
# explicitly so mobile-broadband probing cannot be D-Bus-activated later with
# its output attached to tty1. Do not start nftables on the ephemeral live
# medium: installed profiles independently reconcile authoritative OpenRC
# runlevels and enable nftables on the target.
livecd/rcadd: hostname|boot oxys-gentoo-repo|boot dbus|default modemmanager|default NetworkManager|default bluetooth|default

# ---- kernel: oxys-build's own published kernel + zfs-kmod, injected by
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
# zfs-kmod are unpacked from oxys-build's own published tarballs instead (see
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

# ---- size trim: drop docs only, KEEP the build toolchain ----
# CRITICAL: the installer rsyncs this live root to the target *verbatim* (see
# install/exec.rs::rsync_args -- excludes are only runtime/cache dirs, nothing
# under /usr). So the live root IS the installed root. Anything unmerged or
# emptied here is missing on every installed system, not just the live ISO.
#
# A normal installed OxysOS is a full Gentoo where the user runs `emerge`. That
# needs the whole C build toolchain: glibc headers (/usr/include), linux-headers,
# make, patch, m4, autoconf, automake -- plus gcc/binutils. gcc/binutils were
# always kept (see below); the headers + make/patch/m4/autotools were formerly
# stripped, which left the installed system unable to compile C packages
# (missing /usr/include/stdio.h, no `make`, etc.). They are kept now too. The
# incremental size is small next to the gcc/binutils already retained.
#
# DO NOT add sys-devel/gcc or sys-devel/binutils here either. Beyond the emerge
# use above, OxysOS compiles the user's Rust config into a manifest ON the live
# system (oxys::compile -> cargo build), and rustc shells out to `cc` (gcc) as
# its linker driver, which in turn calls `ld`/`as` (binutils). Unmerging either
# breaks that on-target build with `error: linker `cc` not found`.
#
# Only pure documentation is dropped below -- it never affects compilation.
livecd/unmerge:
	sys-apps/texinfo
	sys-apps/man-db
	sys-apps/man-pages

livecd/empty:
	/var/cache
	/var/tmp
	/tmp
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
