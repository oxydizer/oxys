use std::{
    fmt, fs,
    path::{Path, PathBuf},
    sync::mpsc::Sender,
};

#[cfg(unix)]
use std::os::unix::fs::FileTypeExt;

use thiserror::Error;

use crate::exec::{self, CommandStep, ExecError, StepEvent, StepStream};
use crate::manifest::{Disk, DiskLayout, Encryption, SwapConfig, ZfsDataset, MB};

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
    #[error("refusing to provision mounted/live disk: {0}")]
    DeviceMounted(String),
    #[error("unsupported disk layout for real provisioning: {0:?}")]
    UnsupportedLayout(DiskLayout),
    #[error("unsupported disk encryption mode for real provisioning: {0:?}")]
    UnsupportedEncryption(Encryption),
    #[error("invalid disk layout: {0}")]
    InvalidLayout(String),
    #[error(transparent)]
    Exec(#[from] ExecError),
}

pub fn plan_disk(disk: &Disk, target_mount: &Path) -> Result<DiskPlan, DiskError> {
    if disk.device.trim().is_empty() {
        return Err(DiskError::MissingDevice);
    }
    if disk.encryption != Encryption::None {
        return Err(DiskError::UnsupportedEncryption(disk.encryption));
    }

    let device = disk.device.clone();
    let efi_part = partition_path(&device, 1);
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
            let zfs_swap = zfs_swap_partition(disk, next_part + 1);
            let rpool_part_number = if zfs_swap.is_some() {
                next_part + 2
            } else {
                next_part + 1
            };
            let swap_part = zfs_swap.map(|(number, _)| partition_path(&device, number));
            plan_zfs(
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
            let swap_part = plan_swap_partition(disk, &device, &mut steps, &mut next_part);
            plan_ext4(disk, target_mount, &device, &mut steps, next_part)?;
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

fn plan_zfs(
    disk: &Disk,
    target_mount: &Path,
    device: &str,
    steps: &mut Vec<DiskStep>,
    bpool_part_number: usize,
    swap_part: Option<(usize, u64)>,
    rpool_part_number: usize,
) -> Result<(), DiskError> {
    let pool = disk.zfs.pool.trim();
    let boot_pool = disk.zfs.boot_pool.trim();
    if pool.is_empty() {
        return Err(DiskError::InvalidLayout(
            "ZFS pool name is empty".to_owned(),
        ));
    }
    if boot_pool.is_empty() {
        return Err(DiskError::InvalidLayout(
            "ZFS boot pool name is empty".to_owned(),
        ));
    }
    if pool == boot_pool {
        return Err(DiskError::InvalidLayout(
            "ZFS root pool and boot pool must be different".to_owned(),
        ));
    }

    let bpool_part = partition_path(device, bpool_part_number);
    let rpool_part = partition_path(device, rpool_part_number);
    steps.push(DiskStep::new(
        "Ensure hostid exists on the live system",
        "zgenhostid",
        ["-f"],
    ));
    steps.push(DiskStep::new(
        "Create ZFS boot pool partition",
        "sgdisk",
        [
            "-n",
            &format!(
                "{bpool_part_number}:0:+{}M",
                mib(disk.zfs.boot_pool_size).max(1)
            ),
            "-t",
            &format!("{bpool_part_number}:bf00"),
            device,
        ],
    ));
    if let Some((number, size)) = swap_part {
        steps.push(swap_partition_step(device, number, size));
    }
    steps.push(DiskStep::new(
        "Create ZFS root pool partition",
        "sgdisk",
        [
            "-n",
            &format!("{rpool_part_number}:0:0"),
            "-t",
            &format!("{rpool_part_number}:bf00"),
            device,
        ],
    ));
    steps.push(DiskStep::new(
        "Ask kernel to reread partition table",
        "partprobe",
        [device],
    ));
    steps.push(DiskStep::new(
        "Wait for partition device nodes to settle",
        "udevadm",
        ["settle", "--timeout=30"],
    ));
    steps.push(DiskStep::new(
        "Create ZFS root pool",
        "zpool",
        [
            "create",
            "-f",
            "-o",
            &format!("ashift={}", disk.zfs.ashift),
            "-o",
            "autotrim=on",
            "-O",
            &format!("compression={}", disk.zfs.compression),
            "-O",
            "acltype=posixacl",
            "-O",
            "xattr=sa",
            "-O",
            "atime=off",
            "-O",
            "normalization=formD",
            "-O",
            "dnodesize=auto",
            "-O",
            "canmount=off",
            "-O",
            "mountpoint=none",
            "-R",
            &target_mount.display().to_string(),
            pool,
            &rpool_part,
        ],
    ));
    steps.push(DiskStep::new(
        "Create ZFS boot pool",
        "zpool",
        [
            "create",
            "-f",
            "-o",
            "compatibility=grub2",
            "-o",
            &format!("ashift={}", disk.zfs.ashift),
            "-o",
            "autotrim=on",
            "-O",
            &format!("compression={}", disk.zfs.boot_compression),
            "-O",
            "acltype=posixacl",
            "-O",
            "xattr=sa",
            "-O",
            "devices=off",
            "-O",
            "atime=off",
            "-O",
            "canmount=off",
            "-O",
            "mountpoint=/boot",
            "-R",
            &target_mount.display().to_string(),
            boot_pool,
            &bpool_part,
        ],
    ));

    let root_dataset = disk
        .zfs
        .datasets
        .iter()
        .find(|dataset| dataset.pool.trim() == pool && dataset.mount.trim() == "/")
        .ok_or_else(|| {
            DiskError::InvalidLayout("ZFS layout needs a root dataset mounted at /".to_owned())
        })?;

    for dataset in ordered_zfs_datasets(disk, pool, boot_pool)? {
        let pool_name = dataset.pool.trim();
        let dataset_name = zfs_dataset_name(&dataset.name)?;
        let full_dataset = format!("{pool_name}/{dataset_name}");
        let mount = normalize_zfs_mountpoint(&dataset.mount)?;
        if mount != "none" && mount != "/" {
            steps.push(DiskStep::new(
                format!("Create ZFS mountpoint {mount}"),
                "mkdir",
                [
                    "-p",
                    &target_mount
                        .join(mount.trim_start_matches('/'))
                        .display()
                        .to_string(),
                ],
            ));
        }
        steps.push(DiskStep::new(
            format!("Create ZFS dataset {full_dataset}"),
            "zfs",
            [
                "create",
                "-p",
                "-o",
                &format!("mountpoint={mount}"),
                "-o",
                &format!("canmount={}", dataset.canmount.as_zfs_value()),
                &full_dataset,
            ],
        ));
        if mount == "/" {
            steps.push(DiskStep::new(
                format!("Mount ZFS root dataset {full_dataset}"),
                "zfs",
                ["mount", &full_dataset],
            ));
        }
    }

    let bootfs = format!("{pool}/{}", zfs_dataset_name(&root_dataset.name)?);
    steps.push(DiskStep::new(
        "Set ZFS boot filesystem",
        "zpool",
        ["set", &format!("bootfs={bootfs}"), pool],
    ));
    steps.push(DiskStep::new(
        "Create target ZFS cache directory",
        "mkdir",
        ["-p", &target_mount.join("etc/zfs").display().to_string()],
    ));
    steps.push(DiskStep::new(
        "Write root pool import cache",
        "zpool",
        [
            "set",
            &format!(
                "cachefile={}",
                target_mount.join("etc/zfs/zpool.cache").display()
            ),
            pool,
        ],
    ));
    steps.push(DiskStep::new(
        "Write boot pool import cache",
        "zpool",
        [
            "set",
            &format!(
                "cachefile={}",
                target_mount.join("etc/zfs/zpool.cache").display()
            ),
            boot_pool,
        ],
    ));

    Ok(())
}

fn plan_swap_partition(
    disk: &Disk,
    device: &str,
    steps: &mut Vec<DiskStep>,
    next_part: &mut usize,
) -> Option<String> {
    match disk.partitions.swap {
        SwapConfig::Partition { size } => {
            let number = *next_part;
            *next_part += 1;
            steps.push(swap_partition_step(device, number, size));
            Some(partition_path(device, number))
        }
        SwapConfig::Zram { .. } | SwapConfig::None | SwapConfig::File { .. } => None,
    }
}

fn zfs_swap_partition(disk: &Disk, number: usize) -> Option<(usize, u64)> {
    match disk.partitions.swap {
        SwapConfig::Partition { size } => Some((number, size)),
        SwapConfig::Zram { .. } | SwapConfig::None | SwapConfig::File { .. } => None,
    }
}

fn swap_partition_step(device: &str, number: usize, size: u64) -> DiskStep {
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
fn wipe_signatures_step(part: &str) -> DiskStep {
    DiskStep::new("Wipe stale signatures on partition", "wipefs", ["-a", part])
}

fn plan_ext4(
    disk: &Disk,
    target_mount: &Path,
    device: &str,
    steps: &mut Vec<DiskStep>,
    part_number: usize,
) -> Result<(), DiskError> {
    // With a separate /home the root partition is sized to `root_size` and home
    // takes the remainder. As a whole-disk single-partition layout (the default)
    // the root fills the rest of the disk after the EFI/swap partitions.
    if disk.ext4.separate_home && disk.ext4.root_size == 0 {
        return Err(DiskError::InvalidLayout(
            "ext4 root_size must be greater than zero when a separate /home is used".to_owned(),
        ));
    }

    let root_part = partition_path(device, part_number);
    let home_part = partition_path(device, part_number + 1);
    let root_span = if disk.ext4.separate_home {
        format!("{part_number}:0:+{}M", mib(disk.ext4.root_size).max(1))
    } else {
        format!("{part_number}:0:0")
    };
    steps.push(DiskStep::new(
        "Create ext4 root partition",
        "sgdisk",
        [
            "-n",
            &root_span,
            "-t",
            &format!("{part_number}:8300"),
            device,
        ],
    ));

    if disk.ext4.separate_home {
        steps.push(DiskStep::new(
            "Create ext4 home partition",
            "sgdisk",
            [
                "-n",
                &format!("{}:0:0", part_number + 1),
                "-t",
                &format!("{}:8300", part_number + 1),
                device,
            ],
        ));
    }

    steps.push(DiskStep::new(
        "Ask kernel to reread partition table",
        "partprobe",
        [device],
    ));
    steps.push(DiskStep::new(
        "Wait for partition device nodes to settle",
        "udevadm",
        ["settle", "--timeout=30"],
    ));
    steps.push(wipe_signatures_step(&root_part));
    steps.push(DiskStep::new(
        "Create ext4 root filesystem",
        "mkfs.ext4",
        ["-F", &root_part],
    ));
    steps.push(DiskStep::new(
        "Create target root mountpoint",
        "mkdir",
        ["-p", &target_mount.display().to_string()],
    ));
    steps.push(DiskStep::new(
        "Mount ext4 root filesystem",
        "mount",
        [&root_part, &target_mount.display().to_string()],
    ));

    if disk.ext4.separate_home {
        steps.push(wipe_signatures_step(&home_part));
        steps.push(DiskStep::new(
            "Create ext4 home filesystem",
            "mkfs.ext4",
            ["-F", &home_part],
        ));
        steps.push(DiskStep::new(
            "Create target home mountpoint",
            "mkdir",
            ["-p", &target_mount.join("home").display().to_string()],
        ));
        steps.push(DiskStep::new(
            "Mount ext4 home filesystem",
            "mount",
            [&home_part, &target_mount.join("home").display().to_string()],
        ));
    }

    Ok(())
}

pub fn preflight(disk: &Disk) -> Result<(), DiskError> {
    let device = disk.device.trim();
    if device.is_empty() {
        return Err(DiskError::MissingDevice);
    }

    let metadata = fs::metadata(device).map_err(|_| DiskError::DeviceMissing(device.to_owned()))?;

    #[cfg(unix)]
    if !metadata.file_type().is_block_device() {
        return Err(DiskError::NotBlockDevice(device.to_owned()));
    }

    let device_path = Path::new(device);
    let canonical_device = fs::canonicalize(device_path).unwrap_or_else(|_| device_path.into());
    let mounts = fs::read_to_string("/proc/mounts").unwrap_or_default();
    for line in mounts.lines() {
        let mut fields = line.split_whitespace();
        let Some(source) = fields.next() else {
            continue;
        };
        if fields.next().is_none() {
            continue;
        }
        if mount_source_matches_device(source, device, &canonical_device) {
            return Err(DiskError::DeviceMounted(device.to_owned()));
        }
    }

    Ok(())
}

/// Best-effort release of any leftover mounts under the installer's own target
/// mount point.
///
/// An install that aborts before the final unmount (`Finalize`) leaves the new
/// root -- and the ESP mounted beneath it -- mounted at `target_mount`. On the
/// next attempt `preflight` sees those and refuses with "refusing to provision
/// mounted/live disk", forcing a manual `umount`. Clearing them here makes a
/// re-run just work. This only ever touches the installer's own mount point
/// (`/mnt/oxys`), never arbitrary user mounts, so a partition mounted somewhere
/// else still (correctly) trips the preflight guard.
pub fn release_target_mounts(target_mount: &Path) {
    let path = target_mount.display().to_string();
    let cleared = exec::capture_command("umount", ["-R", &path])
        .map(|output| output.status.success())
        .unwrap_or(false);
    if !cleared {
        // A busy mount (e.g. a lingering process) needs a lazy detach.
        let _ = exec::capture_command("umount", ["-Rl", &path]);
    }
}

pub fn apply_disk_plan(plan: &DiskPlan) -> ProvisionStream {
    let plan = plan.clone();
    StepStream::spawn(move |sender| run_plan(plan, sender))
}

fn run_plan(plan: DiskPlan, sender: Sender<ProvisionEvent>) -> Result<(), DiskError> {
    for step in &plan.steps {
        if let Err(error) = exec::run_command_step(step, &sender).map_err(DiskError::from) {
            let _ = sender.send(StepEvent::Error {
                step: step.description.clone(),
                message: error.to_string(),
            });
            // A step may have already mounted the target; release it so the next
            // attempt isn't blocked by preflight's mounted-disk guard.
            release_target_mounts(&plan.target_mount);
            return Err(error);
        }
    }

    Ok(())
}

fn partition_path(device: &str, number: usize) -> String {
    crate::util::partition_path(device, number)
}

fn partition_prefix(device: &str) -> String {
    if device
        .chars()
        .last()
        .is_some_and(|character| character.is_ascii_digit())
    {
        format!("{device}p")
    } else {
        device.to_owned()
    }
}

fn mount_source_matches_device(source: &str, device: &str, canonical_device: &Path) -> bool {
    if source == device || is_partition_path(source, device) {
        return true;
    }

    let source_path = Path::new(source);
    let Ok(canonical_source) = fs::canonicalize(source_path) else {
        return false;
    };
    canonical_source == canonical_device
        || is_partition_path(
            &canonical_source.to_string_lossy(),
            &canonical_device.to_string_lossy(),
        )
}

fn is_partition_path(source: &str, device: &str) -> bool {
    let prefix = partition_prefix(device);
    let Some(rest) = source.strip_prefix(&prefix) else {
        return false;
    };
    !rest.is_empty() && rest.chars().all(|character| character.is_ascii_digit())
}

fn mib(bytes: u64) -> u64 {
    bytes.div_ceil(MB)
}

fn normalize_mountpoint(mount: &str) -> Result<String, DiskError> {
    let trimmed = mount.trim();
    if !trimmed.starts_with('/') {
        return Err(DiskError::InvalidLayout(format!(
            "mountpoint must be absolute: {trimmed}"
        )));
    }
    Ok(trimmed.to_owned())
}

fn normalize_zfs_mountpoint(mount: &str) -> Result<String, DiskError> {
    let trimmed = mount.trim();
    if trimmed == "none" {
        return Ok(trimmed.to_owned());
    }
    normalize_mountpoint(trimmed)
}

fn ordered_zfs_datasets<'a>(
    disk: &'a Disk,
    pool: &str,
    boot_pool: &str,
) -> Result<Vec<&'a ZfsDataset>, DiskError> {
    let mut datasets = disk.zfs.datasets.iter().collect::<Vec<_>>();
    for dataset in &datasets {
        let dataset_pool = dataset.pool.trim();
        if dataset_pool != pool && dataset_pool != boot_pool {
            return Err(DiskError::InvalidLayout(format!(
                "ZFS dataset {} belongs to unknown pool {dataset_pool}",
                dataset.name
            )));
        }
    }
    datasets.sort_by_key(|dataset| {
        let mount = dataset.mount.trim();
        let dataset_pool = dataset.pool.trim();
        let dataset_name = dataset.name.trim();
        let group = if dataset_pool == pool && dataset_name == "ROOT" {
            0
        } else if dataset_pool == pool && mount == "/" {
            1
        } else if dataset_pool == boot_pool {
            2
        } else {
            3
        };
        let depth = dataset.name.split('/').count();
        (group, depth)
    });
    Ok(datasets)
}

fn zfs_dataset_name(name: &str) -> Result<String, DiskError> {
    crate::util::zfs_dataset_name(name).map_err(DiskError::InvalidLayout)
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;
    use crate::manifest::{Disk, DiskLayout, DiskPartitions, Encryption, SwapConfig, GB};

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
        assert!(rendered.contains("zfs create -p -o mountpoint=/ -o canmount=noauto rpool/ROOT/os"));
        assert!(rendered.contains("zfs mount rpool/ROOT/os"));
        assert!(rendered.contains("zfs create -p -o mountpoint=/boot -o canmount=on bpool/BOOT/os"));
        assert!(rendered.contains("zfs create -p -o mountpoint=/var/cache/distfiles -o canmount=on rpool/gentoo/distfiles"));
        assert!(rendered.contains("zpool set bootfs=rpool/ROOT/os rpool"));
        assert!(rendered.contains("zpool set cachefile=/mnt/oxys/etc/zfs/zpool.cache rpool"));
        assert!(rendered.contains("zpool set cachefile=/mnt/oxys/etc/zfs/zpool.cache bpool"));
    }

    #[test]
    fn mounted_device_check_only_matches_numeric_partition_suffixes() {
        assert!(mount_source_matches_device(
            "/dev/sda1",
            "/dev/sda",
            Path::new("/dev/sda")
        ));
        assert!(!mount_source_matches_device(
            "/dev/sdab",
            "/dev/sda",
            Path::new("/dev/sda")
        ));
        assert!(mount_source_matches_device(
            "/dev/nvme0n1p1",
            "/dev/nvme0n1",
            Path::new("/dev/nvme0n1")
        ));
    }
}
