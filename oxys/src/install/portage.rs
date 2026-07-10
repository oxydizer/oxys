use std::{fs, path::Path, sync::mpsc::Sender};

use crate::{
    exec,
    manifest::SystemManifest,
    use_resolver::{plan_portage, run_emerge_chroot, write_portage_plan_config, EmergeLine},
};

use super::{SystemInstallError, SystemInstallEvent};

pub(super) fn emerge_manifest_packages(
    manifest: &SystemManifest,
    target_mount: &Path,
    sender: &Sender<SystemInstallEvent>,
) -> Result<(), SystemInstallError> {
    // Installing the manifest's packages is best-effort: the base system is
    // already in place from the rsync, so any problem here is logged and skipped
    // rather than aborted. Aborting would also strand the target still mounted
    // (Finalize never runs), which then blocks the next install attempt on the
    // "refusing to provision mounted/live disk" preflight.
    let portage_tree = target_mount.join("var/db/repos");
    let gentoo_tree = portage_tree.join("gentoo");
    if !gentoo_tree.join("metadata/md5-cache").is_dir() {
        let _ = sender.send(SystemInstallEvent::StepOutput {
            line: format!(
                "Warning: target Portage tree missing/incomplete ({}); skipping package install",
                gentoo_tree.display()
            ),
        });
        return Ok(());
    }

    let cache_dir = target_mount.join("var/cache/oxys/use-resolver");
    let plan = match plan_portage(manifest, &portage_tree, &cache_dir) {
        Ok(plan) => plan,
        Err(error) => {
            let _ = sender.send(SystemInstallEvent::StepOutput {
                line: format!(
                    "Warning: package planning failed; skipping package install: {error}"
                ),
            });
            return Ok(());
        }
    };
    if !plan.resolution.conflicts.is_empty() {
        let conflicts = plan
            .resolution
            .conflicts
            .iter()
            .map(|conflict| {
                format!(
                    "{}: {} ({})",
                    conflict.flag,
                    conflict.reason,
                    conflict.packages.join(", ")
                )
            })
            .collect::<Vec<_>>()
            .join("; ");
        let _ = sender.send(SystemInstallEvent::StepOutput {
            line: format!(
                "Warning: package plan has unresolved conflicts; skipping package install: {conflicts}"
            ),
        });
        return Ok(());
    }

    if let Err(error) = write_portage_plan_config(&plan, &target_mount.join("etc/portage")) {
        let _ = sender.send(SystemInstallEvent::StepOutput {
            line: format!(
                "Warning: could not write Portage config; skipping package install: {error}"
            ),
        });
        return Ok(());
    }
    let _ = sender.send(SystemInstallEvent::StepOutput {
        line: format!("planned package target(s): {}", plan.targets.join(", ")),
    });

    ensure_target_resolv_conf(target_mount, sender);

    if !chroot_has_connectivity(target_mount, sender) {
        let _ = sender.send(SystemInstallEvent::StepOutput {
            line: "Network preflight failed; skipping manifest package emerge".to_owned(),
        });
        return Ok(());
    }

    let mut stream = match run_emerge_chroot(
        &plan.targets,
        target_mount,
        Path::new("/var/tmp"),
        plan.manifest.compiler.emerge_jobs,
        plan.use_binpkgs,
    ) {
        Ok(stream) => stream,
        Err(error) => {
            let _ = sender.send(SystemInstallEvent::StepOutput {
                line: format!("Warning: failed to start package emerge: {error}"),
            });
            return Ok(());
        }
    };

    for line in &mut stream {
        send_emerge_line(line, sender);
    }

    if let Err(error) = stream.wait() {
        let _ = sender.send(SystemInstallEvent::StepOutput {
            line: format!("Warning: manifest package emerge failed: {error}"),
        });
    }

    Ok(())
}

/// Ensure the target has a usable `/etc/resolv.conf` before we emerge into it.
///
/// The target's resolv.conf is rsync'd from the live system, but on many setups
/// `/etc/resolv.conf` is a symlink into `/run` (NetworkManager, systemd-resolved)
/// -- and `/run` is excluded from the rsync, so the target inherits a *dangling*
/// symlink and the chroot has no DNS. That makes both the connectivity preflight
/// and emerge's fetches fail, silently skipping every package. Reading the host
/// file follows the symlink to its real content; we write that through as a plain
/// file (replacing any dangling link) so name resolution works inside the chroot.
/// Best-effort: on failure we log and let the connectivity preflight decide.
fn ensure_target_resolv_conf(target_mount: &Path, sender: &Sender<SystemInstallEvent>) {
    let target_resolv = target_mount.join("etc/resolv.conf");
    match fs::read("/etc/resolv.conf") {
        Ok(contents) => {
            // Drop any inherited (possibly dangling) symlink before writing.
            let _ = fs::remove_file(&target_resolv);
            if let Some(parent) = target_resolv.parent() {
                let _ = fs::create_dir_all(parent);
            }
            if let Err(error) = fs::write(&target_resolv, contents) {
                let _ = sender.send(SystemInstallEvent::StepOutput {
                    line: format!("Warning: could not write target resolv.conf: {error}"),
                });
            }
        }
        Err(error) => {
            let _ = sender.send(SystemInstallEvent::StepOutput {
                line: format!("Warning: no readable host resolv.conf for target DNS: {error}"),
            });
        }
    }
}

fn chroot_has_connectivity(target_mount: &Path, sender: &Sender<SystemInstallEvent>) -> bool {
    let target = target_mount.display().to_string();
    match exec::capture_command(
        "chroot",
        [&target, "getent", "hosts", "distfiles.gentoo.org"],
    ) {
        Ok(output) if output.status.success() => {
            for line in String::from_utf8_lossy(&output.stdout).lines() {
                let _ = sender.send(SystemInstallEvent::StepOutput {
                    line: format!("network preflight: {line}"),
                });
            }
            true
        }
        Ok(output) => {
            for line in String::from_utf8_lossy(&output.stderr).lines() {
                let _ = sender.send(SystemInstallEvent::StepOutput {
                    line: format!("network preflight: {line}"),
                });
            }
            false
        }
        Err(error) => {
            let _ = sender.send(SystemInstallEvent::StepOutput {
                line: format!("network preflight failed: {error}"),
            });
            false
        }
    }
}

fn send_emerge_line(line: EmergeLine, sender: &Sender<SystemInstallEvent>) {
    let rendered = match line {
        EmergeLine::BuildStart { package } => format!("emerging {package}"),
        EmergeLine::BuildProgress { package, line } => package
            .map(|package| format!("{package}: {line}"))
            .unwrap_or(line),
        EmergeLine::BuildComplete { package } => format!("completed {package}"),
        EmergeLine::FetchStart { package } => format!("fetching {package}"),
        EmergeLine::FetchComplete { package } => format!("fetched {package}"),
        EmergeLine::Error { package, message } => package
            .map(|package| format!("{package}: {message}"))
            .unwrap_or(message),
    };

    let _ = sender.send(SystemInstallEvent::StepOutput { line: rendered });
}
