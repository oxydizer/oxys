use std::{
    fs,
    path::{Path, PathBuf},
    sync::mpsc::Sender,
};

use crate::{
    exec::{self, ExecError},
    kernel_cmdline::ResolvedKernelCmdline,
    manifest::{DiskLayout, SystemManifest},
};

use super::{
    blkid_value, run_chroot, write_file, zfs_dataset_name, DiskPartitionMap, SystemInstallError,
    SystemInstallEvent,
};

fn get_grub_relative_path(
    target_mount: &Path,
    path_in_chroot: &str,
) -> Result<String, SystemInstallError> {
    let target = target_mount.display().to_string();
    let output = exec::capture_command("chroot", &[&target, "grub-mkrelpath", path_in_chroot])?;
    if !output.status.success() {
        return Err(ExecError::StepFailed {
            step: format!("grub-mkrelpath {path_in_chroot}"),
            status: output.status,
        }
        .into());
    }
    let rel_path = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    Ok(rel_path)
}

fn get_grub_fs_uuid(
    target_mount: &Path,
    path_in_chroot: &str,
) -> Result<String, SystemInstallError> {
    let target = target_mount.display().to_string();
    let output = exec::capture_command(
        "chroot",
        &[&target, "grub-probe", "--target=fs_uuid", path_in_chroot],
    )?;
    if !output.status.success() {
        return Err(ExecError::StepFailed {
            step: format!("grub-probe --target=fs_uuid {path_in_chroot}"),
            status: output.status,
        }
        .into());
    }
    let uuid = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    Ok(uuid)
}

pub(super) fn generate_initramfs(
    target_mount: &Path,
    kver: &str,
    drivers: &[String],
    sender: &Sender<SystemInstallEvent>,
) -> Result<(), SystemInstallError> {
    let target = target_mount.display().to_string();
    let initramfs_path = format!("/boot/initramfs-{kver}.img");

    let mut args = vec![
        "dracut".to_owned(),
        "--force".to_owned(),
        "--kver".to_owned(),
        kver.to_owned(),
        "--add".to_owned(),
        "zfs".to_owned(),
    ];
    if !drivers.is_empty() {
        args.push("--add-drivers".to_owned());
        args.push(drivers.join(" "));
    }
    args.push(initramfs_path);

    run_chroot(&target, &args, sender)?;
    Ok(())
}

pub(super) fn derive_kernel_version(source_root: &Path) -> Result<String, SystemInstallError> {
    let boot_dir = source_root.join("boot");
    if let Ok(kernel_path) = newest_matching_file(&boot_dir, |name| name.starts_with("vmlinuz")) {
        let filename = kernel_path
            .file_name()
            .ok_or_else(|| {
                SystemInstallError::InvalidPlan(format!(
                    "invalid kernel filename: {}",
                    kernel_path.display()
                ))
            })?
            .to_string_lossy();
        if filename.starts_with("vmlinuz-") {
            return Ok(filename["vmlinuz-".len()..].to_owned());
        }
    }

    // Fallback: read /lib/modules under source_root
    let modules_dir = source_root.join("lib/modules");
    if let Ok(entries) = fs::read_dir(&modules_dir) {
        let mut candidates = Vec::new();
        for entry in entries {
            if let Ok(entry) = entry {
                if entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                    candidates.push(entry.file_name().to_string_lossy().into_owned());
                }
            }
        }
        candidates.sort();
        if let Some(kver) = candidates.last() {
            return Ok(kver.clone());
        }
    }

    Err(SystemInstallError::InvalidPlan(
        "Could not detect kernel version from source boot directory or lib/modules".to_owned(),
    ))
}

pub(super) fn install_boot_assets(
    target_mount: &Path,
    efi_mount: &str,
    sender: &Sender<SystemInstallEvent>,
) -> Result<(), SystemInstallError> {
    let boot = target_mount.join("boot");
    let esp_dir = target_mount
        .join(efi_mount.trim_start_matches('/'))
        .join("EFI/oxys");
    fs::create_dir_all(&esp_dir)?;

    let kernel = newest_matching_file(&boot, |name| name.starts_with("vmlinuz"))?;
    fs::copy(&kernel, esp_dir.join("vmlinuz"))?;
    let _ = sender.send(SystemInstallEvent::StepOutput {
        line: format!("copied {}", kernel.display()),
    });

    if let Some(initramfs) = newest_optional_matching_file(&boot, |name| {
        name.starts_with("initramfs") || name.starts_with("initrd")
    })? {
        fs::copy(&initramfs, esp_dir.join("initramfs.img"))?;
        let _ = sender.send(SystemInstallEvent::StepOutput {
            line: format!("copied {}", initramfs.display()),
        });
    } else {
        let _ = sender.send(SystemInstallEvent::StepOutput {
            line: "no initramfs found under target /boot; boot entry will omit initrd".to_owned(),
        });
    }

    Ok(())
}

