use std::{
    fmt,
    path::{Path, PathBuf},
};

use thiserror::Error;

use crate::exec::{CommandStep, ExecError, StepEvent, StepStream};
use crate::manifest::{
    Disk, DiskLayout, Encryption, MB, ResolvedSwap, SwapResolveError, SystemManifest,
};

mod apply;
mod ext4;
mod preflight;
mod zfs;

pub use apply::{apply_disk_plan, release_target_mounts};
pub use preflight::{MIN_INSTALL_BYTES, preflight, preflight_with_swap};

pub type DiskStep = CommandStep;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiskPlan {
    pub device: String,
    pub target_mount: PathBuf,
    pub steps: Vec<DiskStep>,
}

impl DiskPlan {
    pub fn render(&self) -> String {
        self.steps
            .iter()
            .enumerate()
            .map(|(idx, step)| {
                format!(
                    "{:>2}. {}\n    {}",
                    idx + 1,
                    step.description,
                    step.command_line()
                )
            })
            .collect::<Vec<_>>()
            .join("\n")
    }
}

impl fmt::Display for DiskPlan {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.render())
    }
}

pub type ProvisionEvent = StepEvent;
pub type ProvisionStream = StepStream<DiskError>;

#[derive(Debug, Error)]
pub enum DiskError {
    #[error("disk device is not set")]
    MissingDevice,
    #[error("disk device does not exist: {0}")]
    DeviceMissing(String),
    #[error("disk device is not a block device: {0}")]
    NotBlockDevice(String),
    /// Disk is mounted, swapped, held by LVM/RAID/dm, read-only, etc.
    #[error("refusing to provision {device}: {reason}")]
    DeviceBusy { device: String, reason: String },
    #[error("disk too small: {device} is {have}, need at least {need}")]
    DeviceTooSmall {
        device: String,
        have: String,
        need: String,
    },
    #[error("unsupported disk layout for real provisioning: {0:?}")]
    UnsupportedLayout(DiskLayout),
    #[error("unsupported disk encryption mode for real provisioning: {0:?}")]
    UnsupportedEncryption(Encryption),
    #[error("invalid disk layout: {0}")]
    InvalidLayout(String),
    #[error(transparent)]
    InvalidSwap(#[from] SwapResolveError),
    #[error(transparent)]
    Exec(#[from] ExecError),
}

pub fn plan_disk(disk: &Disk, target_mount: &Path) -> Result<DiskPlan, DiskError> {
    let manifest = SystemManifest {
        disk: disk.clone(),
        ..SystemManifest::default()
    };
    let resolved_swap = manifest.resolved_swap()?;
    plan_disk_with_swap(disk, &resolved_swap, target_mount)
}

pub fn plan_disk_with_swap(
    disk: &Disk,
    resolved_swap: &ResolvedSwap,
    target_mount: &Path,
) -> Result<DiskPlan, DiskError> {
    if disk.device.trim().is_empty() {
        return Err(DiskError::MissingDevice);
    }
    if disk.encryption != Encryption::None {
        return Err(DiskError::UnsupportedEncryption(disk.encryption));
    }

    let device = disk.device.clone();
    let efi_part = apply::partition_path(&device, 1);
    let mut next_part = 2;
    let mut steps = vec![
        DiskStep::new("Wipe filesystem signatures", "wipefs", ["-a", &device]),
        DiskStep::new(
            "Zap existing GPT/MBR data",
            "sgdisk",
            ["--zap-all", &device],
        ),
        DiskStep::new(
            "Create EFI system partition",
            "sgdisk",
            [
                "-n",
                &format!("1:1M:+{}M", mib(disk.partitions.efi.size).max(1)),
                "-t",
                "1:ef00",
                &device,
            ],
        ),
    ];

    let swap_part = match disk.layout {
        DiskLayout::Zfs => {
            let zfs_swap = zfs::zfs_swap_partition(resolved_swap, next_part + 1);
            let rpool_part_number = if zfs_swap.is_some() {
                next_part + 2
            } else {
                next_part + 1
            };
            let swap_part = zfs_swap.map(|(number, _)| apply::partition_path(&device, number));
            zfs::plan_zfs(
                disk,
                target_mount,
                &device,
                &mut steps,
                next_part,
                zfs_swap,
                rpool_part_number,
            )?;
            swap_part
        }
        DiskLayout::Ext4 => {
            let swap_part =
                ext4::plan_swap_partition(resolved_swap, &device, &mut steps, &mut next_part);
            ext4::plan_ext4(disk, target_mount, &device, &mut steps, next_part)?;
            swap_part
        }
        DiskLayout::Btrfs | DiskLayout::LuksBtrfs => {
            return Err(DiskError::UnsupportedLayout(disk.layout));
        }
    };

    steps.push(wipe_signatures_step(&efi_part));
    steps.push(DiskStep::new(
        "Create EFI FAT32 filesystem",
        "mkfs.vfat",
        ["-F32", &efi_part],
    ));
    steps.push(DiskStep::new(
        "Create EFI mountpoint",
        "mkdir",
        [
            "-p",
            &target_mount
                .join(disk.partitions.efi.mount.trim_start_matches('/'))
                .display()
                .to_string(),
        ],
    ));
    steps.push(DiskStep::new(
        "Mount EFI system partition",
        "mount",
        [
            &efi_part,
            &target_mount
                .join(disk.partitions.efi.mount.trim_start_matches('/'))
                .display()
                .to_string(),
        ],
    ));

    if let Some(swap_part) = swap_part {
        steps.push(wipe_signatures_step(&swap_part));
        steps.push(DiskStep::new(
            "Create swap signature",
            "mkswap",
            [&swap_part],
        ));
        steps.push(DiskStep::new(
            "Enable swap partition",
            "swapon",
            [&swap_part],
        ));
    }

    Ok(DiskPlan {
        device,
        target_mount: target_mount.to_path_buf(),
        steps,
    })
}

/// Erase any residual filesystem/pool signatures inside a freshly created
/// partition before formatting it.
///
/// The whole-device `wipefs -a` at the start of provisioning only reaches
/// signatures at the start and end of the *disk*; it cannot touch the interior
/// region a partition will later occupy. On a reused disk that region may still
/// hold old signatures -- ZFS is the worst offender because it writes labels at
/// both ends of its vdev, so a former zpool leaves `zfs_member` labels sitting
/// exactly where the new partition now lives. `mkfs` overwrites the front of
/// the partition but not those stray trailing labels, leaving two conflicting
/// signatures. libblkid's safe-probe then treats the partition as ambiguous and
/// reports no UUID at all, which later fails the fstab/bootloader UUID lookups
/// even though the filesystem itself is perfectly healthy.
///
/// `wipefs -a` on the partition clears every signature it finds (front and
/// back), and is a harmless no-op on an already-clean partition.
pub(super) fn wipe_signatures_step(part: &str) -> DiskStep {
    DiskStep::new("Wipe stale signatures on partition", "wipefs", ["-a", part])
}

pub(super) fn swap_partition_step(device: &str, number: usize, size: u64) -> DiskStep {
    DiskStep::new(
        "Create swap partition",
        "sgdisk",
        [
            "-n",
            &format!("{number}:0:+{}M", mib(size).max(1)),
            "-t",
            &format!("{number}:8200"),
            device,
        ],
    )
}

pub(super) fn mib(bytes: u64) -> u64 {
    bytes.div_ceil(MB)
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;
    use crate::manifest::{
        Compression, Disk, DiskLayout, DiskPartitions, Encryption, GB, ResolvedDiskSwap,
        ResolvedSwap, ResolvedZram, SwapConfig,
    };

    #[test]
    fn plan_disk_refuses_encryption_until_luks_is_wired() {
        let disk = Disk {
            device: "/dev/testdisk".to_owned(),
            layout: DiskLayout::Ext4,
            encryption: Encryption::Password,
            ..Disk::default()
        };

        let error = plan_disk(&disk, Path::new("/mnt/oxys")).expect_err("plan should fail");

        assert!(matches!(
            error,
            DiskError::UnsupportedEncryption(Encryption::Password)
        ));
    }

    #[test]
    fn ext4_default_plan_is_whole_disk_single_root() {
        let disk = Disk {
            device: "/dev/nvme0n1".to_owned(),
            ..Disk::default()
        };
        assert_eq!(disk.layout, DiskLayout::Ext4);

        let plan = plan_disk(&disk, Path::new("/mnt/oxys")).unwrap();
        let rendered = plan.render();

        // EFI ESP as p1, ext4 root filling the rest of the disk as p2.
        assert!(rendered.contains("sgdisk -n 1:1M:+512M -t 1:ef00 /dev/nvme0n1"));
        assert!(rendered.contains("sgdisk -n 2:0:0 -t 2:8300 /dev/nvme0n1"));
        assert!(rendered.contains("mkfs.ext4 -F /dev/nvme0n1p2"));
        // Whole-disk default: no separate /home partition or filesystem.
        assert!(!rendered.contains("/dev/nvme0n1p3"));
        assert!(!rendered.contains("home"));
    }

    #[test]
    fn zfs_plan_uses_boot_pool_root_pool_and_nested_datasets() {
        let disk = Disk {
            device: "/dev/nvme0n1".to_owned(),
            layout: DiskLayout::Zfs,
            partitions: DiskPartitions {
                swap: SwapConfig::Partition { size: 8 * GB },
                ..DiskPartitions::default()
            },
            ..Disk::default()
        };

        let plan = plan_disk(&disk, Path::new("/mnt/oxys")).unwrap();
        let rendered = plan.render();

        assert!(rendered.contains("zgenhostid"));
        assert!(rendered.contains("sgdisk -n 2:0:+2048M -t 2:bf00 /dev/nvme0n1"));
        assert!(rendered.contains("sgdisk -n 3:0:+8192M -t 3:8200 /dev/nvme0n1"));
        assert!(rendered.contains("sgdisk -n 4:0:0 -t 4:bf00 /dev/nvme0n1"));
        assert!(rendered.contains("zpool create -f -o ashift=12 -o autotrim=on -O compression=zstd -O acltype=posixacl -O xattr=sa -O atime=off -O normalization=formD -O dnodesize=auto -O canmount=off -O mountpoint=none -R /mnt/oxys rpool /dev/nvme0n1p4"));
        assert!(rendered.contains("zpool create -f -o compatibility=grub2 -o ashift=12 -o autotrim=on -O compression=lz4 -O acltype=posixacl -O xattr=sa -O devices=off -O atime=off -O canmount=off -O mountpoint=/boot -R /mnt/oxys bpool /dev/nvme0n1p2"));
        assert!(
            rendered.contains("zfs create -p -o mountpoint=/ -o canmount=noauto rpool/ROOT/os")
        );
        assert!(rendered.contains("zfs mount rpool/ROOT/os"));
        assert!(
            rendered.contains("zfs create -p -o mountpoint=/boot -o canmount=on bpool/BOOT/os")
        );
        assert!(rendered.contains(
            "zfs create -p -o mountpoint=/var/cache/distfiles -o canmount=on rpool/gentoo/distfiles"
        ));
        assert!(rendered.contains("zpool set bootfs=rpool/ROOT/os rpool"));
        assert!(rendered.contains("zpool set cachefile=/mnt/oxys/etc/zfs/zpool.cache rpool"));
        assert!(rendered.contains("zpool set cachefile=/mnt/oxys/etc/zfs/zpool.cache bpool"));
    }

    #[test]
    fn hybrid_swap_adds_one_disk_partition() {
        let disk = Disk {
            device: "/dev/nvme0n1".to_owned(),
            ..Disk::default()
        };
        let swap = ResolvedSwap {
            zram: Some(ResolvedZram {
                size: 8 * GB,
                algorithm: Compression::Zstd,
                priority: 100,
            }),
            disk: Some(ResolvedDiskSwap {
                size: 4 * GB,
                priority: 10,
            }),
            swappiness: 180,
        };

        let rendered = plan_disk_with_swap(&disk, &swap, Path::new("/mnt/oxys"))
            .unwrap()
            .render();
        assert!(rendered.contains("sgdisk -n 2:0:+4096M -t 2:8200 /dev/nvme0n1"));
        assert!(rendered.contains("sgdisk -n 3:0:0 -t 3:8300 /dev/nvme0n1"));
    }

    #[test]
    fn mounted_device_check_only_matches_numeric_partition_suffixes() {
        assert!(apply::mount_source_matches_device(
            "/dev/sda1",
            "/dev/sda",
            Path::new("/dev/sda")
        ));
        assert!(!apply::mount_source_matches_device(
            "/dev/sdab",
            "/dev/sda",
            Path::new("/dev/sda")
        ));
        assert!(apply::mount_source_matches_device(
            "/dev/nvme0n1p1",
            "/dev/nvme0n1",
            Path::new("/dev/nvme0n1")
        ));
    }
}
