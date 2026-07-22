use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use oxys::{
    ProvisionEvent, SystemInstallEvent, SystemInstallStep, apply_disk_plan,
    apply_system_install_plan,
    detect::DetectedDisk,
    manifest::{Disk, GB, Password, Timezone, Username},
    plan_disk, plan_disk_with_swap, plan_system_install, preflight_with_swap,
    release_target_mounts,
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
            ..
        } if program == "rsync"
    )
}

/// Whether this step streams Portage package completion counts.
fn step_reports_emerge_progress(step: &SystemInstallStep) -> bool {
    matches!(step, SystemInstallStep::EmergePackages { .. })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct RsyncProgress {
    transferred_bytes: u64,
    percent: u16,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct EmergeProgress {
    completed: u32,
    total: u32,
}

/// Parse the suffix added by `install::portage`, e.g.
/// `completed gui-wm/niri (12/133)`.
fn parse_emerge_progress(line: &str) -> Option<EmergeProgress> {
    let suffix = line.strip_prefix("completed ")?.rsplit_once(" (")?.1;
    let counts = suffix.strip_suffix(')')?;
    let (completed, total) = counts.split_once('/')?;
    let completed = completed.parse::<u32>().ok()?;
    let total = total.parse::<u32>().ok()?;
    if total == 0 || completed > total {
        return None;
    }
    Some(EmergeProgress { completed, total })
}

/// Parse the overall completion percent from an rsync `--info=progress2` line.
///
/// Typical form: `1,234,567  45%  12.34MB/s  0:00:12` (optionally with xfr#).
/// Returns `None` for unrelated command output so we never treat an emerge or
/// mkfs percentage as rsync progress.
fn parse_rsync_progress(line: &str) -> Option<RsyncProgress> {
    // progress2 always reports a transfer rate unit ending in B/s (kB/s, MB/s…).
    if !line.contains("B/s") {
        return None;
    }
    let transferred_bytes = line
        .split_whitespace()
        .next()?
        .replace(',', "")
        .parse::<u64>()
        .ok()?;
    let mut best: Option<u16> = None;
    for token in line.split_whitespace() {
        let Some(num) = token.strip_suffix('%') else {
            continue;
        };
        if let Ok(value) = num.parse::<u16>()
            && value <= 100
        {
            best = Some(value);
        }
    }
    best.map(|percent| RsyncProgress {
        transferred_bytes,
        percent,
    })
}

fn format_step_metrics(
    description: &str,
    elapsed: Duration,
    transferred_bytes: Option<u64>,
) -> String {
    let seconds = elapsed.as_secs_f64();
    match transferred_bytes {
        Some(bytes) => {
            let bytes_per_second = if seconds > 0.0 {
                bytes as f64 / seconds
            } else {
                0.0
            };
            format!(
                "[metric] {description}: elapsed={seconds:.3}s transferred={bytes} bytes effective_throughput={}",
                format_rate(bytes_per_second)
            )
        }
        None => format!("[metric] {description}: elapsed={seconds:.3}s"),
    }
}

fn format_rate(bytes_per_second: f64) -> String {
    const KIB: f64 = 1024.0;
    const MIB: f64 = KIB * 1024.0;
    const GIB: f64 = MIB * 1024.0;
    if bytes_per_second >= GIB {
        format!("{:.2} GiB/s", bytes_per_second / GIB)
    } else if bytes_per_second >= MIB {
        format!("{:.2} MiB/s", bytes_per_second / MIB)
    } else if bytes_per_second >= KIB {
        format!("{:.2} KiB/s", bytes_per_second / KIB)
    } else {
        format!("{bytes_per_second:.2} B/s")
    }
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

fn prepare_target_mountpoint(target_mount: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(target_mount)
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
    // The selected target is a UI-owned value. Preserve the compiled profile's
    // filesystem and swap policy instead of replacing the whole Disk object.
    manifest.disk.device = disk.device;
    manifest.disk.layout = disk.layout;
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
    let target_mount = Path::new(TARGET_MOUNT);
    release_target_mounts(target_mount);

    // System planning happens before destructive disk work so live-image and
    // hardware-policy failures cannot wipe the selected disk. The planner also
    // requires its destination to exist, while the disk plan normally creates
    // this mountpoint later as one of its steps. Prepare the empty directory
    // now so a clean live boot can pass pre-wipe system-plan validation.
    if let Err(error) = prepare_target_mountpoint(target_mount) {
        let _ = tx.send(format!(
            "[error] failed to prepare target mountpoint {TARGET_MOUNT}: {error}"
        ));
        return;
    }

    let _ = tx.send("[run  ] preflight disk".to_string());
    let resolved_swap = match manifest.resolved_swap() {
        Ok(swap) => swap,
        Err(error) => {
            let _ = tx.send(format!("[error] {error}"));
            return;
        }
    };
    if let Err(error) = preflight_with_swap(&manifest.disk, &resolved_swap) {
        let _ = tx.send(format!("[error] {error}"));
        return;
    }
    let _ = tx.send("[ok   ] preflight passed".to_string());

    // Build both plans *before* any destructive disk work. System planning
    // resolves session/graphics and validates the live source image; on
    // machines the image cannot support (e.g. proprietary NVIDIA policy on a
    // nouveau-only ISO) we must fail here so the selected disk is never wiped.
    let plan = match plan_disk_with_swap(&manifest.disk, &resolved_swap, target_mount) {
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
        target_mount,
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
    if let Err(error) = preflight_with_swap(&manifest.disk, &resolved_swap) {
        let _ = tx.send(format!("[error] {error}"));
        return;
    }

    let disk_total = plan.steps.len().max(1);
    let mut disk_done = 0usize;
    let mut disk_step_started: Option<Instant> = None;
    send_progress(&tx, 0);
    let mut stream = apply_disk_plan(&plan);
    for event in &mut stream {
        // Nudge the bar when a disk step *starts* so a slow mkfs/wipe doesn't
        // leave the UI frozen at the previous complete mark.
        if matches!(event, ProvisionEvent::StepStart { .. }) {
            let pct = (disk_done as f32 / disk_total as f32 * DISK_BAND as f32) as u16;
            send_progress(&tx, pct.min(DISK_BAND.saturating_sub(1)));
        }
        let metric = match &event {
            ProvisionEvent::StepStart { .. } => {
                disk_step_started = Some(Instant::now());
                None
            }
            ProvisionEvent::StepComplete { description }
            | ProvisionEvent::Error {
                step: description, ..
            } => disk_step_started
                .take()
                .map(|started| format_step_metrics(description, started.elapsed(), None)),
            ProvisionEvent::StepOutput { .. } => None,
        };
        let completed = matches!(event, ProvisionEvent::StepComplete { .. });
        let _ = tx.send(format_provision_event(event));
        if let Some(metric) = metric {
            let _ = tx.send(metric);
        }
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
    let weights: Vec<u32> = system_plan.steps.iter().map(system_step_weight).collect();
    let total_weight: u32 = weights.iter().sum::<u32>().max(1);
    let mut weight_done: u32 = 0;
    let mut step_idx: usize = 0;
    let mut current_weight: u32 = 0;
    let mut current_frac: f32 = 0.0;
    let mut tracks_rsync = false;
    let mut tracks_emerge = false;
    let mut step_started: Option<Instant> = None;
    let mut transferred_bytes: Option<u64> = None;
    // Monotonic ceiling so a noisy/out-of-order refresh never rewinds the bar.
    let mut last_pct = DISK_BAND;

    let emit_system_progress = |tx: &UnboundedSender<String>,
                                weight_done: u32,
                                current_weight: u32,
                                current_frac: f32,
                                last_pct: &mut u16| {
        let pct = system_progress_percent(weight_done, current_weight, current_frac, total_weight);
        let pct = pct.max(*last_pct);
        *last_pct = pct;
        send_progress(tx, pct);
    };

    let mut stream = apply_system_install_plan(&system_plan);
    for event in &mut stream {
        let mut metric = None;
        match &event {
            SystemInstallEvent::StepStart { .. } => {
                step_started = Some(Instant::now());
                transferred_bytes = None;
                current_weight = weights.get(step_idx).copied().unwrap_or(1);
                current_frac = 0.0;
                tracks_rsync = system_plan
                    .steps
                    .get(step_idx)
                    .is_some_and(step_reports_rsync_progress);
                tracks_emerge = system_plan
                    .steps
                    .get(step_idx)
                    .is_some_and(step_reports_emerge_progress);
                emit_system_progress(
                    &tx,
                    weight_done,
                    current_weight,
                    current_frac,
                    &mut last_pct,
                );
            }
            SystemInstallEvent::StepOutput { line } => {
                if tracks_rsync && let Some(progress) = parse_rsync_progress(line) {
                    transferred_bytes = Some(
                        transferred_bytes
                            .unwrap_or_default()
                            .max(progress.transferred_bytes),
                    );
                    let frac = (progress.percent as f32 / 100.0).clamp(0.0, 1.0);
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
                if tracks_emerge && let Some(progress) = parse_emerge_progress(line) {
                    let frac = (progress.completed as f32 / progress.total as f32).clamp(0.0, 1.0);
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
            SystemInstallEvent::StepComplete { description } => {
                metric = step_started.take().map(|started| {
                    format_step_metrics(description, started.elapsed(), transferred_bytes)
                });
                weight_done = weight_done.saturating_add(current_weight);
                current_weight = 0;
                current_frac = 0.0;
                tracks_rsync = false;
                tracks_emerge = false;
                transferred_bytes = None;
                step_idx = step_idx.saturating_add(1);
                emit_system_progress(
                    &tx,
                    weight_done,
                    current_weight,
                    current_frac,
                    &mut last_pct,
                );
            }
            SystemInstallEvent::Error { step, .. } => {
                metric = step_started
                    .take()
                    .map(|started| format_step_metrics(step, started.elapsed(), transferred_bytes));
                transferred_bytes = None;
            }
        }
        let _ = tx.send(format_system_install_event(event));
        if let Some(metric) = metric {
            let _ = tx.send(metric);
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prepare_target_mountpoint_creates_missing_directory() {
        let temp = std::env::temp_dir().join(format!(
            "oxys-installer-target-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let source = temp.join("source");
        let target = temp.join("target");
        std::fs::create_dir_all(source.join("boot")).unwrap();

        assert!(!target.exists());
        prepare_target_mountpoint(&target).unwrap();
        assert!(target.is_dir());
        prepare_target_mountpoint(&target).unwrap();
        plan_system_install(
            &oxys::manifest::SystemManifest::default(),
            &source,
            &target,
            None,
        )
        .expect("prepared target mountpoint should pass system-plan validation");

        std::fs::remove_dir_all(temp).unwrap();
    }

    #[test]
    fn parse_rsync_progress_reads_bytes_and_percent() {
        assert_eq!(
            parse_rsync_progress("  1,234,567  45%  12.34MB/s  0:00:12"),
            Some(RsyncProgress {
                transferred_bytes: 1_234_567,
                percent: 45,
            })
        );
        assert_eq!(
            parse_rsync_progress("987654321  100%  200.00MB/s  0:00:05 (xfr#12, to-chk=0/99)"),
            Some(RsyncProgress {
                transferred_bytes: 987_654_321,
                percent: 100,
            })
        );
        assert_eq!(
            parse_rsync_progress("        0   0%    0.00kB/s    0:00:00"),
            Some(RsyncProgress {
                transferred_bytes: 0,
                percent: 0,
            })
        );
    }

    #[test]
    fn parse_rsync_progress_ignores_unrelated_output() {
        assert_eq!(parse_rsync_progress("emerging foo-1.2.3"), None);
        assert_eq!(parse_rsync_progress("using 50% of free space"), None);
        assert_eq!(parse_rsync_progress("speed 12.34MB/s"), None);
    }

    #[test]
    fn parse_emerge_progress_reads_completed_package_counts() {
        assert_eq!(
            parse_emerge_progress("completed gui-wm/niri (12/133)"),
            Some(EmergeProgress {
                completed: 12,
                total: 133,
            })
        );
        assert_eq!(parse_emerge_progress("emerging gui-wm/niri"), None);
        assert_eq!(
            parse_emerge_progress("completed gui-wm/niri (134/133)"),
            None
        );
    }

    #[test]
    fn step_metrics_include_effective_transfer_rate() {
        let rendered = format_step_metrics(
            "Copy live system into target",
            Duration::from_secs(2),
            Some(2 * 1024 * 1024),
        );
        assert_eq!(
            rendered,
            "[metric] Copy live system into target: elapsed=2.000s transferred=2097152 bytes effective_throughput=1.00 MiB/s"
        );
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