pub(super) fn write_systemd_boot(
    manifest: &SystemManifest,
    resolved_kernel_cmdline: &ResolvedKernelCmdline,
    target_mount: &Path,
) -> Result<(), SystemInstallError> {
    let esp_root = target_mount.join(manifest.disk.partitions.efi.mount.trim_start_matches('/'));
    let loader_dir = esp_root.join("loader");
    let entries_dir = loader_dir.join("entries");
    fs::create_dir_all(&entries_dir)?;

    write_file(
        &loader_dir.join("loader.conf"),
        "default oxys.conf\ntimeout 3\neditor no\n",
    )?;

    let options = boot_options(manifest, resolved_kernel_cmdline)?;
    let initrd_line = if esp_root.join("EFI/oxys/initramfs.img").exists() {
        "initrd /EFI/oxys/initramfs.img\n"
    } else {
        ""
    };
    let entry = format!("title Oxys\nlinux /EFI/oxys/vmlinuz\n{initrd_line}options {options}\n");
    write_file(&entries_dir.join("oxys.conf"), &entry)
}

pub(super) fn write_grub_config(
    manifest: &SystemManifest,
    resolved_kernel_cmdline: &ResolvedKernelCmdline,
    target_mount: &Path,
) -> Result<(), SystemInstallError> {
    let grub_dir = target_mount.join("boot/grub");
    fs::create_dir_all(&grub_dir)?;

    if manifest.disk.layout == DiskLayout::Zfs {
        return write_zfs_grub_config(manifest, resolved_kernel_cmdline, target_mount, &grub_dir);
    }

    // The kernel and initramfs are copied onto the ESP under /EFI/oxys by the
    // shared InstallBootAssets step, so the GRUB entry points at the same
    // location as the systemd-boot entry. `search` re-roots at the ESP by its
    // filesystem UUID before loading the kernel.
    let esp_root = target_mount.join(manifest.disk.partitions.efi.mount.trim_start_matches('/'));
    let parts = DiskPartitionMap::from_disk(&manifest.disk);
    let esp_uuid = blkid_value(&parts.efi, "UUID")?;
    let options = boot_options(manifest, resolved_kernel_cmdline)?;

    let config = render_grub_config(
        &esp_uuid,
        &options,
        esp_root.join("EFI/oxys/initramfs.img").exists(),
    );
    write_file(&grub_dir.join("grub.cfg"), &config)
}

fn render_grub_config(esp_uuid: &str, options: &str, has_initramfs: bool) -> String {
    let mut lines = vec![
        "# generated by oxys - do not edit manually".to_owned(),
        "set timeout=3".to_owned(),
        "set default=0".to_owned(),
        String::new(),
        "menuentry \"Oxys\" {".to_owned(),
        format!("    search --no-floppy --fs-uuid --set=root {esp_uuid}"),
        format!("    linux /EFI/oxys/vmlinuz {options}"),
    ];
    if has_initramfs {
        lines.push("    initrd /EFI/oxys/initramfs.img".to_owned());
    }
    lines.push("}".to_owned());
    lines.push(String::new());

    lines.join("\n")
}

fn write_zfs_grub_config(
    manifest: &SystemManifest,
    resolved_kernel_cmdline: &ResolvedKernelCmdline,
    target_mount: &Path,
    grub_dir: &Path,
) -> Result<(), SystemInstallError> {
    let boot = target_mount.join("boot");
    let kernel = newest_matching_file(&boot, |name| name.starts_with("vmlinuz"))?;
    let kernel_name = kernel
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| SystemInstallError::InvalidPlan("kernel path is not UTF-8".to_owned()))?;

    // Use grub-mkrelpath to find the GRUB-compatible path relative to bpool root
    let kernel_path_in_chroot = format!("/boot/{kernel_name}");
    let grub_kernel_path = get_grub_relative_path(target_mount, &kernel_path_in_chroot)?;

    // Probe the FS UUID of /boot (bpool)
    let boot_uuid = get_grub_fs_uuid(target_mount, "/boot")?;

    let initramfs = newest_optional_matching_file(&boot, |name| {
        name.starts_with("initramfs") || name.starts_with("initrd")
    })?;
    let options = boot_options(manifest, resolved_kernel_cmdline)?;

    let mut lines = vec![
        "# generated by oxys - do not edit manually".to_owned(),
        "insmod zfs".to_owned(),
        "set timeout=3".to_owned(),
        "set default=0".to_owned(),
        String::new(),
        "menuentry \"Oxys\" {".to_owned(),
        format!("    search --no-floppy --fs-uuid --set=root {boot_uuid}"),
        format!("    linux {grub_kernel_path} {options}"),
    ];
    if let Some(initramfs) = initramfs {
        let initramfs_name = initramfs
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(|| {
                SystemInstallError::InvalidPlan("initramfs path is not UTF-8".to_owned())
            })?;
        let initramfs_path_in_chroot = format!("/boot/{initramfs_name}");
        let grub_initramfs_path = get_grub_relative_path(target_mount, &initramfs_path_in_chroot)?;
        lines.push(format!("    initrd {grub_initramfs_path}"));
    }
    lines.push("}".to_owned());
    lines.push(String::new());

    write_file(&grub_dir.join("grub.cfg"), &lines.join("\n"))
}

