use std::{fs, io::Write, path::Path, sync::mpsc::Sender, thread, time::Duration};

use crate::{
    exec::{self, ExecError},
    manifest::{Disk, DiskLayout, ResolvedSwap, SystemManifest},
};

use super::{SystemInstallError, SystemInstallEvent};

pub(crate) fn run_chroot(
    target: &str,
    args: &[String],
    sender: &Sender<SystemInstallEvent>,
) -> Result<(), SystemInstallError> {
    let mut argv = Vec::with_capacity(args.len() + 1);
    argv.push(target.to_owned());
    argv.extend(args.iter().cloned());

    let output = exec::capture_command("chroot", &argv)?;
    emit_command_output(&output.stdout, sender);
    emit_command_output(&output.stderr, sender);
    if !output.status.success() {
        return Err(ExecError::StepFailed {
            step: format!("chroot {}", args.join(" ")),
            status: output.status,
        }
        .into());
    }
    Ok(())
}

fn run_host_command(
    program: &str,
    args: &[&str],
    sender: &Sender<SystemInstallEvent>,
) -> Result<(), SystemInstallError> {
    let output = exec::capture_command(program, args)?;
    emit_command_output(&output.stdout, sender);
    emit_command_output(&output.stderr, sender);
    if !output.status.success() {
        return Err(ExecError::StepFailed {
            step: format!("{} {}", program, args.join(" ")),
            status: output.status,
        }
        .into());
    }
    Ok(())
}

fn emit_command_output(output: &[u8], sender: &Sender<SystemInstallEvent>) {
    for line in String::from_utf8_lossy(output).lines() {
        let _ = sender.send(SystemInstallEvent::StepOutput {
            line: line.to_owned(),
        });
    }
}

pub(crate) fn bind_mount_pseudo(
    target_mount: &Path,
    sender: &Sender<SystemInstallEvent>,
) -> Result<(), SystemInstallError> {
    let mounts = [
        ("/dev", "dev"),
        ("/sys", "sys"),
        ("/proc", "proc"),
        ("/run", "run"),
    ];

    for &(source, target_sub) in &mounts {
        let dest = target_mount.join(target_sub);
        let dest_str = dest.display().to_string();

        let _ = sender.send(SystemInstallEvent::StepOutput {
            line: format!("Mounting {source} to {dest_str}"),
        });

        run_host_command("mount", &["--rbind", source, &dest_str], sender)?;
        run_host_command("mount", &["--make-rslave", &dest_str], sender)?;
    }

    Ok(())
}

fn unmount_pseudo(
    target_mount: &Path,
    sender: &Sender<SystemInstallEvent>,
) -> Result<(), SystemInstallError> {
    let mounts = ["run", "proc", "sys", "dev"];
    let mut first_error = None;

    for target_sub in &mounts {
        let dest = target_mount.join(target_sub);
        let dest_str = dest.display().to_string();

        let _ = sender.send(SystemInstallEvent::StepOutput {
            line: format!("Unmounting {dest_str}"),
        });

        if let Err(err) = run_host_command("umount", &["-R", &dest_str], sender) {
            let _ = sender.send(SystemInstallEvent::StepOutput {
                line: format!("Warning: failed to unmount {dest_str}: {err}"),
            });
            if first_error.is_none() {
                first_error = Some(err);
            }
        }
    }

    first_error.map_or(Ok(()), Err)
}

