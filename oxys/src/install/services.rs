use std::{fs, path::Path, sync::mpsc::Sender};

use crate::{
    exec,
    manifest::{DiskLayout, InitSystem, SystemManifest},
};

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
pub(super) fn activate_openrc_services(
    manifest: &SystemManifest,
    target_mount: &Path,
    sender: &Sender<SystemInstallEvent>,
) -> Result<(), SystemInstallError> {
    for service in openrc_enabled_services(manifest) {
        let runlevel = openrc_runlevel_for_service(&service);
        let runlevel_dir = target_mount.join("etc/runlevels").join(runlevel);
        fs::create_dir_all(&runlevel_dir)?;
        let link = runlevel_dir.join(&service);
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

    for service in &manifest.services.disabled {
        for runlevel in ["default", "boot"] {
            let link = target_mount
                .join("etc/runlevels")
                .join(runlevel)
                .join(service);
            if fs::symlink_metadata(&link).is_ok() {
                fs::remove_file(&link)?;
                let _ = sender.send(SystemInstallEvent::StepOutput {
                    line: format!("disabled {service} ({runlevel} runlevel)"),
                });
            }
        }
    }

    Ok(())
}

pub(super) fn openrc_enabled_services(manifest: &SystemManifest) -> Vec<String> {
    let mut services = manifest.services.enabled.clone();
    if manifest.init_system == InitSystem::Openrc && manifest.disk.layout == DiskLayout::Zfs {
        for service in ["zfs-import", "zfs-mount"] {
            if !services.iter().any(|existing| existing == service) {
                services.push(service.to_owned());
            }
        }
    }
    services
}

fn openrc_runlevel_for_service(service: &str) -> &'static str {
    match service {
        "zfs-import" | "zfs-mount" => "boot",
        _ => "default",
    }
}
