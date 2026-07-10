# OxysOS ISO build

Minimal Gentoo-based, OpenRC, GUI-free bootable ISO with a TUI installer
(`oxys-installer`) that autostarts on tty1. Built with [catalyst].

## Layout

```
oxys-iso/
├── build.sh                       # runs both catalyst stages in order
├── Containerfile                  # Gentoo stage3 + catalyst pre-installed
│                                   # (+ catalyst-overrides/kmerge.sh baked in)
├── catalyst-overrides/
│   └── kmerge.sh                  # replaces catalyst's own kmerge.sh: injects
│                                   # oxys-build's tagged kernel+zfs-kmod instead
│                                   # of emerging/compiling one
├── scripts/
│   ├── enter-container.sh         # build+enter the catalyst container (rootful)
│   ├── resolve-kernel-build.sh    # finds+verifies the oxys-build kernel/zfs-kmod
│   │                               # pair for OXYS_ARCH/OXYS_KERNEL_BUILD_ID
│   └── run-qemu.sh                # boot a built ISO in QEMU
├── specs/
│   ├── installcd-stage1.spec      # target: livecd-stage1  (live package set)
│   └── installcd-stage2.spec      # target: livecd-stage2  (kernel+initramfs+ISO)
├── fsscript/
│   └── fsscript.sh                # autologin + zfs autoload wiring (runs in chroot)
├── overlay/                       # copied verbatim onto the live root fs
│   └── usr/local/bin/
│       └── oxys-installer         # <-- generated installer binary
└── portage_confdir/               # /etc/portage for the build chroot
    └── make.conf                  # getbinpkg binhost + no-GUI USE
```

## Prebuilt kernel dependency

`oxys-build` (the sibling directory in this monorepo) must be run **first**
for the target arch, producing a build-id-tagged, vermagic-verified kernel +
zfs-kmod + zfs tarball set under `../oxys-build/output/<arch>/`. `build.sh`
consumes that output directly — catalyst's own kernel-build step is disabled
entirely for stage2 (see `specs/installcd-stage2.spec` and
`catalyst-overrides/kmerge.sh`), so the ISO's kernel is byte-for-byte the same
one `oxys-build` produced, not a second, independently re-derived one. This is
what prevents the ISO's kernel and the post-install package pipeline's kernel
from silently drifting apart.

Two environment variables control which build gets used:

- **`OXYS_ARCH`** (required, no default) — which `../oxys-build/output/<arch>/`
  to pull from, e.g. `alderlake`. `build.sh` fails fast and lists available
  arches if this isn't set; a hardware-targeted kernel build shouldn't have a
  silent default.
