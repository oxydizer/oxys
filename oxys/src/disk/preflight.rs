//! Destructive-install preflight.
//!
//! Runs before any wipe/partition step. The goal is not a perfect storage
//! inventory — just enough real-hardware checks that we refuse to erase a disk
//! that is clearly in use or too small for a live-root rsync.

use std::{fs, path::Path};

use crate::manifest::{Disk, DiskLayout, GB, ResolvedSwap, SystemManifest};

use super::{DiskError, apply};

/// Floor for whole-disk installs. Desktop rsync of the live root routinely
/// needs more than ~8 GiB (see AGENTS.md); refuse below this so wipe never
/// starts on a disk that will fail mid-copy with ENOSPC.
pub const MIN_INSTALL_BYTES: u64 = 12 * GB;
// Tripwire: the install floor must never drop below 12 GiB.
const _: () = assert!(MIN_INSTALL_BYTES >= 12 * GB);

/// Verify `disk.device` is a real, writable whole-disk candidate that is not
/// currently mounted, swapped on, held by LVM/RAID/dm, or undersized.
pub fn preflight(disk: &Disk) -> Result<(), DiskError> {
    let manifest = SystemManifest {
        disk: disk.clone(),
        ..SystemManifest::default()
    };
    let resolved_swap = manifest.resolved_swap()?;
    preflight_with_swap(disk, &resolved_swap)
}

pub fn preflight_with_swap(disk: &Disk, resolved_swap: &ResolvedSwap) -> Result<(), DiskError> {
    let device = disk.device.trim();
    if device.is_empty() {
        return Err(DiskError::MissingDevice);
    }

    let metadata = fs::metadata(device).map_err(|_| DiskError::DeviceMissing(device.to_owned()))?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::FileTypeExt;
        if !metadata.file_type().is_block_device() {
            return Err(DiskError::NotBlockDevice(device.to_owned()));
        }
    }

    let device_path = Path::new(device);
    let canonical_device = fs::canonicalize(device_path).unwrap_or_else(|_| device_path.into());
    let sys_name = block_sysfs_name(&canonical_device).ok_or_else(|| DiskError::DeviceBusy {
        device: device.to_owned(),
        reason: format!(
            "could not resolve {} to a /sys/block entry (is it a whole disk?)",
            canonical_device.display()
        ),
    })?;

    refuse_if_read_only(device, &sys_name)?;
    refuse_if_mounted(device, &canonical_device)?;
    refuse_if_swapped(device, &canonical_device)?;
    refuse_if_has_holders(device, &sys_name)?;
    refuse_if_too_small(device, &sys_name, disk, resolved_swap)?;

    Ok(())
}

fn refuse_if_read_only(device: &str, sys_name: &str) -> Result<(), DiskError> {
    let ro = fs::read_to_string(Path::new("/sys/block").join(sys_name).join("ro"))
        .map(|value| value.trim() == "1")
        .unwrap_or(false);
    if ro {
        return Err(DiskError::DeviceBusy {
            device: device.to_owned(),
            reason: "disk is read-only (hardware write-protect or firmware lock)".to_owned(),
        });
    }
    Ok(())
}

fn refuse_if_mounted(device: &str, canonical_device: &Path) -> Result<(), DiskError> {
    let mounts = fs::read_to_string("/proc/mounts").unwrap_or_default();
    for line in mounts.lines() {
        let mut fields = line.split_whitespace();
        let Some(source) = fields.next() else {
            continue;
        };
        let Some(target) = fields.next() else {
            continue;
        };
        if apply::mount_source_matches_device(source, device, canonical_device) {
            return Err(DiskError::DeviceBusy {
                device: device.to_owned(),
                reason: format!("mounted at {target} (source {source})"),
            });
        }
    }
    Ok(())
}

fn refuse_if_swapped(device: &str, canonical_device: &Path) -> Result<(), DiskError> {
    let swaps = fs::read_to_string("/proc/swaps").unwrap_or_default();
    for (idx, line) in swaps.lines().enumerate() {
        // Header: Filename Type Size Used Priority
        if idx == 0 {
            continue;
        }
        let mut fields = line.split_whitespace();
        let Some(source) = fields.next() else {
            continue;
        };
        if apply::mount_source_matches_device(source, device, canonical_device) {
            return Err(DiskError::DeviceBusy {
                device: device.to_owned(),
                reason: format!("active swap on {source}"),
            });
        }
    }
    Ok(())
}

/// LVM, md RAID, dm-crypt, multipath, etc. open the disk or a partition via
/// the kernel's holder graph. A non-empty `holders/` directory means wipefs
/// would race with an in-kernel consumer.
fn refuse_if_has_holders(device: &str, sys_name: &str) -> Result<(), DiskError> {
    let holders = collect_holders(sys_name);
    if holders.is_empty() {
        return Ok(());
    }
    let listed = holders.join(", ");
    Err(DiskError::DeviceBusy {
        device: device.to_owned(),
        reason: format!(
            "in use by kernel holder(s): {listed} (deactivate LVM/RAID/LUKS/multipath first)"
        ),
    })
}

fn collect_holders(sys_name: &str) -> Vec<String> {
    let mut found = Vec::new();
    let block_root = Path::new("/sys/block").join(sys_name);

    push_holder_names(&block_root.join("holders"), &mut found);

    // Partitions appear as subdirs of the whole-disk sysfs node (e.g. sda1,
    // nvme0n1p1). Each can have its own holders (md, dm-*).
    if let Ok(entries) = fs::read_dir(&block_root) {
        for entry in entries.filter_map(Result::ok) {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if !is_partition_sysfs_name(sys_name, &name) {
                continue;
            }
            push_holder_names(&entry.path().join("holders"), &mut found);
        }
    }

    found.sort();
    found.dedup();
    found
}

