use std::path::Path;

use crate::manifest::{Disk, ResolvedSwap, ZfsDataset};

use super::apply::partition_path;
use super::{DiskError, DiskStep, mib, swap_partition_step};

pub(super) fn plan_zfs(
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

pub(super) fn zfs_swap_partition(
    resolved_swap: &ResolvedSwap,
    number: usize,
) -> Option<(usize, u64)> {
    resolved_swap.disk.as_ref().map(|swap| (number, swap.size))
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
