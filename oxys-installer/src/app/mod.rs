use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::time::{Instant, SystemTime};

use oxys::{
    detect::DetectedDisk,
    manifest::{Disk, DiskLayout},
};
use tokio::{sync::mpsc::UnboundedReceiver, task::JoinHandle};

use crate::{hardware::HardwareDetectEvent, provisioning};

mod identity;
mod input;
mod polling;
mod tasks;

/// Where the installer tees its full provisioning log. Kept on disk so a failed
/// run is recoverable once the full-screen TUI has exited -- the on-screen log
/// scrolls and vanishes on quit, but this file survives until reboot.
pub(crate) const INSTALL_LOG_PATH: &str = "/var/log/oxys-install.log";

/// Append one provisioning log line to [`INSTALL_LOG_PATH`]. Best-effort: any
/// I/O error is swallowed so logging can never derail or slow the install.
pub(super) fn append_install_log(line: &str) {
    use std::io::Write;
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(INSTALL_LOG_PATH)
    {
        let _ = writeln!(file, "{line}");
    }
}

/// Whether a log line is one of rsync's in-place progress refreshes (from
/// `--info=progress2`), e.g. `[out  ] 1,234,567  45%  12.34MB/s  0:00:12`.
/// Consecutive ones are coalesced into a single updating line in the UI.
pub(super) fn is_rsync_progress(line: &str) -> bool {
    line.starts_with("[out  ]") && line.contains('%') && line.contains("B/s")
}