fn push_holder_names(holders_dir: &Path, out: &mut Vec<String>) {
    let Ok(entries) = fs::read_dir(holders_dir) else {
        return;
    };
    for entry in entries.filter_map(Result::ok) {
        let name = entry.file_name().to_string_lossy().into_owned();
        if !name.is_empty() {
            out.push(name);
        }
    }
}

fn is_partition_sysfs_name(disk: &str, name: &str) -> bool {
    if name == disk {
        return false;
    }
    // nvme0n1p1 / mmcblk0p1
    if let Some(rest) = name.strip_prefix(disk) {
        if let Some(rest) = rest.strip_prefix('p') {
            return !rest.is_empty() && rest.chars().all(|c| c.is_ascii_digit());
        }
        // sda1 style
        return !rest.is_empty() && rest.chars().all(|c| c.is_ascii_digit());
    }
    false
}

fn refuse_if_too_small(
    device: &str,
    sys_name: &str,
    disk: &Disk,
    resolved_swap: &ResolvedSwap,
) -> Result<(), DiskError> {
    let size = read_block_size_bytes(sys_name).unwrap_or(0);
    let mut required = MIN_INSTALL_BYTES.saturating_add(disk.partitions.efi.size);
    if let Some(swap) = &resolved_swap.disk {
        required = required.saturating_add(swap.size);
    }
    if disk.layout == DiskLayout::Zfs {
        required = required.saturating_add(disk.zfs.boot_pool_size);
    }
    if size < required {
        return Err(DiskError::DeviceTooSmall {
            device: device.to_owned(),
            have: format_bytes(size),
            need: format_bytes(required),
        });
    }
    Ok(())
}

fn read_block_size_bytes(sys_name: &str) -> Option<u64> {
    let sectors = fs::read_to_string(Path::new("/sys/block").join(sys_name).join("size"))
        .ok()?
        .trim()
        .parse::<u64>()
        .ok()?;
    sectors.checked_mul(512)
}

fn block_sysfs_name(canonical_device: &Path) -> Option<String> {
    let name = canonical_device.file_name()?.to_str()?.to_owned();
    if Path::new("/sys/block").join(&name).is_dir() {
        return Some(name);
    }
    // Canonical path might be a partition node (/dev/nvme0n1p2). Walk up to
    // the parent whole-disk name via sysfs for clearer errors when someone
    // passes a partition as the install target.
    let class = Path::new("/sys/class/block").join(&name);
    if let Ok(link) = fs::read_link(&class) {
        // e.g. ../../devices/.../block/nvme0n1/nvme0n1p2
        if let Some(parent) = link.parent().and_then(|p| p.file_name()) {
            let parent = parent.to_string_lossy().into_owned();
            if Path::new("/sys/block").join(&parent).is_dir() {
                // Treat selecting a partition as invalid — install needs a whole disk.
                return None;
            }
        }
    }
    None
}

fn format_bytes(bytes: u64) -> String {
    if bytes >= GB {
        format!("{:.1} GiB", bytes as f64 / GB as f64)
    } else if bytes >= 1024 * 1024 {
        format!("{:.0} MiB", bytes as f64 / (1024.0 * 1024.0))
    } else {
        format!("{bytes} B")
    }
}

/// Re-run preflight from a planned device string (used immediately before the
/// first destructive step so a race after the UI confirm still aborts cleanly).
pub(super) fn preflight_device(device: &str) -> Result<(), DiskError> {
    preflight(&Disk {
        device: device.to_owned(),
        ..Disk::default()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn partition_sysfs_names_match_common_schemes() {
        assert!(is_partition_sysfs_name("sda", "sda1"));
        assert!(is_partition_sysfs_name("sda", "sda12"));
        assert!(!is_partition_sysfs_name("sda", "sda"));
        assert!(!is_partition_sysfs_name("sda", "sdb1"));

        assert!(is_partition_sysfs_name("nvme0n1", "nvme0n1p1"));
        assert!(is_partition_sysfs_name("nvme0n1", "nvme0n1p12"));
        assert!(!is_partition_sysfs_name("nvme0n1", "nvme0n1"));
        assert!(!is_partition_sysfs_name("nvme0n1", "nvme0n1p"));
        assert!(!is_partition_sysfs_name("nvme0n1", "nvme1n1p1"));

        assert!(is_partition_sysfs_name("mmcblk0", "mmcblk0p1"));
    }

    #[test]
    fn format_bytes_uses_gib_for_install_floor() {
        assert_eq!(format_bytes(MIN_INSTALL_BYTES), "12.0 GiB");
        assert_eq!(format_bytes(0), "0 B");
    }

    #[test]
    fn preflight_rejects_missing_device() {
        let disk = Disk {
            device: "/dev/oxys-definitely-missing-disk".to_owned(),
            ..Disk::default()
        };
        let err = preflight(&disk).expect_err("missing device");
        assert!(matches!(err, DiskError::DeviceMissing(_)));
    }

    #[test]
    fn preflight_rejects_empty_device() {
        let err = preflight(&Disk::default()).expect_err("empty");
        assert!(matches!(err, DiskError::MissingDevice));
    }
}
