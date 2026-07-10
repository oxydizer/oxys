use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::time::{Instant, SystemTime};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use oxys::{
    detect::{detect_disks, DetectedDisk},
    manifest::{Disk, DiskLayout, Password, User, Username},
};
use tokio::{
    sync::mpsc::{self, error::TryRecvError, UnboundedReceiver},
    task::JoinHandle,
};

use crate::{
    hardware::{self, HardwareDetectEvent},
    provisioning,
    ui::theme::SPINNER,
};

/// Where the installer tees its full provisioning log. Kept on disk so a failed
/// run is recoverable once the full-screen TUI has exited -- the on-screen log
/// scrolls and vanishes on quit, but this file survives until reboot.
pub(crate) const INSTALL_LOG_PATH: &str = "/var/log/oxys-install.log";

/// Append one provisioning log line to [`INSTALL_LOG_PATH`]. Best-effort: any
/// I/O error is swallowed so logging can never derail or slow the install.
fn append_install_log(line: &str) {
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
fn is_rsync_progress(line: &str) -> bool {
    line.starts_with("[out  ]") && line.contains('%') && line.contains("B/s")
}

/// Whether `name` is safe to feed straight into `useradd`: a POSIX-ish login
/// name -- lowercase letters, digits, `-`/`_`, starting with a letter or `_`,
/// and no longer than the usual 32-character `utmp` limit.
fn is_valid_login_name(name: &str) -> bool {
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
            // Validate/Error are sub-states of the config step.
            Screen::ConfigSelect | Screen::ConfigValidate | Screen::ConfigError => 3,
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
        Self {
            current: Screen::Welcome,
            disk_idx: 0,
            target_cursor: 0,
            disks: detect_disks(),
            config_idx: 0,
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
                for name in ["base.rs", "desktop.rs", "custom"] {
                    let filename = if name == "custom" { "custom.rs" } else { name };
                    let path = format!("configs/{}", filename);
                    let mtime = fs::metadata(&path).ok().and_then(|m| m.modified().ok());
                    m.insert(name.to_string(), mtime);
                }
                m
            },
        }
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
        ["base.rs", "desktop.rs", "custom"][self.config_idx]
    }

    pub(crate) fn step_labels() -> [&'static str; 7] {
        [
            "welcome", "hardware", "disk", // "partition",  // hidden for now
            "config", "confirm", "install", "done",
        ]
    }

    fn start_hardware_detect(&mut self) {
        self.hardware_rows.clear();
        self.hardware_detect_done = false;
        self.hardware_detecting = true;
        self.hardware_rx = None;

        if let Some(handle) = self.hardware_task.take() {
            handle.abort();
        }

        let (tx, rx) = mpsc::unbounded_channel();
        self.hardware_rx = Some(rx);

        self.hardware_task = Some(tokio::spawn(async move {
            hardware::stream_hardware(tx).await;
        }));
    }

    #[allow(dead_code)]
    fn start_partition_plan(&mut self) {
        self.partition_lines.clear();
        self.partition_rx = None;
        if let Some(handle) = self.partition_task.take() {
            handle.abort();
        }

        let (tx, rx) = mpsc::unbounded_channel();
        self.partition_rx = Some(rx);
        let disk = self.selected_disk_config();

        self.partition_task = Some(tokio::task::spawn_blocking(move || {
            provisioning::run_partition_plan(disk, tx);
        }));
    }

    fn start_install(&mut self) {
        self.install_lines.clear();
        // Start a fresh on-disk log for this run so a failure is inspectable
        // after the TUI exits (`cat /var/log/oxys-install.log`, or read it over
        // the QEMU serial console). Best-effort: never block the install on it.
        let _ = fs::write(INSTALL_LOG_PATH, b"");
        self.install_progress = 0;
        self.install_rx = None;
        if let Some(handle) = self.install_task.take() {
            handle.abort();
        }

        let (tx, rx) = mpsc::unbounded_channel();
        self.install_rx = Some(rx);
        let disk = self.selected_disk_config();
        let manifest = self.compiled_manifest.clone();
        // Move the collected secrets/names into the worker so they don't
        // linger in App state, and clear the entry buffers.
        let usernames = std::mem::take(&mut self.collected_usernames);
        self.username_input.clear();
        let passwords = std::mem::take(&mut self.collected_passwords);
        self.password_input.clear();
        self.password_confirm_input.clear();

        self.install_task = Some(tokio::task::spawn_blocking(move || {
            provisioning::run_install(disk, manifest, usernames, passwords, tx);
        }));
    }

    /// Load the compiled manifest to see which users need an interactive
    /// name. Returns the next screen: `Usernames` when there is at least one
    /// such user, otherwise falls through to password collection.
    fn begin_identity_collection(&mut self) -> Screen {
        if let Some(error) = provisioning::install_permission_error() {
            self.install_lines = vec![format!("[error] {error}")];
            self.install_progress = 100;
            self.install_rx = None;
            if let Some(handle) = self.install_task.take() {
                handle.abort();
            }
            return Screen::Installing;
        }

        let indices: Vec<usize> = self
            .compiled_manifest
            .as_ref()
            .and_then(|path| oxys::compile::load_manifest(path).ok())
            .map(|manifest| manifest.prompt_usernames())
            .unwrap_or_default();

        if indices.is_empty() {
            return self.begin_password_collection();
        }

        self.prompt_username_indices = indices;
        self.username_idx = 0;
        self.username_input.clear();
        self.username_error = None;
        self.collected_usernames.clear();
        Screen::Usernames
    }

    /// Index of the user currently being prompted for a name, if any.
    pub(crate) fn current_prompt_username_index(&self) -> Option<usize> {
        self.prompt_username_indices.get(self.username_idx).copied()
    }

    /// This user's name: the literal baked into the config, or -- for a
    /// `Username::Prompt` user -- whatever has been collected for it so far
    /// on the `Usernames` screen.
    fn resolved_name(&self, index: usize, user: &User) -> String {
        match &user.name {
            Username::Literal(name) => name.clone(),
            Username::Prompt => self
                .collected_usernames
                .get(&index)
                .cloned()
                .unwrap_or_default(),
        }
    }

    /// Handle a keystroke on the username entry screen. Owns all keys so typed
    /// characters (including `q`) never trigger global shortcuts.
    fn username_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Esc => {
                // Abandon collection and return to the confirm screen.
                self.prompt_username_indices.clear();
                self.collected_usernames.clear();
                self.username_input.clear();
                self.username_error = None;
                self.current = Screen::Confirm;
            }
            KeyCode::Backspace => {
                self.username_input.pop();
            }
            KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.username_input.push(c);
            }
            KeyCode::Enter => self.username_submit(),
            _ => {}
        }
    }

    fn username_submit(&mut self) {
        let name = self.username_input.trim().to_owned();
        if !is_valid_login_name(&name) {
            self.username_error = Some(
                "enter a valid login name: lowercase letters, digits, - or _, starting with a letter or _"
                    .to_owned(),
            );
            return;
        }

        if let Some(index) = self.current_prompt_username_index() {
            self.collected_usernames.insert(index, name);
        }
        self.username_input.clear();
        self.username_error = None;
        self.username_idx += 1;

        if self.username_idx >= self.prompt_username_indices.len() {
            // All names collected; move on to any password prompts.
            self.current = self.begin_password_collection();
        }
    }

    /// Load the compiled manifest to see which users need an interactive
    /// password. Returns the next screen: `Passwords` when there is at least one
    /// such user, otherwise it kicks off the install and returns `Installing`.
    fn begin_password_collection(&mut self) -> Screen {
        let names: Vec<String> = self
            .compiled_manifest
            .as_ref()
            .and_then(|path| oxys::compile::load_manifest(path).ok())
            .map(|manifest| {
                manifest
                    .users
                    .iter()
                    .enumerate()
                    .filter(|(_, user)| user.password == Password::Prompt)
                    .map(|(index, user)| self.resolved_name(index, user))
                    .collect()
            })
            .unwrap_or_default();

        if names.is_empty() {
            self.start_install();
            return Screen::Installing;
        }

        self.prompt_users = names;
        self.password_idx = 0;
        self.password_input.clear();
        self.password_confirm_input.clear();
        self.password_confirming = false;
        self.password_error = None;
        self.collected_passwords.clear();
        Screen::Passwords
    }

    /// Name of the user currently being prompted, if any.
    pub(crate) fn current_prompt_user(&self) -> Option<&str> {
        self.prompt_users.get(self.password_idx).map(String::as_str)
    }

    /// Handle a keystroke on the password entry screen. Owns all keys so typed
    /// characters (including `q`) never trigger global shortcuts.
    fn password_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Esc => {
                // Abandon collection and return to the confirm screen.
                self.prompt_users.clear();
                self.collected_passwords.clear();
                self.password_input.clear();
                self.password_confirm_input.clear();
                self.password_confirming = false;
                self.password_error = None;
                self.current = Screen::Confirm;
            }
            KeyCode::Backspace => {
                if self.password_confirming {
                    self.password_confirm_input.pop();
                } else {
                    self.password_input.pop();
                }
            }
            KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                if self.password_confirming {
                    self.password_confirm_input.push(c);
                } else {
                    self.password_input.push(c);
                }
            }
            KeyCode::Enter => self.password_submit(),
            _ => {}
        }
    }

    fn password_submit(&mut self) {
        if !self.password_confirming {
            if self.password_input.is_empty() {
                self.password_error = Some("password cannot be empty".to_owned());
                return;
            }
            self.password_confirming = true;
            self.password_error = None;
            return;
        }

        if self.password_confirm_input != self.password_input {
            // Mismatch: wipe both entries and restart this user.
            self.password_input.clear();
            self.password_confirm_input.clear();
            self.password_confirming = false;
            self.password_error = Some("passwords did not match, try again".to_owned());
            return;
        }

        if let Some(name) = self.current_prompt_user().map(str::to_owned) {
            self.collected_passwords
                .insert(name, std::mem::take(&mut self.password_input));
        }
        self.password_confirm_input.clear();
        self.password_confirming = false;
        self.password_error = None;
        self.password_idx += 1;

        if self.password_idx >= self.prompt_users.len() {
            // All secrets collected; proceed to install.
            self.start_install();
            self.current = Screen::Installing;
        }
    }

    /// Compile the selected config into a manifest on a blocking worker. Drives
    /// the `ConfigValidate` → `Confirm`/`ConfigError` transition via `poll_streams`.
    fn start_config_compile(&mut self) {
        self.compile_error = None;
        self.compiled_manifest = None;
        self.compile_scroll = 0;
        self.confirm_view_manifest = false;
        self.manifest_scroll = 0;
        self.manifest_text = None;
        self.manifest_read_error = None;
        self.compiling = true;
        if let Some(handle) = self.compile_task.take() {
            handle.abort();
        }

        let (tx, rx) = mpsc::unbounded_channel();
        self.compile_rx = Some(rx);
        let config_path = self.config_file_path();
        // Land manifest.toml in the installer's working directory (the live
        // root's /root), then hand its path to provisioning.
        let out_dir = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));

        self.compile_task = Some(tokio::task::spawn_blocking(move || {
            let result = match config_path {
                Some(path) => oxys::compile::compile_config_file(
                    std::path::Path::new(&path),
                    &oxys::compile::oxys_crate_path(),
                    &out_dir,
                ),
                None => Err(oxys::compile::CompileError::message(
                    "no config file selected",
                )),
            };
            let _ = tx.send(CompileEvent::Done(result));
        }));
    }

    pub(crate) fn poll_streams(&mut self) {
        let mut hardware_events = Vec::new();
        let mut hardware_disconnected = false;

        if let Some(rx) = &mut self.hardware_rx {
            loop {
                match rx.try_recv() {
                    Ok(event) => hardware_events.push(event),
                    Err(TryRecvError::Empty) => break,
                    Err(TryRecvError::Disconnected) => {
                        hardware_disconnected = true;
                        break;
                    }
                }
            }
        }

        for event in hardware_events {
            match event {
                HardwareDetectEvent::Row(key, value) => {
                    self.hardware_rows.push((key, value));
                    self.update_hardware_summary();
                }
                HardwareDetectEvent::Done => {
                    self.hardware_detect_done = true;
                    self.hardware_detecting = false;
                    self.hardware_action_idx = 1;
                }
            }
        }

        if hardware_disconnected {
            self.hardware_rx = None;
            self.hardware_detecting = false;
        }

        if let Some(rx) = &mut self.partition_rx {
            loop {
                match rx.try_recv() {
                    Ok(line) => self.partition_lines.push(line),
                    Err(TryRecvError::Empty) => break,
                    Err(TryRecvError::Disconnected) => {
                        self.partition_rx = None;
                        break;
                    }
                }
            }
        }

        if let Some(rx) = &mut self.install_rx {
            loop {
                match rx.try_recv() {
                    Ok(line) => {
                        // Control lines drive the progress bar off real step
                        // counts (see provisioning::send_progress); they are
                        // never shown in the log.
                        if let Some(rest) = line.strip_prefix(provisioning::PROGRESS_PREFIX) {
                            if let Ok(percent) = rest.trim().parse::<u16>() {
                                self.install_progress = percent.min(100);
                            }
                            continue;
                        }
                        append_install_log(&line);
                        // rsync redraws its progress on one line via carriage
                        // returns; each refresh now streams as its own line.
                        // Collapse those in place so a multi-minute copy doesn't
                        // bury the log under thousands of near-identical lines.
                        if is_rsync_progress(&line)
                            && self
                                .install_lines
                                .last()
                                .is_some_and(|last| is_rsync_progress(last))
                        {
                            if let Some(last) = self.install_lines.last_mut() {
                                *last = line;
                            }
                        } else {
                            self.install_lines.push(line);
                        }
                    }
                    Err(TryRecvError::Empty) => break,
                    Err(TryRecvError::Disconnected) => {
                        self.install_rx = None;
                        // Only advance to the "done" (success) screen on a clean
                        // run. If any step emitted an `[error]` line the install did
                        // NOT complete, so stay on the install screen: draw_install
                        // renders "✗ installation blocked" plus the full log. The
                        // old unconditional jump to Done hid the real failure behind
                        // a hardcoded "Installation complete" -- so a broken install
                        // (e.g. `zpool create` failing) looked like a success.
                        let failed = self
                            .install_lines
                            .iter()
                            .any(|line| line.starts_with("[error]"));
                        // Fill the bar only on a clean finish; a failed run keeps
                        // its last partial value so the UI never shows 100%/"complete"
                        // for an install that actually aborted.
                        if !failed {
                            self.install_progress = 100;
                            if self.current == Screen::Installing {
                                self.current = Screen::Done;
                            }
                        }
                        break;
                    }
                }
            }
        }

        if let Some(rx) = &mut self.compile_rx {
            match rx.try_recv() {
                Ok(CompileEvent::Done(result)) => {
                    self.compiling = false;
                    self.compile_rx = None;
                    match result {
                        Ok(path) => {
                            match fs::read_to_string(&path) {
                                Ok(text) => {
                                    self.manifest_text = Some(text);
                                    self.manifest_read_error = None;
                                }
                                Err(err) => {
                                    self.manifest_text = None;
                                    self.manifest_read_error =
                                        Some(format!("failed to read {}: {err}", path.display()));
                                }
                            }
                            // Classify the compiled manifest's packages for the
                            // review screen. Pure/cheap: manifest only, no
                            // network or Portage tree. On load failure we simply
                            // show no summary rather than block the install.
                            self.package_summary = oxys::compile::load_manifest(&path)
                                .ok()
                                .map(|manifest| oxys::summarize(&manifest));
                            self.package_scroll = 0;
                            self.compiled_manifest = Some(path);
                            if self.current == Screen::ConfigValidate {
                                self.current = Screen::PackageSummary;
                            }
                        }
                        Err(err) => {
                            self.compile_error = Some(err);
                            if self.current == Screen::ConfigValidate {
                                self.current = Screen::ConfigError;
                            }
                        }
                    }
                }
                Err(TryRecvError::Empty) => {}
                Err(TryRecvError::Disconnected) => {
                    // Worker vanished without a result; drop back to selection.
                    self.compiling = false;
                    self.compile_rx = None;
                    if self.current == Screen::ConfigValidate {
                        self.current = Screen::ConfigSelect;
                    }
                }
            }
        }
    }

    pub(crate) fn on_tick(&mut self) {
        self.tick_count = self.tick_count.wrapping_add(1);
        if self.hardware_detecting || self.compiling {
            self.hardware_spinner_idx = (self.hardware_spinner_idx + 1) % SPINNER.len();
        }
    }

    pub(crate) fn splash_lines_visible(&self, max_lines: u16) -> usize {
        (self.tick_count as usize).min(max_lines as usize)
    }

    pub(crate) fn on_key(&mut self, key: KeyEvent) -> bool {
        if self.confirm_quit {
            match key.code {
                KeyCode::Enter => return true,
                KeyCode::Esc | KeyCode::Char('q') => self.confirm_quit = false,
                _ => {}
            }
            return false;
        }

        // On the Done screen the install is finished: Enter reboots into the
        // freshly installed system (otherwise the machine drops back to the
        // live medium), while `q` still quits to a shell for inspection.
        if self.current == Screen::Done {
            match key.code {
                KeyCode::Enter => {
                    self.reboot_requested = true;
                    return true;
                }
                KeyCode::Char('q') => return true,
                _ => {}
            }
            return false;
        }

        // The username/password screens capture every key so typed characters
        // never reach the global shortcuts below.
        if self.current == Screen::Usernames {
            self.username_key(key);
            return false;
        }
        if self.current == Screen::Passwords {
            self.password_key(key);
            return false;
        }

        match key.code {
            KeyCode::Char('q') => self.confirm_quit = true,
            KeyCode::Esc => self.go_back(),
            KeyCode::Enter => self.go_next(),
            KeyCode::Up => self.up(),
            KeyCode::Down => self.down(),
            KeyCode::Left => self.left(),
            KeyCode::Right => self.right(),
            KeyCode::PageUp => self.page_up(),
            KeyCode::PageDown => self.page_down(),
            KeyCode::Char('m') if self.current == Screen::Confirm => {
                self.confirm_view_manifest = !self.confirm_view_manifest;
                self.manifest_scroll = 0;
            }
            KeyCode::Char(' ') if self.current == Screen::DiskSelect => {
                self.apply_target_cursor();
            }
            _ => {}
        }

        if key.code == KeyCode::Char('g')
            && key.modifiers.contains(KeyModifiers::CONTROL)
            && matches!(self.current, Screen::ConfigSelect | Screen::ConfigError)
        {
            self.start_config_edit();
        }
        false
    }

    fn up(&mut self) {
        match self.current {
            Screen::HardwareDetection => {
                if self.hardware_action_idx > 0 {
                    self.hardware_action_idx -= 1;
                }
            }
            Screen::DiskSelect => {
                if self.target_cursor > 0 {
                    self.target_cursor -= 1;
                }
            }
            Screen::ConfigSelect if self.config_idx > 0 => {
                self.config_idx -= 1;
            }
            Screen::ConfigError => {
                self.compile_scroll = self.compile_scroll.saturating_sub(1);
            }
            Screen::PackageSummary => {
                self.package_scroll = self.package_scroll.saturating_sub(1);
            }
            Screen::Confirm if self.confirm_view_manifest => {
                self.manifest_scroll = self.manifest_scroll.saturating_sub(1);
            }
            _ => {}
        }
    }

    fn down(&mut self) {
        match self.current {
            Screen::HardwareDetection => {
                if self.hardware_action_idx < 1 {
                    self.hardware_action_idx += 1;
                }
            }
            Screen::DiskSelect => {
                let total = self.disks.len();
                if self.target_cursor + 1 < total {
                    self.target_cursor += 1;
                }
            }
            Screen::ConfigSelect if self.config_idx < 2 => {
                self.config_idx += 1;
            }
            Screen::ConfigError => {
                self.compile_scroll = self.compile_scroll.saturating_add(1);
            }
            Screen::PackageSummary => {
                self.package_scroll = self.package_scroll.saturating_add(1);
            }
            Screen::Confirm if self.confirm_view_manifest => {
                self.manifest_scroll = self.manifest_scroll.saturating_add(1);
            }
            _ => {}
        }
    }

    fn left(&mut self) {
        match self.current {
            Screen::HardwareDetection if self.hardware_action_idx > 0 => {
                self.hardware_action_idx -= 1;
            }
            _ => {}
        }
    }

    fn right(&mut self) {
        match self.current {
            Screen::HardwareDetection if self.hardware_action_idx < 1 => {
                self.hardware_action_idx += 1;
            }
            _ => {}
        }
    }

    fn page_up(&mut self) {
        match self.current {
            Screen::ConfigError => {
                self.compile_scroll = self.compile_scroll.saturating_sub(10);
            }
            Screen::PackageSummary => {
                self.package_scroll = self.package_scroll.saturating_sub(10);
            }
            Screen::Confirm if self.confirm_view_manifest => {
                self.manifest_scroll = self.manifest_scroll.saturating_sub(10);
            }
            _ => {}
        }
    }

    fn page_down(&mut self) {
        match self.current {
            Screen::ConfigError => {
                self.compile_scroll = self.compile_scroll.saturating_add(10);
            }
            Screen::PackageSummary => {
                self.package_scroll = self.package_scroll.saturating_add(10);
            }
            Screen::Confirm if self.confirm_view_manifest => {
                self.manifest_scroll = self.manifest_scroll.saturating_add(10);
            }
            _ => {}
        }
    }

    fn go_back(&mut self) {
        if self.current == Screen::Confirm && self.confirm_view_manifest {
            self.confirm_view_manifest = false;
            self.manifest_scroll = 0;
            return;
        }

        self.current = match self.current {
            Screen::Welcome => Screen::Welcome,
            Screen::HardwareDetection => Screen::Welcome,
            Screen::DiskSelect => Screen::HardwareDetection,
            Screen::Partition => {
                self.refresh_disks();
                let n = self.disks.len();
                self.target_cursor = if n > 0 { self.disk_idx.min(n - 1) } else { 0 };
                Screen::DiskSelect
            } // hidden for now
            Screen::ConfigSelect => {
                self.refresh_disks();
                // reset focus to the selected disk (fs position would shift if #disks changed)
                let n = self.disks.len();
                self.target_cursor = if n > 0 { self.disk_idx.min(n - 1) } else { 0 };
                Screen::DiskSelect
            }
            Screen::ConfigValidate => {
                // Cancel the in-flight compile and return to selection.
                if let Some(handle) = self.compile_task.take() {
                    handle.abort();
                }
                self.compiling = false;
                self.compile_rx = None;
                Screen::ConfigSelect
            }
            Screen::ConfigError => Screen::ConfigSelect,
            Screen::PackageSummary => {
                self.package_scroll = 0;
                Screen::ConfigSelect
            }
            Screen::Confirm => {
                self.confirm_view_manifest = false;
                self.manifest_scroll = 0;
                self.package_scroll = 0;
                Screen::PackageSummary
            }
            Screen::Usernames => Screen::Confirm,
            Screen::Passwords => Screen::Confirm,
            Screen::Installing => Screen::Confirm,
            Screen::Done => Screen::Installing,
        };
    }

    fn go_next(&mut self) {
        self.current = match self.current {
            Screen::Welcome => Screen::HardwareDetection,
            Screen::HardwareDetection => match self.hardware_action_idx {
                0 => {
                    if !self.hardware_detecting {
                        self.start_hardware_detect();
                    }
                    Screen::HardwareDetection
                }
                1 => {
                    if self.hardware_detect_done {
                        self.refresh_disks();
                        // position cursor on the current disk (or first fs item if no disks)
                        let n = self.disks.len();
                        self.target_cursor = if n > 0 { self.disk_idx.min(n - 1) } else { 0 };
                        Screen::DiskSelect
                    } else {
                        Screen::HardwareDetection
                    }
                }
                _ => Screen::HardwareDetection,
            },
            Screen::DiskSelect => {
                // self.start_partition_plan();  // step 4 (partition) hidden for now
                Screen::ConfigSelect
            }
            Screen::Partition => Screen::ConfigSelect, // hidden for now
            Screen::ConfigSelect => {
                // Gate: compile the selected config before continuing.
                self.start_config_compile();
                Screen::ConfigValidate
            }
            Screen::ConfigValidate => Screen::ConfigValidate, // busy; ignore enter
            Screen::ConfigError => {
                // Retry the compile (e.g. after a Ctrl+G edit).
                self.start_config_compile();
                Screen::ConfigValidate
            }
            Screen::PackageSummary => Screen::Confirm,
            Screen::Confirm => {
                if self.confirm_view_manifest {
                    Screen::Confirm
                } else {
                    self.begin_identity_collection()
                }
            }
            // Enter on the username/password screens is handled by
            // username_key/password_key, not here.
            Screen::Usernames => Screen::Usernames,
            Screen::Passwords => Screen::Passwords,
            Screen::Installing => Screen::Installing,
            Screen::Done => Screen::Done,
        }
    }

    fn update_hardware_summary(&mut self) {
        let mut cpu = "unknown".to_string();
        let mut ram = "unknown".to_string();
        let mut gpu = "unknown".to_string();
        let mut power = "unknown".to_string();

        for (k, v) in &self.hardware_rows {
            match k.as_str() {
                "CPU" => cpu = v.clone(),
                "RAM" => ram = v.clone(),
                "GPU" => gpu = v.clone(),
                "Power" => power = v.clone(),
                _ => {}
            }
        }

        self.hardware_short = format!("{}  ·  {}", cpu, ram);
        self.hardware_full = format!("CPU {}, RAM {}, GPU {}, Power {}", cpu, ram, gpu, power);
    }

    fn apply_target_cursor(&mut self) {
        // The disk-select screen only chooses the target disk now; the
        // filesystem is always ext4 whole-disk.
        if self.target_cursor < self.disks.len() {
            self.disk_idx = self.target_cursor;
        }
    }

    fn start_config_edit(&mut self) {
        if let Some(path) = self.config_file_path() {
            self.pending_edit = Some(path);
        }
    }

    pub(crate) fn take_pending_edit(&mut self) -> Option<String> {
        self.pending_edit.take()
    }

    fn config_file_path(&self) -> Option<String> {
        let name = match self.config_idx {
            0 => "base.rs",
            1 => "desktop.rs",
            2 => "custom",
            _ => return None,
        };
        let filename = if name == "custom" { "custom.rs" } else { name };
        Some(format!("configs/{}", filename))
    }

    pub(crate) fn config_display_name(&self, base_name: &str) -> String {
        let filename = if base_name == "custom" {
            "custom.rs"
        } else {
            base_name
        };
        let path = format!("configs/{}", filename);
        let current = fs::metadata(&path).ok().and_then(|m| m.modified().ok());
        let initial = self.initial_config_mtimes.get(base_name).copied().flatten();
        if let (Some(curr), Some(init)) = (current, initial) {
            if curr > init {
                format!("{} (edited)", base_name)
            } else {
                base_name.to_string()
            }
        } else {
            base_name.to_string()
        }
    }

    fn refresh_disks(&mut self) {
        self.disks = detect_disks();
        if !self.disks.is_empty() && self.disk_idx >= self.disks.len() {
            self.disk_idx = 0;
        }
    }
}
