use std::fs;
use std::path::PathBuf;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use tokio::sync::mpsc;

use oxys::detect::detect_disks;

use crate::{hardware, network, provisioning};

use super::{App, CompileEvent, Screen, INSTALL_LOG_PATH};

impl App {
    /// Kicks off the one-shot startup connectivity probe (see [`network`])
    /// that drives the online/offline indicator in the header.
    pub(super) fn start_network_check(&mut self) {
        let (tx, rx) = mpsc::unbounded_channel();
        self.network_rx = Some(rx);
        self.network_task = Some(tokio::spawn(network::check_connectivity(tx)));
    }

    pub(super) fn start_hardware_detect(&mut self) {
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

    pub(super) fn start_install(&mut self) {
        // Never re-enter while a worker is still streaming. abort() on a
        // spawn_blocking task does not cancel wipefs/rsync; a second start
        // would interleave destructive steps on the same disk.
        if self.install_in_progress() {
            return;
        }

        self.install_lines.clear();
        // Start a fresh on-disk log for this run so a failure is inspectable
        // after the TUI exits (`cat /var/log/oxys-install.log`, or read it over
        // the QEMU serial console). Best-effort: never block the install on it.
        let _ = fs::write(INSTALL_LOG_PATH, b"");
        self.install_progress = 0;
        // Drop any finished worker handle; install_in_progress() is false so
        // this is not a live task we need to keep.
        let _ = self.install_task.take();

        let (tx, rx) = mpsc::unbounded_channel();
        self.install_rx = Some(rx);
        let disk = self.selected_disk_config();
        let manifest = self.compiled_manifest.clone();
        // The declarative source that was compiled for this install; seeded to
        // /etc/oxys/config.fe2o3 on the target so `oxys install` can later edit it.
        let config_source = self.config_file_path().map(PathBuf::from);
        // Move the collected secrets/names into the worker so they don't
        // linger in App state, and clear the entry buffers.
        let timezone = self.collected_timezone.take();
        let usernames = std::mem::take(&mut self.collected_usernames);
        self.username_input.clear();
        let passwords = std::mem::take(&mut self.collected_passwords);
        self.password_input.clear();
        self.password_confirm_input.clear();

        self.install_task = Some(tokio::task::spawn_blocking(move || {
            provisioning::run_install(
                disk,
                manifest,
                config_source,
                timezone,
                usernames,
                passwords,
                tx,
            );
        }));
    }

    /// Compile the selected config into a manifest on a blocking worker. Drives
    /// the `ConfigValidate` → `Confirm`/`ConfigError` transition via `poll_streams`.
    ///
    /// Always goes through the compile step UI. Unedited stock `desktop` /
    /// `base` profiles short-circuit cargo by reusing the ISO prebuilt
    /// manifest (still delivered as a worker `Done` event so the screen
    /// flow is identical, just much faster).
    pub(super) fn start_config_compile(&mut self) {
        self.compile_error = None;
        self.compile_notices = Vec::new();
        self.defaults_report = Vec::new();
        self.compiled_manifest = None;
        self.compile_scroll = 0;
        self.confirm_view_manifest = false;
        self.manifest_scroll = 0;
        self.manifest_text = None;
        self.manifest_read_error = None;
        self.package_summary = None;
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
                Some(path) => {
                    let path = std::path::Path::new(&path);
                    // Prefer the ISO prebuilt when the stock source is unchanged.
                    if let Some(prebuilt) = crate::prebuilt::try_prebuilt_manifest(path) {
                        Ok(oxys::compile::CompileOutcome {
                            manifest_path: prebuilt,
                            notices: Vec::new(),
                        })
                    } else {
                        oxys::compile::compile_config_file(
                            path,
                            &oxys::compile::oxys_crate_path(),
                            &out_dir,
                        )
                    }
                }
                None => Err(oxys::compile::CompileError::message(
                    "no config file selected",
                )),
            };
            let _ = tx.send(CompileEvent::Done(result));
        }));
    }

    /// Apply a successfully produced (or prebuilt) manifest: cache its text,
    /// build the package-summary view model, and remember the path for install.
    pub(super) fn accept_compiled_manifest(&mut self, path: PathBuf) {
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
        // Classify packages for the review screen. Pure/cheap: manifest only,
        // no network or Portage tree. On load failure we simply show no
        // summary rather than block the install.
        let manifest = oxys::compile::load_manifest(&path).ok();
        self.defaults_report = manifest
            .as_ref()
            .map(|manifest| {
                oxys::defaults_report::render_defaults_report(
                    &oxys::defaults_report::defaulted_settings(manifest),
                )
            })
            .unwrap_or_default();
        self.package_summary = manifest.map(|manifest| oxys::summarize(&manifest));
        self.package_scroll = 0;
        self.compiled_manifest = Some(path);
    }

    /// Handle a keystroke on the custom-source entry screen. Owns all keys so
    /// typed characters (including `q`) never trigger global shortcuts.
    pub(super) fn custom_source_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Esc => {
                self.custom_source_error = None;
                self.current = Screen::ConfigSelect;
            }
            KeyCode::Backspace => {
                self.custom_source_input.pop();
            }
            KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.custom_source_input.push(c);
            }
            KeyCode::Enter if !self.custom_fetching => self.custom_source_submit(),
            _ => {}
        }
    }

    /// Resolve whatever was typed on the custom-source screen: a blank entry
    /// falls back to the baked-in `configs/custom.fe2o3` template, an
    /// `http(s)://` URL is fetched via curl, and anything else is treated as
    /// a local path and must already exist.
    fn custom_source_submit(&mut self) {
        let raw = self.custom_source_input.trim().to_owned();

        if raw.is_empty() {
            self.custom_config_path = None;
            self.custom_source_error = None;
            self.start_config_compile();
            self.current = Screen::ConfigValidate;
            return;
        }

        if raw.starts_with("http://") || raw.starts_with("https://") {
            self.start_custom_fetch(raw);
            return;
        }

        if !std::path::Path::new(&raw).is_file() {
            self.custom_source_error = Some(format!("file not found: {raw}"));
            return;
        }

        self.custom_config_path = Some(raw);
        self.custom_source_error = None;
        self.start_config_compile();
        self.current = Screen::ConfigValidate;
    }

    /// Downloads `url` into `configs/custom.fe2o3` on a blocking worker; drives
    /// the fetch → compile handoff via `poll_streams`.
    fn start_custom_fetch(&mut self, url: String) {
        self.custom_source_error = None;
        self.custom_fetching = true;
        if let Some(handle) = self.custom_fetch_task.take() {
            handle.abort();
        }

        let (tx, rx) = mpsc::unbounded_channel();
        self.custom_fetch_rx = Some(rx);
        let dest = PathBuf::from("configs/custom.fe2o3");

        self.custom_fetch_task = Some(tokio::task::spawn_blocking(move || {
            let result = network::fetch_config(&url, &dest);
            let _ = tx.send(result);
        }));
    }

    pub(super) fn update_hardware_summary(&mut self) {
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

        self.hardware_short = format!("{}  ·  {} RAM", hardware::shorten_cpu_model(&cpu), ram);
        self.hardware_full = format!("CPU {}, RAM {}, GPU {}, Power {}", cpu, ram, gpu, power);
    }

    pub(super) fn refresh_disks(&mut self) {
        self.disks = detect_disks();
        if !self.disks.is_empty() && self.disk_idx >= self.disks.len() {
            self.disk_idx = 0;
        }
    }
}