- **`OXYS_KERNEL_BUILD_ID`** (optional) — a specific build-id to use instead of
  the latest. Defaults to the contents of `../oxys-build/output/<arch>/build-id`
  (oxys-build's own "current build" pointer).

`build.sh` fails fast — before catalyst even starts — if no valid, paired
kernel+zfs-kmod build exists for the requested arch/build-id (see
`scripts/resolve-kernel-build.sh`), rather than failing deep inside a stage2
run. It never falls back to letting catalyst build its own kernel.

zfs *userland* (the `zpool`/`zfs` CLI) has no kernel-version coupling, so it's
also pulled from oxys-build's own tarball (for full consistency) but delivered
via a plain `livecd/overlay` dir rather than the kernel-injection path.

## Key design decisions

- **`livecd-stage1` / `livecd-stage2`**, not `stage1`/`stage2`. The plain
  stages build install tarballs; the `livecd-*` targets are the ones that
  produce a bootable squashfs ISO.
- **No-compile, no-emerge kernel.** catalyst has no spec-level "use this exact
  prebuilt kernel tarball" flag — `boot/kernel` always triggers a real `emerge`
  (verified against catalyst's `targets/support/kmerge.sh` and
  `catalyst/base/stagebase.py`). So `catalyst-overrides/kmerge.sh` replaces
  catalyst's own copy in the build image: for stage2's kernel label, it skips
  emerge/genkernel entirely and unpacks the exact kernel + zfs-kmod tarball
  pair `oxys-build` produced (see "Prebuilt kernel dependency" above), then
  runs dracut for the live-ISO initramfs and hands catalyst the same kerncache
  tarball contract its own distkernel path would have — everything downstream
  (`extract_kernels`/`extract_modules`, grub.cfg generation) is untouched
  stock catalyst.
- **ZFS.** The kmod is injected alongside the kernel (same tarball pairing,
  same build_id — see above); it is not in the initramfs (root is squashfs,
  not a pool) — the module is loaded in userspace before the installer runs
  (see below). zfs userland (`zpool`/`zfs` CLI) rides in via `livecd/overlay`
  from oxys-build's own zfs tarball, since it has no kernel-version coupling.
- **ZFS-before-installer ordering** is guaranteed two ways: OpenRC's `modules`
  service loads `zfs` in the *boot* runlevel (before the tty1 getty in the
  *default* runlevel), and `/root/.bash_profile` runs `modprobe zfs`
  synchronously right before `exec`-ing the installer.
- **Autologin/autostart:** Gentoo+OpenRC uses sysvinit as PID 1, so the tty1
  line in `/etc/inittab` is rewritten to `agetty --autologin root`, and root's
  `.bash_profile` execs `oxys-installer` on tty1 only. If it exits, agetty
  respawns it (kiosk behavior). tty2–6 keep normal logins for recovery.
- **GUI-free:** `USE="-X -wayland -gtk -gnome -kde -qt5 -qt6 -gui -fonts"`.

## Prerequisites on the binary

`scripts/enter-container.sh build` refreshes this automatically before entering
the catalyst container. The installer is intentionally built as a standalone
musl binary so it does not depend on live-userland shared libraries. To do it
manually:

```sh
rustup target add x86_64-unknown-linux-musl
cargo build --manifest-path ../oxys-installer/Cargo.toml \
   --release --target x86_64-unknown-linux-musl
cp ../oxys-installer/target/x86_64-unknown-linux-musl/release/oxys-installer \
   ./overlay/usr/local/bin/oxys-installer
```

---

## Running catalyst from Arch Linux

**Recommendation: a privileged `podman` container running a Gentoo stage3.**

Why a container over chroot/nspawn:

- **Isolation & reproducibility** — catalyst wants a full Gentoo userland
  (portage, specific tool versions). A container keeps that entirely off your
  Arch host; teardown is `podman rm`. A bare chroot leaks mounts/state into the
  host and is fiddlier to clean.
- **`--privileged` covers catalyst's needs** — catalyst uses loop devices,
  `mknod`, bind mounts, and squashfs/ISO tooling. Privileged + the bind-mounted
  catalyst storage is enough; you don't hand-craft device passthrough.
- **systemd-nspawn** is a fine alternative if you prefer it (it's basically the
  same stage3, just managed by nspawn) — but on Arch, podman is the lowest-fuss
  path. Plain `chroot` works too but you manage `/proc`,`/sys`,`/dev` mounts and
  cleanup yourself.

### Quick path (recommended)

A `Containerfile` bakes catalyst (plus the required `~amd64` keyword and USE
flags) into a reusable image, and `scripts/enter-container.sh` builds it once,
then enters it rootful with loop passthrough and the correct bind mounts:

```sh
sudo pacman -S --needed podman

./scripts/enter-container.sh          # first run builds the image (~emerge catalyst)

# ---- now inside the container ----
catalyst -s stable                    # create the repo snapshot (first time only)
OXYS_ARCH=alderlake /oxys/oxys-iso/build.sh   # downloads the seed if missing, then builds
```

Or drive the whole build non-interactively (still creates the snapshot on first
run inside if needed):

```sh
OXYS_ARCH=v3 ./scripts/enter-container.sh build    # runs /oxys/oxys-iso/build.sh in the container
```

The image persists, so subsequent runs skip the catalyst compile entirely. The
host `~/catalyst` bind mount holds the seed, snapshot, and output across runs.
`OXYS_REBUILD=1 ./scripts/enter-container.sh` rebuilds the image (e.g. to pick
up a newer catalyst).

The ISO lands in `~/catalyst/builds/23.0-default/oxysos-amd64-<timestamp>.iso`.

### What the wrapper does (manual equivalent)

If you'd rather run the steps by hand, or need to debug the environment:

```sh
# 1. On the Arch host. podman won't auto-create bind-mount sources, so the
#    storage dir must exist first (else: statfs ~/catalyst: no such file...).
mkdir -p ~/catalyst/{builds,packages,snapshots,tmp,kerncache}

# 2. Enter a Gentoo stage3 container. Run it ROOTFUL (sudo) with loop-device
#    passthrough: catalyst mounts its squashfs snapshot (and builds the ISO)
#    via loop devices, which rootless podman CANNOT do even with --privileged
#    (loop setup needs CAP_SYS_ADMIN in the initial user namespace). Symptom of
#    getting this wrong: "Couldn't mount .../gentoo-*.sqfs, Loopdev setup failed".
#    NOTE: under sudo, ~ is /root, so use the absolute /home/<you>/catalyst path.
#    Mount the MONOREPO ROOT at /oxys (not just oxys-iso/), so build.sh can
#    also reach /oxys/oxys-build/output/<arch>/ for the prebuilt kernel and
#    zfs-kmod tarballs. This repo's build.sh now lives at /oxys/oxys-iso/build.sh.
sudo podman run --privileged --rm -it \
  --device /dev/loop-control -v /dev:/dev \
  -v /path/to/oxys:/oxys:Z \
  -v "${HOME}/catalyst:/var/tmp/catalyst:Z" \
  docker.io/gentoo/stage3:amd64-openrc bash

# ---- everything below runs INSIDE the container ----

# 3. Sync portage and install catalyst. catalyst is ~amd64 and pulls two USE
#    flags; package.accept_keywords / package.use are DIRECTORIES in the stage3,
#    so write into files inside them (not to them directly).
emerge-webrsync
echo 'dev-util/catalyst ~amd64' > /etc/portage/package.accept_keywords/catalyst
printf '%s\n' '>=sys-apps/util-linux-2.41.4-r1 python' \
              '>=sys-boot/grub-2.14-r5 grub_platforms_efi-32' \
  > /etc/portage/package.use/catalyst
emerge -j dev-util/catalyst

# 4. The default catalyst.conf already sets storedir="/var/tmp/catalyst",
#    which is where ~/catalyst is bind-mounted. No edit needed.

# 5. SEED: build.sh auto-downloads the current stage3-openrc as
#    /var/tmp/catalyst/builds/23.0-default/stage3-amd64-openrc-seed.tar.xz
#    if it is missing, so you normally don't do this by hand.

# 6. Create a repo snapshot (id must match OXYS_TREEISH / @TREEISH@; default "stable")
catalyst -s stable

# 7. Build (OXYS_ARCH is required — see "Prebuilt kernel dependency" below)
OXYS_ARCH=alderlake /oxys/oxys-iso/build.sh
```

## Running the ISO in QEMU

After a build, boot the newest ISO from the default catalyst output directory:

```sh
./scripts/run-qemu.sh
```

Useful variants:

```sh
./scripts/run-qemu.sh bios
./scripts/run-qemu.sh disk=4G
./scripts/run-qemu.sh no-disk
./scripts/run-qemu.sh persist
./scripts/run-qemu.sh /path/to/oxysos-amd64.iso
```

By default, the script creates and attaches an `8G` install target disk named
`oxys-install.qcow2` next to the selected ISO. Set
`OXYS_DISK_SIZE=4G`, pass `disk=4G`, or set `OXYS_DISK=/path/to/disk.qcow2`
to override it.

The QEMU display defaults to `virtio-vga-gl` with SDL OpenGL enabled, which is
the path to test a Niri/Wayland install. Use `GL=0 ./scripts/run-qemu.sh` only
for serial/headless boot debugging.

Set `OXYS_ISO_DIR=/path/to/builds` if your ISO output lives somewhere else.

> Loop devices: the `sudo` (rootful) + `--device /dev/loop-control -v /dev:/dev`
> in the `podman run` line above are what make squashfs snapshot mounts and the
> ISO build work. Rootless podman cannot set up loop devices even with
> `--privileged` — the failure looks like `Loopdev setup failed`.

## Fields to double-check against your catalyst version

These are the spots where catalyst syntax has shifted across versions — verify
against the [upstream specs] before a production run:

- `snapshot_treeish` / `catalyst -s` form (named snapshot vs git treeish).
- The exact `livecd/cdtar` path under `/usr/share/catalyst/livecd/cdtar/`.
- That `getbinpkg` binhost URL is current and signatures verify (`make.conf`).
- That `catalyst-overrides/kmerge.sh` actually lands at
  `/usr/share/catalyst/targets/support/kmerge.sh` in the built image — this is
  catalyst's documented default `sharedir`, but if the `COPY` in `Containerfile`
  silently lands somewhere catalyst doesn't read from, kernel injection won't
  fire and the build.sh sanity check further up won't catch that (it can only
  verify oxys-build's tarballs exist, not that catalyst is reading our
  override). Confirm with `equery files dev-util/catalyst | grep kmerge.sh`
  inside the container if the ISO's kernel doesn't match what you expected.
- This whole kernel-injection mechanism is unverified against a real catalyst
  run in this repo's current state — it's built from reading catalyst's
  upstream source directly (kmerge.sh, functions.sh, stagebase.py,
  livecd_stage2.py), not from testing it end-to-end. Treat the first real
  build as the actual verification step.

[catalyst]: https://wiki.gentoo.org/wiki/Catalyst
[upstream specs]: https://github.com/gentoo/releng/tree/master/releases/specs/amd64
