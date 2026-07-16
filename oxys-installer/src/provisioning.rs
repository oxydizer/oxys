use std::collections::HashMap;
use std::path::PathBuf;

use oxys::{
    apply_disk_plan, apply_system_install_plan,
    detect::DetectedDisk,
    manifest::{Disk, Password, Timezone, Username, GB},
    plan_disk, plan_system_install, preflight, release_target_mounts, ProvisionEvent,
    SystemInstallEvent, SystemInstallStep,
};
use tokio::sync::mpsc::UnboundedSender;

pub(crate) const TARGET_MOUNT: &str = "/mnt/oxys";

/// Control-line prefix the installer parses to drive the progress bar. Lines
/// that start with this are consumed by the UI and never shown in the log.
pub(crate) const PROGRESS_PREFIX: &str = "[[progress]] ";

/// Fraction of the bar (0..=100) allotted to the fast disk-provisioning phase.
/// The rest belongs to the long system-install phase (rsync, bootloader, …).
const DISK_BAND: u16 = 12;

fn send_progress(tx: &UnboundedSender<String>, percent: u16) {
    // Never report a full bar from here: 100% is reserved for the moment the
    // worker actually finishes and the channel closes, so the install screen
    // can't claim "complete" while steps are still running.
    let _ = tx.send(format!("{PROGRESS_PREFIX}{}", percent.min(99)));
}

/// Relative weight of a system-install step for the progress bar.
///
/// Equal step counts are a poor fit: "Copy live system into target" can run
/// for most of the install while dozens of one-second bookkeeping steps each
/// claim the same slice. Weights keep the bar roughly time-correlated.
fn system_step_weight(step: &SystemInstallStep) -> u32 {
    match step {
        SystemInstallStep::Command {
            program,
            description,
            ..
        } if program == "rsync" && description.contains("live system") => 55,
        SystemInstallStep::Command { program, .. } if program == "rsync" => 4,
        SystemInstallStep::EmergePackages { .. } => 18,
        SystemInstallStep::GenerateInitramfs { .. } => 8,
        SystemInstallStep::SetupUsers { .. } => 3,
        SystemInstallStep::SetupLogin { .. } => 2,
        SystemInstallStep::ConfigureGraphicsRuntime { .. } => 2,
        SystemInstallStep::SeedOxysConfig { .. } => 2,
        SystemInstallStep::Finalize { .. } => 2,
        _ => 1,
    }
}

/// Whether this step streams rsync `--info=progress2` output that can drive
/// the bar *within* the step (not just on complete).
fn step_reports_rsync_progress(step: &SystemInstallStep) -> bool {
    matches!(
        step,
        SystemInstallStep::Command {
            program,
            description,
            ..
        } if program == "rsync" && description.contains("live system")
    )
}

/// Parse the overall completion percent from an rsync `--info=progress2` line.
///
/// Typical form: `1,234,567  45%  12.34MB/s  0:00:12` (optionally with xfr#).
/// Returns `None` for unrelated command output so we never treat an emerge or
/// mkfs percentage as rsync progress.
fn parse_rsync_percent(line: &str) -> Option<u16> {
    // progress2 always reports a transfer rate unit ending in B/s (kB/s, MB/s…).
    if !line.contains("B/s") {
        return None;
    }
    let mut best: Option<u16> = None;
    for token in line.split_whitespace() {
        let Some(num) = token.strip_suffix('%') else {
            continue;
        };
        if let Ok(value) = num.parse::<u16>() {
            if value <= 100 {
                best = Some(value);
            }
        }
    }
    best
}