pub(crate) fn finalize_install(
    manifest: &SystemManifest,
    resolved_swap: &ResolvedSwap,
    target_mount: &Path,
    sender: &Sender<SystemInstallEvent>,
) -> Result<(), SystemInstallError> {
    let mut first_error = None;
    let parts = DiskPartitionMap::from_disk_with_swap(&manifest.disk, resolved_swap);
    if let Some(swap_part) = &parts.swap {
        let _ = sender.send(SystemInstallEvent::StepOutput {
            line: format!("Disabling swap on {swap_part}"),
        });
        if let Err(err) = run_host_command("swapoff", &[swap_part], sender) {
            let _ = sender.send(SystemInstallEvent::StepOutput {
                line: format!("Warning: failed to disable swap on {swap_part}: {err}"),
            });
            if first_error.is_none() {
                first_error = Some(err);
            }
        }
    }

    let _ = sender.send(SystemInstallEvent::StepOutput {
        line: "Unmounting pseudo filesystems".to_owned(),
    });
    if let Err(err) = unmount_pseudo(target_mount, sender) {
        let _ = sender.send(SystemInstallEvent::StepOutput {
            line: format!("Warning: failed to unmount pseudo filesystems: {err}"),
        });
        if first_error.is_none() {
            first_error = Some(err);
        }
    }

    let dest_str = target_mount.display().to_string();
    let _ = sender.send(SystemInstallEvent::StepOutput {
        line: format!("Recursively unmounting target {dest_str}"),
    });
    if let Err(err) = run_host_command("umount", &["-R", &dest_str], sender) {
        let _ = sender.send(SystemInstallEvent::StepOutput {
            line: format!("Warning: failed to recursively unmount {dest_str}: {err}"),
        });
        if first_error.is_none() {
            first_error = Some(err);
        }
    }

    if manifest.disk.layout == DiskLayout::Zfs {
        for pool in [&manifest.disk.zfs.boot_pool, &manifest.disk.zfs.pool] {
            let pool = pool.trim();
            if pool.is_empty() {
                continue;
            }
            let _ = sender.send(SystemInstallEvent::StepOutput {
                line: format!("Exporting ZFS pool {pool}"),
            });
            if let Err(err) = run_host_command("zpool", &["export", pool], sender) {
                let _ = sender.send(SystemInstallEvent::StepOutput {
                    line: format!("Warning: failed to export ZFS pool {pool}: {err}"),
                });
                if first_error.is_none() {
                    first_error = Some(err);
                }
            }
        }
    }

    first_error.map_or(Ok(()), Err)
}

pub(crate) fn blkid_value(device: &str, key: &str) -> Result<String, SystemInstallError> {
    const MAX_ATTEMPTS: u32 = 10;
    const RETRY_DELAY: Duration = Duration::from_millis(250);

    let mut last_err = None;
    for attempt in 0..MAX_ATTEMPTS {
        if attempt == 0 {
            let _ = exec::capture_command("udevadm", ["settle", "--timeout=5"]);
        } else {
            thread::sleep(RETRY_DELAY);
        }
        match try_blkid_value(device, key) {
            Ok(value) => return Ok(value),
            Err(err) => last_err = Some(err),
        }
    }
    Err(last_err.expect("loop runs at least once"))
}

fn try_blkid_value(device: &str, key: &str) -> Result<String, SystemInstallError> {
    let output = exec::capture_command("blkid", ["-p", "-s", key, "-o", "value", device])?;
    if !output.status.success() {
        return Err(ExecError::StepFailed {
            step: format!("read {key} for {device}"),
            status: output.status,
        }
        .into());
    }

    let value = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    if value.is_empty() {
        return Err(SystemInstallError::InvalidPlan(format!(
            "blkid returned empty {key} for {device}"
        )));
    }
    Ok(value)
}

pub(crate) fn write_file(path: &Path, contents: &str) -> Result<(), SystemInstallError> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut file = fs::File::create(path)?;
    file.write_all(contents.as_bytes())?;
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DiskPartitionMap {
    pub(crate) efi: String,
    pub(crate) swap: Option<String>,
    pub(crate) root: String,
    pub(crate) home: Option<String>,
}

impl DiskPartitionMap {
    pub(crate) fn from_disk_with_swap(disk: &Disk, resolved_swap: &ResolvedSwap) -> Self {
        if disk.layout == DiskLayout::Zfs {
            let swap = resolved_swap
                .disk
                .as_ref()
                .map(|_| partition_path(&disk.device, 3));
            let root_part = if swap.is_some() { 4 } else { 3 };
            return Self {
                efi: partition_path(&disk.device, 1),
                swap,
                root: partition_path(&disk.device, root_part),
                home: None,
            };
        }

        let mut next_part = 2;
        let swap = match &resolved_swap.disk {
            Some(_) => {
                let part = partition_path(&disk.device, next_part);
                next_part += 1;
                Some(part)
            }
            None => None,
        };
        let root = partition_path(&disk.device, next_part);
        let home = if disk.layout == DiskLayout::Ext4 && disk.ext4.separate_home {
            Some(partition_path(&disk.device, next_part + 1))
        } else {
            None
        };

        Self {
            efi: partition_path(&disk.device, 1),
            swap,
            root,
            home,
        }
    }
}

fn partition_path(device: &str, number: usize) -> String {
    crate::util::partition_path(device, number)
}

pub(crate) fn zfs_dataset_name(name: &str) -> Result<String, SystemInstallError> {
    crate::util::zfs_dataset_name(name).map_err(SystemInstallError::InvalidPlan)
}
