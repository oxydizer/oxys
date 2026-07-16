use std::fs;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use super::{App, Screen};

impl App {
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

        // While wipe/rsync is running the worker cannot be cancelled safely.
        // Swallow quit/back/navigation so we never leave Installing or start a
        // second destructive run on top of the first.
        if self.install_in_progress() {
            return false;
        }

        // The timezone/username/password/custom-source screens capture every
        // key so typed characters never reach the global shortcuts below.
        if self.current == Screen::Timezone {
            self.timezone_key(key);
            return false;
        }
        if self.current == Screen::Usernames {
            self.username_key(key);
            return false;
        }
        if self.current == Screen::Passwords {
            self.password_key(key);
            return false;
        }
        if self.current == Screen::CustomSource {
            self.custom_source_key(key);
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
            // Esc on CustomSource is handled by custom_source_key, not here.
            Screen::CustomSource => Screen::ConfigSelect,
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
            Screen::Timezone => Screen::Confirm,
            Screen::Usernames => Screen::Confirm,
            Screen::Passwords => Screen::Confirm,
            // Only reachable when the worker has finished (failed run); a live
            // install is locked out in on_key via install_in_progress().
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
                if self.config_idx == 2 {
                    // "custom" needs a source (file path or URL) first.
                    self.custom_source_error = None;
                    Screen::CustomSource
                } else {
                    // Gate: compile the selected config before continuing.
                    // Always land on ConfigValidate so the step is visible;
                    // unedited stock profiles finish almost immediately via
                    // the ISO prebuilt (see start_config_compile).
                    self.start_config_compile();
                    Screen::ConfigValidate
                }
            }
            // Enter on CustomSource is handled by custom_source_key, not here.
            Screen::CustomSource => Screen::CustomSource,
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
            // Enter on the timezone/username/password screens is handled by
            // timezone_key/username_key/password_key, not here.
            Screen::Timezone => Screen::Timezone,
            Screen::Usernames => Screen::Usernames,
            Screen::Passwords => Screen::Passwords,
            Screen::Installing => Screen::Installing,
            Screen::Done => Screen::Done,
        }
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

    pub(super) fn config_file_path(&self) -> Option<String> {
        if self.config_idx == 2 {
            if let Some(path) = &self.custom_config_path {
                return Some(path.clone());
            }
        }
        let name = match self.config_idx {
            0 => "desktop.fe2o3",
            1 => "base.fe2o3",
            2 => "custom",
            _ => return None,
        };
        let filename = if name == "custom" {
            "custom.fe2o3"
        } else {
            name
        };
        Some(format!("configs/{}", filename))
    }

    pub(crate) fn config_display_name(&self, base_name: &str) -> String {
        let filename = if base_name == "custom" {
            "custom.fe2o3"
        } else {
            base_name
        };
        let path = format!("configs/{}", filename);
        let current = fs::metadata(&path).ok().and_then(|m| m.modified().ok());
        let initial = self.initial_config_mtimes.get(base_name).copied().flatten();
        let display_name = base_name.strip_suffix(".fe2o3").unwrap_or(base_name);
        if let (Some(curr), Some(init)) = (current, initial) {
            if curr > init {
                format!("{} (edited)", display_name)
            } else {
                display_name.to_string()
            }
        } else {
            display_name.to_string()
        }
    }
}