/// Map completed weight + in-step fraction into the system-install band
/// (`DISK_BAND` ..= 99).
fn system_progress_percent(
    weight_done: u32,
    current_weight: u32,
    current_frac: f32,
    total_weight: u32,
) -> u16 {
    let system_span = 99u16.saturating_sub(DISK_BAND);
    let total = total_weight.max(1) as f32;
    let units = weight_done as f32 + current_weight as f32 * current_frac.clamp(0.0, 1.0);
    let within = ((units / total) * system_span as f32).floor() as u16;
    (DISK_BAND + within.min(system_span)).min(99)
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
    config_source: Option<PathBuf>,
    prompted_timezone: Option<String>,
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
    // Resolve the interactively-picked timezone. Planning refuses an
    // unresolved Timezone::Prompt, so when nothing was collected (picker
    // skipped on a broken live image) fall back to leaving the target on the
    // live root's zone.
    if manifest.prompts_timezone() {
        match prompted_timezone {
            Some(zone) => manifest.os.timezone = Timezone::Literal(zone),
            None => {
                let _ = tx.send(
                    "[warn ] no timezone collected; keeping the live image default".to_string(),
                );
                manifest.os.timezone = Timezone::Literal(String::new());
            }
        }
    }
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

    let _ = tx.send("[run  ] preflight disk".to_string());
    if let Err(error) = preflight(&manifest.disk) {
        let _ = tx.send(format!("[error] {error}"));
        return;
    }
    let _ = tx.send("[ok   ] preflight passed".to_string());

    // Build both plans *before* any destructive disk work. System planning
    // resolves session/graphics and validates the live source image; on
    // machines the image cannot support (e.g. proprietary NVIDIA policy on a
    // nouveau-only ISO) we must fail here so the selected disk is never wiped.
    let plan = match plan_disk(&manifest.disk, std::path::Path::new(TARGET_MOUNT)) {
        Ok(plan) => plan,
        Err(error) => {
            let _ = tx.send(format!("[error] {error}"));
            return;
        }
    };
    let _ = tx.send("[run  ] validate system install plan".to_string());
    let system_plan = match plan_system_install(
        &manifest,
        std::path::Path::new("/"),
        std::path::Path::new(TARGET_MOUNT),
        config_source.as_deref(),
    ) {
        Ok(plan) => {
            let _ = tx.send(format!(
                "[ok   ] system install plan validated ({} steps)",
                plan.steps.len()
            ));
            plan
        }
        Err(error) => {
            let _ = tx.send(format!("[error] {error}"));
            return;
        }
    };

    // Re-check immediately before wipe — covers races after Confirm and any
    // mount activity during the (usually short) planning phase.
    let _ = tx.send("[run  ] re-check disk preflight before wipe".to_string());
    if let Err(error) = preflight(&manifest.disk) {
        let _ = tx.send(format!("[error] {error}"));
        return;
    }

    let disk_total = plan.steps.len().max(1);
    let mut disk_done = 0usize;
    send_progress(&tx, 0);
    let mut stream = apply_disk_plan(&plan);
    for event in &mut stream {
        // Nudge the bar when a disk step *starts* so a slow mkfs/wipe doesn't
        // leave the UI frozen at the previous complete mark.
        if matches!(event, ProvisionEvent::StepStart { .. }) {
            let pct = (disk_done as f32 / disk_total as f32 * DISK_BAND as f32) as u16;
            send_progress(&tx, pct.min(DISK_BAND.saturating_sub(1)));
        }
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
    send_progress(&tx, DISK_BAND);

    // Pre-weight every planned step so the bar can grow smoothly *during* the
    // long rsync (via progress2 %) rather than jumping only on StepComplete.
    let weights: Vec<u32> = system_plan
        .steps
        .iter()
        .map(system_step_weight)
        .collect();
    let total_weight: u32 = weights.iter().sum::<u32>().max(1);
    let mut weight_done: u32 = 0;
    let mut step_idx: usize = 0;
    let mut current_weight: u32 = 0;
    let mut current_frac: f32 = 0.0;
    let mut tracks_rsync = false;
    // Monotonic ceiling so a noisy/out-of-order refresh never rewinds the bar.
    let mut last_pct = DISK_BAND;

    let emit_system_progress = |tx: &UnboundedSender<String>,
                                weight_done: u32,
                                current_weight: u32,
                                current_frac: f32,
                                last_pct: &mut u16| {
        let pct =
            system_progress_percent(weight_done, current_weight, current_frac, total_weight);
        let pct = pct.max(*last_pct);
        *last_pct = pct;
        send_progress(tx, pct);
    };

    let mut stream = apply_system_install_plan(&system_plan);
    for event in &mut stream {
        match &event {
            SystemInstallEvent::StepStart { .. } => {
                current_weight = weights.get(step_idx).copied().unwrap_or(1);
                current_frac = 0.0;
                tracks_rsync = system_plan
                    .steps
                    .get(step_idx)
                    .is_some_and(step_reports_rsync_progress);
                emit_system_progress(
                    &tx,
                    weight_done,
                    current_weight,
                    current_frac,
                    &mut last_pct,
                );
            }
            SystemInstallEvent::StepOutput { line } => {
                if tracks_rsync {
                    if let Some(rsync_pct) = parse_rsync_percent(line) {
                        let frac = (rsync_pct as f32 / 100.0).clamp(0.0, 1.0);
                        // Only push when the transfer actually advanced; progress2
                        // redraws many times at the same percent.
                        if frac > current_frac + 0.001 {
                            current_frac = frac;
                            emit_system_progress(
                                &tx,
                                weight_done,
                                current_weight,
                                current_frac,
                                &mut last_pct,
                            );
                        }
                    }
                }
            }
            SystemInstallEvent::StepComplete { .. } => {
                weight_done = weight_done.saturating_add(current_weight);
                current_weight = 0;
                current_frac = 0.0;
                tracks_rsync = false;
                step_idx = step_idx.saturating_add(1);
                emit_system_progress(
                    &tx,
                    weight_done,
                    current_weight,
                    current_frac,
                    &mut last_pct,
                );
            }
            SystemInstallEvent::Error { .. } => {}
        }
        let _ = tx.send(format_system_install_event(event));
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_rsync_percent_reads_progress2_line() {
        assert_eq!(
            parse_rsync_percent("  1,234,567  45%  12.34MB/s  0:00:12"),
            Some(45)
        );
        assert_eq!(
            parse_rsync_percent("987654321  100%  200.00MB/s  0:00:05 (xfr#12, to-chk=0/99)"),
            Some(100)
        );
        assert_eq!(
            parse_rsync_percent("        0   0%    0.00kB/s    0:00:00"),
            Some(0)
        );
    }

    #[test]
    fn parse_rsync_percent_ignores_unrelated_output() {
        assert_eq!(parse_rsync_percent("emerging foo-1.2.3"), None);
        assert_eq!(parse_rsync_percent("using 50% of free space"), None);
        assert_eq!(parse_rsync_percent("speed 12.34MB/s"), None);
    }

    #[test]
    fn system_progress_grows_with_rsync_fraction() {
        // Main copy is most of the weight; half-done rsync should land well
        // above the disk band and well below the finish.
        let total = 55 + 20; // rsync + assorted bookkeeping
        let mid = system_progress_percent(0, 55, 0.5, total);
        let start = system_progress_percent(0, 55, 0.0, total);
        let almost = system_progress_percent(0, 55, 0.99, total);
        let done = system_progress_percent(55, 0, 0.0, total);

        assert!(mid > DISK_BAND);
        assert!(mid > start);
        assert!(almost > mid);
        assert!(done >= almost);
        assert!(done < 99);
    }

    #[test]
    fn system_progress_never_exceeds_99() {
        assert_eq!(system_progress_percent(100, 0, 0.0, 100), 99);
        assert_eq!(system_progress_percent(99, 1, 1.0, 100), 99);
    }
}
