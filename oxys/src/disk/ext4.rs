use std::path::Path;

use crate::manifest::{Disk, SwapConfig};

use super::apply::partition_path;
use super::{mib, swap_partition_step, wipe_signatures_step, DiskError, DiskStep};

pub(super) fn plan_swap_partition(
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

pub(super) fn plan_ext4(
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
