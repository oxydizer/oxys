# OxysOS Agent Notes

These instructions apply to the whole repository. Preserve unrelated user
changes; ISO, installer, and kernel work is frequently developed together in a
dirty worktree.

## Authoritative paths

- `oxys-installer/configs/*.fe2o3` are the canonical installer profiles.
  Keep their copies in `oxys-iso/overlay/root/configs/` byte-for-byte in sync.
- `oxys-iso/portage_confdir/` controls packages built into the live root.
  Installer profile package flags also matter because they control later target
  updates. Desktop-critical USE flags generally belong in both places.
- `oxys-build/podman/kernel/base.config` is the kernel fragment. Catalyst does
  not build a new kernel: `oxys-iso` injects artifacts from
  `oxys-build/output/<arch>/`.
- `oxys-iso/fsscript/fsscript.sh` runs inside the live root near the end of the
  catalyst build. Put assertions here when an ISO must never ship without a
  required runtime file.

## Kernel and hostname branding

- The intended `uname -a` prefix is `Linux OxysOS <version>-oxys`.
- `CONFIG_LOCALVERSION="-oxys"` and `CONFIG_LOCALVERSION_AUTO=n` provide the
  local suffix. `oxys-build-packages.sh` must also clear gentoo-sources'
  top-level `EXTRAVERSION`; otherwise the release still contains `gentoo`.
- Never choose kernel artifacts by filename order. Multiple artifacts can have
  the same build ID. `oxys-iso/scripts/resolve-kernel-build.sh` selects the
  newest metadata set by `created_utc`, verifies the kernel/ZFS release pair,
  requires `-oxys`, and rejects a release containing `gentoo`.
- Check the selected release before an ISO build:

  ```sh
  OXYS_ARCH=v3 ./oxys-iso/scripts/resolve-kernel-build.sh
  ```

- The source atom and internal build ID may still say `gentoo` because the
  patched source package is gentoo-sources. Those provenance strings must not
  leak into `kernel_release` or `uname`.
- OpenRC reads `/etc/conf.d/hostname`; `/etc/hostname` alone does not replace
  the live medium's `livecd` nodename. Keep both set to `OxysOS`, ensure the
  `hostname` service is in the boot runlevel, and make the installer write both
  files to the target.

## QEMU and Niri

- Use the project runner for graphical testing:

  ```sh
  GL=1 ./oxys-iso/scripts/run-qemu.sh
  GL=1 DRY_RUN=1 ./oxys-iso/scripts/run-qemu.sh
  ```

- `virtio-vga-gl` is a QEMU device model passed with `-device`; it is not a
  command that can be executed in the host shell. The dry run must show both
  `-device virtio-vga-gl,...` and `-display sdl,gl=on`.
- A working host QEMU command does not prove the guest userspace can render.
  `/dev/dri/card0`, `/dev/dri/renderD128`, and a Virtio GPU in `lspci` prove
  only that the kernel driver is present. The ISO must also contain Mesa's
  `virtio_gpu_dri.so`, supplied by `media-libs/mesa[video_cards_virgl]`.
- Keep `video_cards_virgl` package-local in
  `oxys-iso/portage_confdir/package.use/desktop` as well as in `VIDEO_CARDS`.
  Portage may otherwise accept an official Mesa binpkg without virgl. Keep
  `--binpkg-respect-use=y` enabled so an incompatible binpkg is rebuilt instead
  of silently weakening the image.
- Gentoo's `sys-auth/seatd` installs libseat without necessarily installing the
  daemon. `USE=server` is required for `/etc/init.d/seatd` and the seat service.
  Desktop users need the `seat` group, and the service must be enabled. Confirm
  the package created the group with `getent group seat` before using `usermod`.
- Starting elogind does not compensate for a missing virgl renderer. Treat seat
  acquisition and EGL/GPU rendering as separate checks.
- A valid `niri validate` result only rules out KDL syntax errors. If Niri takes
  tty1, VT switching stops working, and the log remains empty, check seatd and
  `virtio_gpu_dri.so` before changing the Niri config.
- The fsscript deliberately fails the ISO build when the seatd init script or
  Mesa virgl driver is absent. Do not remove those assertions to get a build
  through; fix the package USE flags or cache instead.
- Niri/libinput requires `/dev/input/event*`. A kernel can accept keyboard
  input on a tty through AT keyboard support while still having
  `CONFIG_INPUT_EVDEV` disabled, leaving the graphical session with neither
  keyboard nor pointer input. Keep `CONFIG_INPUT_EVDEV=y` and
  `CONFIG_VIRTIO_INPUT=y`; the QEMU runner supplies explicit virtio keyboard
  and tablet devices.

## QEMU networking and recovery

- The runner uses QEMU user-mode NAT and forwards host port 2222 to guest port
  22 by default. `ss -ltn` showing host port 2222 only proves QEMU created the
  forward; it does not prove that the guest has DHCP, that sshd is listening,
  or that the guest user can authenticate.
- For SSH recovery, verify inside the guest that NetworkManager and sshd are
  running, the interface has an address, and the chosen account has a usable
  password or key. Connect with:

  ```sh
  ssh -p 2222 USER@localhost
  ```

- The QEMU runner multiplexes a serial device and the QEMU monitor onto the host
  terminal. The monitor remains useful, but guest shell access over serial also
  requires a `console=ttyS0` kernel argument/getty, which is not currently
  configured. Prefer SSH for guest diagnosis when a compositor owns tty1.
- The default install disk is 24 GiB. An 8 GiB target can fill during a desktop
  install and make logs silently fail with `ENOSPC`, which can look like a Niri
  hang.

## Build and verification

- A corrected cached kernel artifact means only the ISO needs rebuilding; do
  not rebuild the kernel unless the resolver reports that no valid branded pair
  exists.
- Normal cached ISO build:

  ```sh
  OXYS_ARCH=v3 ./oxys-iso/scripts/enter-container.sh build
  ```

- Before handing off relevant changes, run:

  ```sh
  bash -n oxys-iso/fsscript/fsscript.sh \
    oxys-iso/scripts/resolve-kernel-build.sh \
    oxys-iso/scripts/run-qemu.sh
  CARGO_NET_OFFLINE=true cargo test --manifest-path oxys/Cargo.toml
  git diff --check
  ```

- Compile every changed `.fe2o3` profile with the local `oxys compile` command.
  Use `CARGO_NET_OFFLINE=true` when dependencies are already cached; sandboxed
  DNS failures are not compiler failures.
- If Portage reports a missing `/var/db/repos/gentoo`, repair/sync the repository
  before diagnosing individual ebuild failures. `scdoc` or `re2c` failures are
  build-dependency failures; inspect their build logs rather than attributing
  them to Niri.
