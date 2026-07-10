use std::collections::HashMap;
use std::path::PathBuf;

use oxys::{
    apply_disk_plan, apply_system_install_plan,
    detect::DetectedDisk,
    manifest::{Disk, Password, Username, GB},
    plan_disk, plan_system_install, preflight, release_target_mounts, ProvisionEvent,
    SystemInstallEvent,
};
use tokio::sync::mpsc::UnboundedSender;

pub(crate) const TARGET_MOUNT: &str = "/mnt/oxys";

/// Control-line prefix the installer parses to drive the progress bar. Lines
/// that start with this are consumed by the UI and never shown in the log.
pub(crate) const PROGRESS_PREFIX: &str = "[[progress]] ";

/// Fraction of the bar (0..=100) allotted to the fast disk-provisioning phase.
/// The rest belongs to the long system-install phase (rsync, bootloader, …).
const DISK_BAND: u16 = 15;

fn send_progress(tx: &UnboundedSender<String>, percent: u16) {
    // Never report a full bar from here: 100% is reserved for the moment the
    // worker actually finishes and the channel closes, so the install screen
    // can't claim "complete" while steps are still running.
    let _ = tx.send(format!("{PROGRESS_PREFIX}{}", percent.min(99)));
}

pub(crate) fn install_permission_error() -> Option<String> {
    if running_as_root() {
        None
    } else {
        Some(
            "installer must be run as root; disk provisioning needs privileges for wipefs, partitioning, filesystems, mounts, and bootloader setup"
                .to_string(),
        )
    }
}

fn ensure_install_permissions(tx: &UnboundedSender<String>) -> bool {
    match install_permission_error() {
        Some(error) => {
            let _ = tx.send(format!("[error] {error}"));
            false
        }
        None => true,
    }
}

#[cfg(unix)]
fn running_as_root() -> bool {
    unsafe extern "C" {
        fn geteuid() -> u32;
    }

    unsafe { geteuid() == 0 }
}

#[cfg(not(unix))]
fn running_as_root() -> bool {
    false
}

pub(crate) fn format_disk(disk: &DetectedDisk) -> String {
    format!(
        "{} ({:.1} GiB {})",
        disk.device,
        disk.size as f64 / GB as f64,
        disk.model
    )
}

#[allow(dead_code)]
pub(crate) fn run_partition_plan(disk: Option<Disk>, tx: UnboundedSender<String>) {
    if !ensure_install_permissions(&tx) {
        return;
    }

    let Some(disk) = disk else {
        let _ = tx.send("[error] no installable disk selected".to_string());
        return;
    };
    match plan_disk(&disk, std::path::Path::new(TARGET_MOUNT)) {
        Ok(plan) => {
            let _ = tx.send(format!("[plan ] target {}", TARGET_MOUNT));
            for line in plan.render().lines() {
                let _ = tx.send(line.to_string());
            }
            let _ = tx.send("[ok   ] review the plan before continuing".to_string());
        }
        Err(error) => {
            let _ = tx.send(format!("[error] {error}"));
        }
    }
}

