# Oxys Install Pipeline

This documents the first bootable install path after disk provisioning. The
current implementation is intentionally conservative:

- disk provisioning is still a separate phase
- live-system copy is opt-in with `oxys install --copy-system`
- the bootloader is selectable (`bootloader = grub` or `systemd_boot`),
  independently of the init system; GRUB is the default
- ext4 is the first supported bootable layout
- ZFS can be copied/configured, but ZFS-root boot still depends on a known-good
  initramfs setup

## Phase 1: Provision Disk

The disk phase partitions, formats, and mounts the target at `/mnt/oxys`.

```sh
oxys install --device /dev/nvme0n1
```

For unattended test runs where the device is already verified:

```sh
oxys install --device /dev/nvme0n1 --confirm
```

Without `--copy-system`, the command stops after mounting the target root. This
is useful for inspecting the filesystem before copying the live system.

## Phase 2: Copy Live System

To copy the running Gentoo live system into the newly mounted target and install
systemd-boot:

```sh
oxys install --device /dev/nvme0n1 --copy-system
```

With `--confirm`, both the disk wipe and the copy-system confirmation are
skipped:

```sh
oxys install --device /dev/nvme0n1 --copy-system --confirm
```

The default source root is `/`. A different source root can be used for tests:

```sh
oxys install --device /dev/loop0 --copy-system --source-root /tmp/source-root
```

## What Gets Copied

The installer uses:

```sh
rsync -aHAXx --numeric-ids --info=progress2 / /mnt/oxys/
```

Runtime and live-ISO paths are excluded:

```text
/dev/*
/proc/*
/sys/*
/run/*
/tmp/*
/mnt/*
/media/*
/lost+found
/boot/efi/*
/var/tmp/*
/var/cache/binpkgs/*
/var/cache/distfiles/*
/root/.bash_history
/etc/machine-id
/etc/ssh/ssh_host_*
```

After the copy, the installer creates target runtime directories, resets
`/etc/machine-id`, writes `/etc/fstab`, copies the newest kernel/initramfs it
finds under target `/boot` into the ESP, installs the selected bootloader, and
writes its boot entry.

## Bootloader and Init System

`bootloader` and `init_system` are independent manifest choices — any
combination is valid (for example OpenRC with systemd-boot, or systemd with
GRUB). Both default when unset: `init_system = openrc`, `bootloader = grub`.

- `bootloader = systemd_boot`: runs `bootctl --esp-path <esp> install` and
  writes `loader/entries/oxys.conf`.
- `bootloader = grub`: runs
  `grub-install --target=x86_64-efi --efi-directory=<esp> --boot-directory=<target>/boot --removable`
  and hand-writes `<target>/boot/grub/grub.cfg`. The entry `search`es for the
  ESP by filesystem UUID and boots the same `/EFI/oxys/vmlinuz` copied by the
  shared boot-asset step, so both bootloaders share one kernel layout.

Service state (`services.enabled` / `services.disabled`) is applied offline for
the resolved init system:

- systemd: `systemctl --root <target> enable/disable <unit>` (unit names include
  the `.service` suffix).
- OpenRC: runlevel symlinks are managed directly under
  `<target>/etc/runlevels/default/` (bare service names, e.g. `NetworkManager`),
  exactly as `rc-update add/del <name> default` would — no chroot required.

The installer also performs a second, explicit `/boot` copy:

```sh
rsync -aHAX --numeric-ids --exclude=/efi/* /boot/ /mnt/oxys/boot/
```

This handles live systems where `/boot` is mounted separately from `/`.

## ext4 Boot Path

For an ext4 layout, `/etc/fstab` is generated from runtime `blkid` UUIDs:

```text
UUID=<root-uuid> / ext4 defaults,noatime 0 1
UUID=<home-uuid> /home ext4 defaults,noatime 0 2
UUID=<efi-uuid> /boot/efi vfat umask=0077 0 2
UUID=<swap-uuid> none swap sw 0 0
```

The systemd-boot entry is written to:

```text
/mnt/oxys/boot/efi/loader/entries/oxys.conf
```

The entry points at:

```text
/EFI/oxys/vmlinuz
/EFI/oxys/initramfs.img
```

The root option uses the root filesystem UUID:

```text
options root=UUID=<root-uuid> rw
```

Any values in `kernel.cmdline` are appended after `rw`.

The copy phase does not install packages or enable services. It assumes the live
system being copied is already a working Gentoo base for the intended init
system.

## ZFS Caveat

The copy phase can write a ZFS-shaped boot entry:

```text
options root=ZFS=rpool/ROOT rw
```

That is not the same as guaranteeing a bootable ZFS-root system. ZFS-root needs
an initramfs that includes the ZFS module and import/mount logic for the pool.
Until the initramfs generator is pinned down and tested on the live ISO, ext4 is
the only layout that should be treated as bootable.

## Manual Recovery

If the copy or bootloader phase fails after the disk is mounted:

```sh
umount -R /mnt/oxys
```

For ZFS:

```sh
zpool export rpool
```

If the ESP was partially configured, rerunning `oxys install --copy-system`
after reprovisioning is the cleanest path.
