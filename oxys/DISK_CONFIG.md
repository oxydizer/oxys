# Disk Configuration

Oxys disk provisioning is currently disk-only: it partitions the selected disk,
creates filesystems or a ZFS pool, mounts the target root at `/mnt/oxys`, then
stops. Package installation, initramfs generation, and bootloader setup are
separate follow-up work.

The real installer supports `zfs` and `ext4` layouts. `btrfs` and `luks_btrfs`
remain valid manifest layouts, but the real disk executor refuses them for now
instead of pretending to provision them.

Encryption is modeled but not provisioned yet. `Encryption::None` is the only
mode the real executor accepts today; `Password` and `Tpm` fail during planning
so a protected-looking config cannot accidentally install plaintext.

## ZFS Root

ZFS is the primary layout. It creates:

- EFI system partition mounted at `/boot/efi`
- optional swap partition when the resolved top-level swap policy uses disk
- one ZFS data partition
- one unencrypted ZFS pool, default `rpool`
- datasets derived from `disk.subvolumes`

The existing Btrfs-style subvolume names are mapped to valid ZFS dataset names:

- `@` -> `ROOT`
- `@home` -> `home`
- `@snapshots` -> `snapshots`
- `@log` -> `log`
- `@pkg` -> `pkg`

```rust
use oxys::prelude::*;

pub fn config() -> Oxys {
    Oxys {
        disk: Disk {
            device: "/dev/nvme0n1".into(),
            layout: DiskLayout::Zfs,
            zfs: ZfsOptions {
                pool: "rpool".into(),
                ashift: 12,
                compression: "lz4".into(),
            },
            ..Disk::default()
        },
        ..Oxys::default()
    }
}
```

Custom ZFS datasets use the same `Subvolume` shape as Btrfs:

```rust
Disk {
    device: "/dev/nvme0n1".into(),
    layout: DiskLayout::Zfs,
    subvolumes: vec![
        Subvolume {
            name: "@".into(),
            mount: "/".into(),
        },
        Subvolume {
            name: "@home".into(),
            mount: "/home".into(),
        },
        Subvolume {
            name: "@varlog".into(),
            mount: "/var/log".into(),
        },
    ],
    ..Disk::default()
}
```

## ext4 Root and Home

The ext4 layout creates:

- EFI system partition mounted at `/boot/efi`
- optional swap partition when the resolved top-level swap policy uses disk
- ext4 root partition
- ext4 `/home` partition using the remaining disk space by default

```rust
use oxys::prelude::*;

pub fn config() -> Oxys {
    Oxys {
        disk: Disk {
            device: "/dev/sda".into(),
            layout: DiskLayout::Ext4,
            ext4: Ext4Options {
                separate_home: true,
                root_size: 80 * GB,
            },
            ..Disk::default()
        },
        ..Oxys::default()
    }
}
```

To put all ext4 space in `/`, disable the separate home partition:

```rust
Disk {
    device: "/dev/sda".into(),
    layout: DiskLayout::Ext4,
    ext4: Ext4Options {
        separate_home: false,
        root_size: 100 * GB,
    },
    ..Disk::default()
}
```

## EFI and Swap

EFI and swap settings are shared by ZFS and ext4.

```rust
Disk {
    device: "/dev/nvme0n1".into(),
    layout: DiskLayout::Zfs,
    partitions: DiskPartitions {
        efi: EfiPartition {
            size: 1024 * MB,
            mount: "/boot/efi".into(),
        },
        swap: SwapConfig::Partition { size: 16 * GB }, // legacy compatibility
    },
    ..Disk::default()
}
```

New configs declare swap at the top level. To avoid any swap setup:

```rust
Swap {
    strategy: SwapStrategy::Disabled,
    swappiness: 180,
}
```

## Encryption Plan

The manifest shape is already available:

```rust
Disk {
    device: "/dev/nvme0n1".into(),
    layout: DiskLayout::Ext4,
    encryption: Encryption::Password,
    ..Disk::default()
}
```

Password encryption is **not provisioned yet** (`plan_disk` rejects non-`None`).
The full design lives in
[`docs/INSTALLER_DUALBOOT_ENCRYPTION_PLAN.md`](../docs/INSTALLER_DUALBOOT_ENCRYPTION_PLAN.md)
§6, refined against matrixOS’s working LUKS path. Summary for implementers:

1. **Validate first** — `cryptsetup` present, non-empty key source (password file or
   session passphrase). Never a baked default passphrase in config.
2. Create/select the root **partition**, then:
   - `cryptsetup luksFormat --type luks2 <partition>` (key via `--key-file` or stdin)
   - `cryptsetup open --allow-discards … <partition> oxys-root`
   - settle udev; require `/dev/mapper/oxys-root`
3. Keep two device identities (matrixOS pattern):
   - **backing partition** → LUKS UUID → `rd.luks.uuid=` and crypttab
   - **mapper** → ext4 UUID → `root=UUID=` and fstab
4. `mkfs.ext4` + mount the **mapper**, never the raw LUKS node as `/`.
5. Write `/etc/crypttab` (`oxys-root UUID=<luks-uuid> none luks,discard`) **and**
   kernel cmdline; generate installed initramfs with dracut `--add crypt`.
6. On failure: unmount target, `cryptsetup close` tracked mappers.
7. When encryption is on: zram-only swap for v1; ext4 only; single root.

TPM unlock can build on the same LUKS layout later with
`systemd-cryptenroll --tpm2-device=auto`.

## Running the Installer

Build or generate `manifest.toml` first, then run:

```sh
oxys install
```

The command prints the exact destructive disk plan and asks you to type the
target device name before it runs. To override the manifest device:

```sh
oxys install --device /dev/nvme0n1
```

For unattended live-ISO runs, `--confirm` skips the interactive prompt:

```sh
oxys install --device /dev/nvme0n1 --confirm
```

Use `--confirm` only when the device has already been verified. The installer
refuses to continue if the selected disk is already mounted or appears to be in
use.