pub(crate) fn run_install(
    disk: Option<Disk>,
    manifest_path: Option<PathBuf>,
    prompted_usernames: HashMap<usize, String>,
    prompted_passwords: HashMap<String, String>,
    tx: UnboundedSender<String>,
) {
    if !ensure_install_permissions(&tx) {
        return;
    }

    let Some(disk) = disk else {
        let _ = tx.send("[error] no installable disk selected".to_string());
        return;
    };
    let Some(manifest_path) = manifest_path else {
        let _ = tx.send("[error] no compiled manifest available".to_string());
        return;
    };

    let mut manifest = match oxys::compile::load_manifest(&manifest_path) {
        Ok(manifest) => manifest,
        Err(error) => {
            let _ = tx.send(format!("[error] failed to load compiled manifest: {error}"));
            return;
        }
    };
    manifest.disk = disk;
    // Resolve interactively-collected names first: the password resolution
    // below keys off the concrete name, and install-time code assumes every
    // user's name is a `Username::Literal` by the time it runs.
    for (index, user) in manifest.users.iter_mut().enumerate() {
        if user.name == Username::Prompt {
            match prompted_usernames.get(&index) {
                Some(name) => user.name = Username::Literal(name.clone()),
                None => {
                    let _ = tx.send(format!(
                        "[warn ] no username collected for account {index}; locking the account"
                    ));
                    user.name = Username::Literal(format!("user{index}"));
                    user.password = Password::None;
                }
            }
        }
    }
    // Resolve interactively-collected passwords into concrete values. Secrets
    // arrive here in memory only and are never written back to manifest.toml.
    for user in &mut manifest.users {
        if user.password == Password::Prompt {
            match prompted_passwords.get(user.name.as_str()) {
                Some(secret) => user.password = Password::Plain(secret.clone()),
                None => {
                    let _ = tx.send(format!(
                        "[warn ] no password collected for {}; locking the account",
                        user.name.as_str()
                    ));
                    user.password = Password::None;
                }
            }
        }
    }
    let _ = tx.send(format!(
        "[ok   ] using compiled manifest {}",
        manifest_path.display()
    ));

    // Clear any leftover mounts from a previous aborted run at our own target
    // mount point, so the preflight guard below doesn't refuse a re-run.
    release_target_mounts(std::path::Path::new(TARGET_MOUNT));

    if let Err(error) = preflight(&manifest.disk) {
        let _ = tx.send(format!("[error] {error}"));
        return;
    }

    let plan = match plan_disk(&manifest.disk, std::path::Path::new(TARGET_MOUNT)) {
        Ok(plan) => plan,
        Err(error) => {
            let _ = tx.send(format!("[error] {error}"));
            return;
        }
    };
    let disk_total = plan.steps.len().max(1);
    let mut disk_done = 0usize;
    send_progress(&tx, 0);
    let mut stream = apply_disk_plan(&plan);
    for event in &mut stream {
        let completed = matches!(event, ProvisionEvent::StepComplete { .. });
        let _ = tx.send(format_provision_event(event));
        if completed {
            disk_done += 1;
            let pct = (disk_done as f32 / disk_total as f32 * DISK_BAND as f32) as u16;
            send_progress(&tx, pct.min(DISK_BAND));
        }
    }
    if let Err(error) = stream.wait() {
        let _ = tx.send(format!("[error] {error}"));
        return;
    }

    let system_plan = match plan_system_install(
        &manifest,
        std::path::Path::new("/"),
        std::path::Path::new(TARGET_MOUNT),
    ) {
        Ok(plan) => plan,
        Err(error) => {
            let _ = tx.send(format!("[error] {error}"));
            return;
        }
    };
    let system_total = system_plan.steps.len().max(1);
    let mut system_done = 0usize;
    let system_span = 99 - DISK_BAND;
    let mut stream = apply_system_install_plan(&system_plan);
    for event in &mut stream {
        let completed = matches!(event, SystemInstallEvent::StepComplete { .. });
        let _ = tx.send(format_system_install_event(event));
        if completed {
            system_done += 1;
            let pct =
                DISK_BAND + (system_done as f32 / system_total as f32 * system_span as f32) as u16;
            send_progress(&tx, pct);
        }
    }
    match stream.wait() {
        Ok(()) => {
            let _ = tx.send(format!("[ok   ] install complete at {TARGET_MOUNT}"));
        }
        Err(error) => {
            let _ = tx.send(format!("[error] {error}"));
        }
    }
}

fn format_provision_event(event: ProvisionEvent) -> String {
    match event {
        ProvisionEvent::StepStart { description } => format!("[run  ] {description}"),
        ProvisionEvent::StepOutput { line } => format!("[out  ] {line}"),
        ProvisionEvent::StepComplete { description } => format!("[ok   ] {description}"),
        ProvisionEvent::Error { step, message } => format!("[error] {step}: {message}"),
    }
}

fn format_system_install_event(event: SystemInstallEvent) -> String {
    match event {
        SystemInstallEvent::StepStart { description } => format!("[run  ] {description}"),
        SystemInstallEvent::StepOutput { line } => format!("[out  ] {line}"),
        SystemInstallEvent::StepComplete { description } => format!("[ok   ] {description}"),
        SystemInstallEvent::Error { step, message } => format!("[error] {step}: {message}"),
    }
}