/// Whether `name` is safe to feed straight into `useradd`: a POSIX-ish login
/// name -- lowercase letters, digits, `-`/`_`, starting with a letter or `_`,
/// and no longer than the usual 32-character `utmp` limit.
pub(super) fn is_valid_login_name(name: &str) -> bool {
    let mut chars = name.chars();
    match chars.next() {
        Some(c) if c.is_ascii_lowercase() || c == '_' => {}
        _ => return false,
    }
    name.len() <= 32
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-')
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Screen {
    Welcome,
    HardwareDetection,
    DiskSelect,
    #[allow(dead_code)]
    Partition,
    ConfigSelect,
    /// Prompts for a local file path or a URL when the "custom" profile is
    /// selected; submitting kicks off a fetch (URL) or validates a path,
    /// then gates into `ConfigValidate` same as the baked-in profiles.
    CustomSource,
    /// Compiling the selected config (spinner); gates entry to Confirm.
    ConfigValidate,
    /// The selected config failed to compile; shows the compiler output.
    ConfigError,
    /// Read-only review of the compiled manifest's packages, grouped by whether
    /// they arrive as prebuilt binaries or build from source. Sits between a
    /// successful compile and the final confirm gate.
    PackageSummary,
    Confirm,
    /// Interactive username entry for users declaring `Username::Prompt`.
    /// Reached from Confirm only when the compiled manifest has such users,
    /// and always resolved before `Passwords` (the password screen displays
    /// and keys off the resolved name).
    Usernames,
    /// Interactive password entry for users declaring `Password::Prompt`.
    /// Reached from Confirm (via `Usernames` when applicable) only when the
    /// compiled manifest has such users.
    Passwords,
    Installing,
    Done,
}

impl Screen {
    pub(crate) fn index(self) -> usize {
        match self {
            Screen::Welcome => 0,
            Screen::HardwareDetection => 1,
            Screen::DiskSelect => 2,
            Screen::Partition => 3, // hidden for now
            // CustomSource/Validate/Error are sub-states of the config step.
            Screen::ConfigSelect
            | Screen::CustomSource
            | Screen::ConfigValidate
            | Screen::ConfigError => 3,
            // Usernames/Passwords are sub-states between confirm and install;
            // they share the confirm step marker on the rail. PackageSummary
            // is the review beat leading into confirm, so it shares that
            // marker too.
            Screen::PackageSummary | Screen::Confirm | Screen::Usernames | Screen::Passwords => 4,
            Screen::Installing => 5,
            Screen::Done => 6,
        }
    }
}

/// Result of an asynchronous config compilation.
pub(crate) enum CompileEvent {
    Done(Result<std::path::PathBuf, oxys::compile::CompileError>),
}

pub(crate) struct App {
    pub(crate) current: Screen,
    pub(crate) disk_idx: usize,
    pub(crate) target_cursor: usize,
    pub(crate) disks: Vec<DetectedDisk>,
    pub(crate) config_idx: usize,
    /// Explicit path to a config file when the "custom" profile's source is
    /// a user-provided path or a URL fetched into `configs/custom.fe2o3`.
    /// `None` falls back to the baked-in `configs/custom.fe2o3` template.
    custom_config_path: Option<String>,
    pub(crate) custom_source_input: String,
    pub(crate) custom_source_error: Option<String>,
    pub(crate) custom_fetching: bool,
    custom_fetch_rx: Option<UnboundedReceiver<Result<(), String>>>,
    custom_fetch_task: Option<JoinHandle<()>>,
    pub(crate) partition_lines: Vec<String>,
    pub(crate) install_lines: Vec<String>,
    partition_rx: Option<UnboundedReceiver<String>>,
    install_rx: Option<UnboundedReceiver<String>>,
    #[allow(dead_code)]
    partition_task: Option<JoinHandle<()>>,
    install_task: Option<JoinHandle<()>>,
    pub(crate) install_progress: u16,
    pub(crate) last_tick: Instant,
    tick_count: u64,
    pub(crate) hardware_action_idx: usize,
    pub(crate) hardware_rows: Vec<(String, String)>,
    pub(crate) hardware_detecting: bool,
    pub(crate) hardware_detect_done: bool,
    hardware_rx: Option<UnboundedReceiver<HardwareDetectEvent>>,
    hardware_task: Option<JoinHandle<()>>,
    pub(crate) hardware_spinner_idx: usize,
    pub(crate) hardware_short: String,
    pub(crate) hardware_full: String,
    /// `None` while the startup connectivity probe (see [`network`]) is still
    /// in flight; `Some` once it has reported online/offline.
    pub(crate) network_online: Option<bool>,
    pub(crate) network_spinner_idx: usize,
    network_rx: Option<UnboundedReceiver<bool>>,
    #[allow(dead_code)]
    network_task: Option<JoinHandle<()>>,
    pub(crate) confirm_quit: bool,
    /// Set when the user chooses to reboot from the Done screen; `main` reboots
    /// the machine after the TUI tears down instead of dropping to the shell.
    pub(crate) reboot_requested: bool,
    pending_edit: Option<String>,
    initial_config_mtimes: HashMap<String, Option<SystemTime>>,
    // --- config compilation / validation gate ---
    pub(crate) compiling: bool,
    pub(crate) compile_error: Option<oxys::compile::CompileError>,
    pub(crate) compile_scroll: usize,
    pub(crate) confirm_view_manifest: bool,
    pub(crate) manifest_scroll: usize,
    pub(crate) manifest_text: Option<String>,
    pub(crate) manifest_read_error: Option<String>,
    compiled_manifest: Option<PathBuf>,
    compile_rx: Option<UnboundedReceiver<CompileEvent>>,
    compile_task: Option<JoinHandle<()>>,
    // --- package source summary (binary vs. from-source), shown post-compile ---
    pub(crate) package_summary: Option<oxys::PackageSummary>,
    pub(crate) package_scroll: usize,
    // --- interactive username collection (Username::Prompt users), resolved
    // before password collection since the password screen displays and keys
    // off the resolved name ---
    pub(crate) prompt_username_indices: Vec<usize>,
    pub(crate) username_idx: usize,
    pub(crate) username_input: String,
    pub(crate) username_error: Option<String>,
    collected_usernames: HashMap<usize, String>,
    // --- interactive password collection (Password::Prompt users) ---
    pub(crate) prompt_users: Vec<String>,
    pub(crate) password_idx: usize,
    pub(crate) password_input: String,
    pub(crate) password_confirm_input: String,
    pub(crate) password_confirming: bool,
    pub(crate) password_error: Option<String>,
    collected_passwords: HashMap<String, String>,
}

impl App {
    pub(crate) fn new() -> Self {
        let mut app = Self {
            current: Screen::Welcome,
            disk_idx: 0,
            target_cursor: 0,
            disks: oxys::detect::detect_disks(),
            config_idx: 0,
            custom_config_path: None,
            custom_source_input: String::new(),
            custom_source_error: None,
            custom_fetching: false,
            custom_fetch_rx: None,
            custom_fetch_task: None,
            partition_lines: Vec::new(),
            install_lines: Vec::new(),
            partition_rx: None,
            install_rx: None,
            partition_task: None,
            install_task: None,
            install_progress: 0,
            last_tick: Instant::now(),
            tick_count: 0,
            hardware_action_idx: 0,
            hardware_rows: Vec::new(),
            hardware_detecting: false,
            hardware_detect_done: false,
            hardware_rx: None,
            hardware_task: None,
            hardware_spinner_idx: 0,
            hardware_short: "hardware not detected".to_string(),
            hardware_full: "not detected".to_string(),
            network_online: None,
            network_spinner_idx: 0,
            network_rx: None,
            network_task: None,
            confirm_quit: false,
            reboot_requested: false,
            pending_edit: None,
            compiling: false,
            compile_error: None,
            compile_scroll: 0,
            confirm_view_manifest: false,
            manifest_scroll: 0,
            manifest_text: None,
            manifest_read_error: None,
            compiled_manifest: None,
            compile_rx: None,
            compile_task: None,
            package_summary: None,
            package_scroll: 0,
            prompt_username_indices: Vec::new(),
            username_idx: 0,
            username_input: String::new(),
            username_error: None,
            collected_usernames: HashMap::new(),
            prompt_users: Vec::new(),
            password_idx: 0,
            password_input: String::new(),
            password_confirm_input: String::new(),
            password_confirming: false,
            password_error: None,
            collected_passwords: HashMap::new(),
            initial_config_mtimes: {
                let mut m = HashMap::new();
                for name in ["desktop.fe2o3", "base.fe2o3", "custom"] {
                    let filename = if name == "custom" {
                        "custom.fe2o3"
                    } else {
                        name
                    };
                    let path = format!("configs/{}", filename);
                    let mtime = fs::metadata(&path).ok().and_then(|m| m.modified().ok());
                    m.insert(name.to_string(), mtime);
                }
                m
            },
        };
        app.start_network_check();
        app
    }

    pub(crate) fn selected_disk(&self) -> String {
        self.disks
            .get(self.disk_idx)
            .map(provisioning::format_disk)
            .unwrap_or_else(|| "no installable disk detected".to_string())
    }

    pub(crate) fn selected_disk_device(&self) -> Option<String> {
        self.disks
            .get(self.disk_idx)
            .map(|disk| disk.device.clone())
    }

    pub(crate) fn selected_layout(&self) -> DiskLayout {
        DiskLayout::Ext4
    }

    pub(crate) fn selected_layout_label(&self) -> &'static str {
        "ext4"
    }

    pub(crate) fn selected_disk_config(&self) -> Option<Disk> {
        Some(Disk {
            device: self.selected_disk_device()?,
            layout: self.selected_layout(),
            ..Disk::default()
        })
    }

    pub(crate) fn selected_config(&self) -> &'static str {
        ["desktop", "base", "custom"][self.config_idx]
    }

    pub(crate) fn step_labels() -> [&'static str; 7] {
        [
            "welcome", "hardware", "disk", // "partition",  // hidden for now
            "config", "confirm", "install", "done",
        ]
    }
}