fn boot_options(
    manifest: &SystemManifest,
    resolved_kernel_cmdline: &ResolvedKernelCmdline,
) -> Result<String, SystemInstallError> {
    let mut options = Vec::new();
    match manifest.disk.layout {
        DiskLayout::Ext4 => {
            let parts = DiskPartitionMap::from_disk(&manifest.disk);
            let root_uuid = blkid_value(&parts.root, "UUID")?;
            options.push(format!("root=UUID={root_uuid}"));
            options.push("rw".to_owned());
        }
        DiskLayout::Zfs => {
            let root_dataset = manifest
                .disk
                .zfs
                .datasets
                .iter()
                .find(|dataset| {
                    dataset.pool.trim() == manifest.disk.zfs.pool.trim()
                        && dataset.mount.trim() == "/"
                })
                .ok_or_else(|| {
                    SystemInstallError::InvalidPlan(
                        "ZFS boot entry needs a dataset mounted at /".to_owned(),
                    )
                })?;
            options.push(format!(
                "root=ZFS={}/{}",
                manifest.disk.zfs.pool,
                zfs_dataset_name(&root_dataset.name)?
            ));
            options.push("rw".to_owned());
        }
        layout => return Err(SystemInstallError::UnsupportedLayout(layout)),
    }
    options.extend(resolved_kernel_cmdline.values().map(str::to_owned));
    Ok(options.join(" "))
}

/// Read a filesystem attribute (UUID, TYPE, ...) off a partition we just
/// created and formatted.
///
/// The critical detail is the `-p` (low-level probe) flag. Plain
/// `blkid <device>` does *not* read the disk — it returns whatever is in
/// blkid's cache (`/run/blkid/blkid.tab` plus the udev database). When
/// `sgdisk` first creates a partition, udev probes it while it is still empty
/// and caches "no filesystem". `mkfs.ext4`/`mkfs.vfat` then write the
/// superblock, but nothing re-probes the partition, so the cache still says
/// "no UUID". A cached `blkid` therefore returns exit 2 ("token not found")
/// *deterministically*, forever — retrying or sleeping never helps because
/// every call reads the same stale cache. `-p` bypasses the cache entirely and
/// probes the superblock straight off the device node, which always reflects
/// the freshly written filesystem.
///
/// The short retry is only a guard against the unrelated, transient case where
/// the device node itself is momentarily busy right after partitioning; a
/// best-effort `udevadm settle` up front lets udev finish creating nodes.
fn newest_matching_file<F>(dir: &Path, predicate: F) -> Result<PathBuf, SystemInstallError>
where
    F: Fn(&str) -> bool,
{
    newest_optional_matching_file(dir, predicate)?.ok_or_else(|| {
        SystemInstallError::InvalidPlan(format!("no matching kernel found under {}", dir.display()))
    })
}

fn newest_optional_matching_file<F>(
    dir: &Path,
    predicate: F,
) -> Result<Option<PathBuf>, SystemInstallError>
where
    F: Fn(&str) -> bool,
{
    let mut newest = None;
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let metadata = entry.metadata()?;
        if !metadata.is_file() {
            continue;
        }
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if !predicate(&name) {
            continue;
        }
        let modified = metadata.modified().ok();
        let replace = newest
            .as_ref()
            .and_then(|(_, current)| *current)
            .zip(modified)
            .map(|(current, candidate)| candidate > current)
            .unwrap_or_else(|| newest.is_none());
        if replace {
            newest = Some((entry.path(), modified));
        }
    }

    Ok(newest.map(|(path, _)| path))
}

#[cfg(test)]
mod tests {
    use crate::{
        kernel_cmdline::resolve_kernel_cmdline,
        manifest::{Kernel, SystemManifest},
    };

    #[test]
    fn grub_renders_resolved_arguments_in_order() {
        let manifest = SystemManifest {
            kernel: Kernel {
                cmdline: vec!["quiet".into(), "loglevel=3".into(), "quiet".into()],
            },
            ..SystemManifest::default()
        };
        let resolved = resolve_kernel_cmdline(&manifest).unwrap();

        let options = format!(
            "root=UUID=test rw {}",
            resolved.values().collect::<Vec<_>>().join(" ")
        );
        let config = super::render_grub_config("esp-test", &options, true);

        assert!(config.contains("linux /EFI/oxys/vmlinuz root=UUID=test rw quiet loglevel=3"));
        assert!(config.contains("search --no-floppy --fs-uuid --set=root esp-test"));
        assert!(config.contains("initrd /EFI/oxys/initramfs.img"));
    }
}
