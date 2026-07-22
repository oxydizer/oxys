use std::time::Duration;

use tokio::sync::mpsc::error::TryRecvError;

use crate::{
    hardware::HardwareDetectEvent,
    provisioning,
    ui::theme::{ASCII_SPINNER, SPINNER},
};

use super::{App, CompileEvent, Screen, append_install_log, is_rsync_progress};

impl App {
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

        if let Some(rx) = &mut self.network_rx {
            match rx.try_recv() {
                Ok(online) => {
                    self.network_online = Some(online);
                    self.network_rx = None;
                }
                Err(TryRecvError::Empty) => {}
                Err(TryRecvError::Disconnected) => self.network_rx = None,
            }
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
                        self.install_elapsed = self
                            .install_started_at
                            .take()
                            .map(|started_at| started_at.elapsed());
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
                    self.compile_started_at = None;
                    self.compile_rx = None;
                    match result {
                        Ok(outcome) => {
                            self.compile_progress = 100;
                            self.compile_notices = outcome.notices;
                            self.accept_compiled_manifest(outcome.manifest_path);
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
                    self.compile_progress = 0;
                    self.compile_started_at = None;
                    self.compile_rx = None;
                    if self.current == Screen::ConfigValidate {
                        self.current = Screen::ConfigSelect;
                    }
                }
            }
        }

        if let Some(rx) = &mut self.custom_fetch_rx {
            match rx.try_recv() {
                Ok(Ok(())) => {
                    self.custom_fetching = false;
                    self.custom_fetch_rx = None;
                    // Fetched straight into configs/custom.fe2o3, the same spot
                    // the default "custom" profile already resolves to.
                    self.custom_config_path = None;
                    self.start_config_compile();
                    self.current = Screen::ConfigValidate;
                }
                Ok(Err(err)) => {
                    self.custom_fetching = false;
                    self.custom_fetch_rx = None;
                    self.custom_source_error = Some(err);
                }
                Err(TryRecvError::Empty) => {}
                Err(TryRecvError::Disconnected) => {
                    self.custom_fetching = false;
                    self.custom_fetch_rx = None;
                }
            }
        }
    }

    pub(crate) fn on_tick(&mut self) {
        self.tick_count = self.tick_count.wrapping_add(1);
        if self.compiling
            && let Some(started) = self.compile_started_at {
                // Cargo exposes no meaningful percentage for this tiny
                // generated crate. Let the estimate advance smoothly while
                // reserving 100% for the actual worker result.
                self.compile_progress = self
                    .compile_progress
                    .max(estimated_compile_progress(started.elapsed()));
            }
        if self.hardware_detecting
            || self.compiling
            || self.custom_fetching
            || self.current == Screen::Installing
        {
            self.hardware_spinner_idx = (self.hardware_spinner_idx + 1) % SPINNER.len();
        }
        if self.network_online.is_none() {
            self.network_spinner_idx = (self.network_spinner_idx + 1) % ASCII_SPINNER.len();
        }
    }

    pub(crate) fn splash_lines_visible(&self, max_lines: u16) -> usize {
        (self.tick_count as usize).min(max_lines as usize)
    }

    /// Software blink for terminals that ignore ANSI SLOW_BLINK (Linux VT /
    /// fbcon on bare metal). ~530 ms on / off at the 30 ms app tick.
    pub(crate) fn blink_on(&self) -> bool {
        const HALF_PERIOD_TICKS: u64 = 18;
        (self.tick_count / HALF_PERIOD_TICKS).is_multiple_of(2)
    }
}

/// A deliberately conservative, time-based estimate for config compilation.
/// It moves quickly for a warm cache, then slows and caps at 95% for cold or
/// unusually slow builds. Only `CompileEvent::Done(Ok(_))` represents success.
fn estimated_compile_progress(elapsed: Duration) -> u16 {
    let seconds = elapsed.as_secs_f64();
    let percent = if seconds <= 1.0 {
        5.0 + 30.0 * seconds
    } else if seconds <= 5.0 {
        35.0 + 35.0 * ((seconds - 1.0) / 4.0)
    } else if seconds <= 15.0 {
        70.0 + 18.0 * ((seconds - 5.0) / 10.0)
    } else {
        88.0 + (7.0 * ((seconds - 15.0) / 15.0)).min(7.0)
    };
    (percent.floor() as u16).min(95)
}

#[cfg(test)]
mod tests {
    use super::estimated_compile_progress;
    use std::time::Duration;

    #[test]
    fn compile_estimate_advances_but_never_claims_completion() {
        let samples = [0, 1, 5, 15, 30, 300]
            .map(|seconds| estimated_compile_progress(Duration::from_secs(seconds)));

        assert_eq!(samples[0], 5);
        assert!(samples.windows(2).all(|pair| pair[0] <= pair[1]));
        assert_eq!(samples.last(), Some(&95));
    }
}
