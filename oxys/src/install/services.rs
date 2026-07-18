use std::{collections::HashSet, fs, path::Path, sync::mpsc::Sender};

use crate::{exec, manifest::SystemManifest};

use super::{SystemInstallError, SystemInstallEvent};

pub(super) fn activate_systemd_services(
    manifest: &SystemManifest,
    target_mount: &Path,
    sender: &Sender<SystemInstallEvent>,
) -> Result<(), SystemInstallError> {
    if !manifest.services.enabled.is_empty() {
        let mut args = vec![
            "--root".to_owned(),
            target_mount.display().to_string(),
            "enable".to_owned(),
        ];
        args.extend(manifest.services.enabled.iter().cloned());
        exec::run_command("Enable systemd services", "systemctl", &args, sender)?;
    }

    if !manifest.services.disabled.is_empty() {
        let mut args = vec![
            "--root".to_owned(),
            target_mount.display().to_string(),
            "disable".to_owned(),
        ];
        args.extend(manifest.services.disabled.iter().cloned());
        exec::run_command("Disable systemd services", "systemctl", &args, sender)?;
    }

    Ok(())
}

/// Enable/disable OpenRC services offline by managing the runlevel symlinks
/// directly, exactly as `rc-update add/del <name> default` would. This avoids
/// chrooting into the freshly copied target just to run `rc-update`.
pub fn activate_openrc_services(
    manifest: &SystemManifest,
    target_mount: &Path,
    sender: &Sender<SystemInstallEvent>,
) -> Result<(), SystemInstallError> {
    if openrc_enabled_service_count(manifest) == 0 {
        let mut enabled = manifest.services.enabled.clone();
        if manifest.disk.layout == crate::manifest::DiskLayout::Zfs {
            enabled.extend(["zfs-import".to_owned(), "zfs-mount".to_owned()]);
        }
        for service in enabled {
            let runlevel = if matches!(service.as_str(), "zfs-import" | "zfs-mount") {
                "boot"
            } else {
                "default"
            };
            let dir = target_mount.join("etc/runlevels").join(runlevel);
            fs::create_dir_all(&dir)?;
            let link = dir.join(&service);
            if fs::symlink_metadata(&link).is_ok() {
                fs::remove_file(&link)?;
            }
            std::os::unix::fs::symlink(format!("/etc/init.d/{service}"), link)?;
        }
        for service in &manifest.services.disabled {
            for runlevel in ["default", "boot"] {
                let link = target_mount
                    .join("etc/runlevels")
                    .join(runlevel)
                    .join(service);
                if fs::symlink_metadata(&link).is_ok() {
                    fs::remove_file(link)?;
                }
            }
        }
        return Ok(());
    }
    validate_authoritative_openrc_services(manifest, target_mount)?;
    for (runlevel, desired) in manifest.services.openrc.runlevels() {
        let runlevel_dir = target_mount.join("etc/runlevels").join(runlevel);
        fs::create_dir_all(&runlevel_dir)?;
        for entry in fs::read_dir(&runlevel_dir)? {
            let entry = entry?;
            if !desired
                .iter()
                .any(|service| entry.file_name() == service.as_str())
            {
                let path = entry.path();
                fs::remove_file(&path)?;
            }
        }
        for service in desired {
            let link = runlevel_dir.join(service);
            // Replace any existing entry so enabling stays idempotent, including
            // when a previous run left a broken symlink behind.
            if fs::symlink_metadata(&link).is_ok() {
                fs::remove_file(&link)?;
            }
            std::os::unix::fs::symlink(format!("/etc/init.d/{service}"), &link)?;
            let _ = sender.send(SystemInstallEvent::StepOutput {
                line: format!("enabled {service} ({runlevel} runlevel)"),
            });
        }
    }

    Ok(())
}

fn validate_authoritative_openrc_services(
    manifest: &SystemManifest,
    target_mount: &Path,
) -> Result<(), SystemInstallError> {
    for (runlevel, desired) in manifest.services.openrc.runlevels() {
        let mut seen = HashSet::new();
        for service in desired {
            if service.is_empty() || service == "." || service == ".." || service.contains('/') {
                return Err(SystemInstallError::InvalidPlan(format!(
                    "invalid OpenRC service name {service:?} in {runlevel}"
                )));
            }
            if !seen.insert(service) {
                return Err(SystemInstallError::InvalidPlan(format!(
                    "OpenRC service {service:?} is listed more than once in {runlevel}"
                )));
            }
            let init_script = target_mount.join("etc/init.d").join(service);
            if !init_script.is_file() {
                return Err(SystemInstallError::InvalidPlan(format!(
                    "OpenRC service {service:?} is listed in {runlevel} but {} is missing",
                    init_script.display()
                )));
            }
        }

        let runlevel_dir = target_mount.join("etc/runlevels").join(runlevel);
        if runlevel_dir.is_dir() {
            for entry in fs::read_dir(&runlevel_dir)? {
                let entry = entry?;
                if entry.file_type()?.is_dir() {
                    return Err(SystemInstallError::InvalidPlan(format!(
                        "unexpected directory in OpenRC runlevel: {}",
                        entry.path().display()
                    )));
                }
            }
        }
    }
    Ok(())
}

pub(super) fn openrc_enabled_service_count(manifest: &SystemManifest) -> usize {
    manifest
        .services
        .openrc
        .runlevels()
        .map(|(_, services)| services.len())
        .sum()
}
