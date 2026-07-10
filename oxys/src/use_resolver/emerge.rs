use std::{path::Path, process::Command};

use crate::exec::ProcessStream;

use super::{util::strip_version_suffix, UseResolverError};

/// Structured line events emitted while streaming `emerge` output.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EmergeLine {
    /// A package build has started.
    BuildStart { package: String },
    /// A non-terminal progress line associated with the current package when known.
    BuildProgress {
        package: Option<String>,
        line: String,
    },
    /// A package build has completed.
    BuildComplete { package: String },
    /// Fetching of package sources has started.
    FetchStart { package: String },
    /// Fetching of package sources has completed.
    FetchComplete { package: String },
    /// An error line was detected in the stream.
    Error {
        package: Option<String>,
        message: String,
    },
}

/// Streaming iterator over structured `emerge` output events.
pub struct EmergeStream {
    output: ProcessStream,
    parser_state: ParserState,
    exhausted: bool,
}

/// Starts `emerge` and returns a streaming handle for line-oriented TUI consumption.
///
/// Call [`EmergeStream::wait`] after consuming the iterator to observe the final process exit
/// status. Output parsing is heuristic and based on common emerge line formats.
pub fn run_emerge(
    packages: &[String],
    root: &Path,
    portage_tmpdir: &Path,
    jobs: usize,
    use_binpkgs: bool,
) -> Result<EmergeStream, UseResolverError> {
    let mut command = Command::new("emerge");
    command
        .arg("--root")
        .arg(root)
        .arg("--jobs")
        .arg(jobs.to_string());
    if use_binpkgs {
        command.arg("--getbinpkg").arg("--usepkg");
    }
    command.env("PORTAGE_TMPDIR", portage_tmpdir).args(packages);
    let output = ProcessStream::spawn(command)?;

    Ok(EmergeStream {
        output,
        parser_state: ParserState::default(),
        exhausted: false,
    })
}

/// Starts `emerge` inside a target chroot and returns a streaming handle.
pub fn run_emerge_chroot(
    packages: &[String],
    target: &Path,
    portage_tmpdir: &Path,
    jobs: usize,
    use_binpkgs: bool,
) -> Result<EmergeStream, UseResolverError> {
    let mut command = Command::new("chroot");
    command
        .arg(target)
        .arg("env")
        .arg(format!("PORTAGE_TMPDIR={}", portage_tmpdir.display()))
        .arg("emerge")
        .arg("--root")
        .arg("/")
        .arg("--jobs")
        .arg(jobs.to_string());
    if use_binpkgs {
        command.arg("--getbinpkg").arg("--usepkg");
    }
    command.args(packages);
    let output = ProcessStream::spawn(command)?;

    Ok(EmergeStream {
        output,
        parser_state: ParserState::default(),
        exhausted: false,
    })
}

impl EmergeStream {
    /// Waits for the `emerge` subprocess to exit after draining any remaining output.
    pub fn wait(mut self) -> Result<(), UseResolverError> {
        self.drain_remaining()?;
        let status = self.output.wait_for_exit()?;

        if status.success() {
            return Ok(());
        }

        let package = self
            .parser_state
            .failed_package
            .or(self.parser_state.current_package)
            .map(|value| format!(" for package {value}"))
            .unwrap_or_default();
        let message = self
            .parser_state
            .last_error_message
            .map(|value| format!(": {value}"))
            .unwrap_or_default();

        Err(UseResolverError::EmergeExit {
            status: status.to_string(),
            package,
            message,
        })
    }

    fn drain_remaining(&mut self) -> Result<(), UseResolverError> {
        while !self.exhausted {
            if self.next_internal()?.is_none() {
                break;
            }
        }

        Ok(())
    }

    fn next_internal(&mut self) -> Result<Option<EmergeLine>, UseResolverError> {
        if self.exhausted {
            return Ok(None);
        }

        match self.output.next_line() {
            Ok(Some(line)) => {
                let event = parse_emerge_line(&line, &mut self.parser_state);
                Ok(Some(event))
            }
            Ok(None) => {
                self.exhausted = true;
                Ok(None)
            }
            Err(error) => {
                self.exhausted = true;
                Err(error.into())
            }
        }
    }
}

impl Iterator for EmergeStream {
    type Item = EmergeLine;

    fn next(&mut self) -> Option<Self::Item> {
        match self.next_internal() {
            Ok(item) => item,
            Err(error) => {
                self.exhausted = true;
                let message = error.to_string();
                self.parser_state.last_error_message = Some(message.clone());
                Some(EmergeLine::Error {
                    package: self.parser_state.current_package.clone(),
                    message,
                })
            }
        }
    }
}

#[derive(Debug, Default)]
struct ParserState {
    current_package: Option<String>,
    failed_package: Option<String>,
    last_error_message: Option<String>,
    fetch_package: Option<String>,
}

fn parse_emerge_line(line: &str, state: &mut ParserState) -> EmergeLine {
    if let Some(package) = parse_prefixed_package(line, ">>> Emerging") {
        state.current_package = Some(package.clone());
        return EmergeLine::BuildStart { package };
    }

    if let Some(package) = parse_prefixed_package(line, ">>> Fetching") {
        state.fetch_package = Some(package.clone());
        return EmergeLine::FetchStart { package };
    }

    if line.starts_with(">>> Downloading") {
        if let Some(package) = state.current_package.clone() {
            state.fetch_package = Some(package.clone());
            return EmergeLine::FetchStart { package };
        }
    }

    if let Some(package) = parse_prefixed_package(line, ">>> Completed installing") {
        state.current_package = Some(package.clone());
        return EmergeLine::BuildComplete { package };
    }

    if line.starts_with(">>> Fetch completed") || line.starts_with(">>> Checking") {
        if let Some(package) = state
            .fetch_package
            .clone()
            .or_else(|| state.current_package.clone())
        {
            return EmergeLine::FetchComplete { package };
        }
    }

    if is_error_line(line) {
        let package = parse_package_token(line).or_else(|| state.current_package.clone());
        if let Some(detected) = package.clone() {
            state.failed_package = Some(detected);
        }
        state.last_error_message = Some(line.to_owned());
        return EmergeLine::Error {
            package,
            message: line.to_owned(),
        };
    }

    EmergeLine::BuildProgress {
        package: state.current_package.clone(),
        line: line.to_owned(),
    }
}

fn parse_prefixed_package(line: &str, prefix: &str) -> Option<String> {
    if !line.starts_with(prefix) {
        return None;
    }

    parse_package_token(line)
}

fn parse_package_token(line: &str) -> Option<String> {
    line.split_whitespace()
        .find_map(|token| normalize_package_token(token))
}

fn normalize_package_token(token: &str) -> Option<String> {
    let trimmed = token
        .trim_matches(|ch: char| matches!(ch, '(' | ')' | '[' | ']' | ',' | ';' | '\''))
        .trim_start_matches("::");

    if !trimmed.contains('/') {
        return None;
    }

    let base = trimmed
        .split_once("::")
        .map(|(package, _)| package)
        .unwrap_or(trimmed)
        .split_once(':')
        .map(|(package, _)| package)
        .unwrap_or(trimmed);

    let (category, package) = base.split_once('/')?;
    if category.is_empty() || package.is_empty() {
        return None;
    }

    let stripped = strip_version_suffix(package);
    if stripped.is_empty() {
        return None;
    }

    Some(format!("{category}/{stripped}"))
}

fn is_error_line(line: &str) -> bool {
    let trimmed = line.trim_start();
    trimmed.starts_with("!!!")
        || trimmed.starts_with("* ERROR:")
        || trimmed.starts_with("ERROR:")
        || trimmed.starts_with("Error:")
        || trimmed.starts_with("emerge: there are no ebuilds")
}

/// Returns the argv vector that `run_emerge` would construct (for verification in tests
/// that the binary/source decision flows through to the actual emerge command line).
pub fn emerge_command_for_test(
    packages: &[String],
    root: &Path,
    jobs: usize,
    use_binpkgs: bool,
) -> Vec<String> {
    let mut args = vec![
        "emerge".to_string(),
        "--root".to_string(),
        root.to_string_lossy().to_string(),
        "--jobs".to_string(),
        jobs.to_string(),
    ];
    if use_binpkgs {
        args.push("--getbinpkg".to_string());
        args.push("--usepkg".to_string());
    }
    args.extend(packages.iter().cloned());
    args
}

/// Returns the argv vector that `run_emerge_chroot` would construct.
pub fn emerge_chroot_command_for_test(
    packages: &[String],
    target: &Path,
    portage_tmpdir: &Path,
    jobs: usize,
    use_binpkgs: bool,
) -> Vec<String> {
    let mut args = vec![
        "chroot".to_string(),
        target.to_string_lossy().to_string(),
        "env".to_string(),
        format!("PORTAGE_TMPDIR={}", portage_tmpdir.display()),
        "emerge".to_string(),
        "--root".to_string(),
        "/".to_string(),
        "--jobs".to_string(),
        jobs.to_string(),
    ];
    if use_binpkgs {
        args.push("--getbinpkg".to_string());
        args.push("--usepkg".to_string());
    }
    args.extend(packages.iter().cloned());
    args
}

#[cfg(test)]
mod tests {
    use super::{parse_emerge_line, EmergeLine, ParserState};

    #[test]
    fn parses_build_start_line() {
        let mut state = ParserState::default();

        let event = parse_emerge_line(
            ">>> Emerging (1 of 1) gui-wm/niri-25.11-r1::guru",
            &mut state,
        );

        assert_eq!(
            event,
            EmergeLine::BuildStart {
                package: "gui-wm/niri".to_owned()
            }
        );
    }

    #[test]
    fn parses_build_complete_line() {
        let mut state = ParserState::default();

        let event = parse_emerge_line(
            ">>> Completed installing gui-wm/niri-25.11-r1 into /",
            &mut state,
        );

        assert_eq!(
            event,
            EmergeLine::BuildComplete {
                package: "gui-wm/niri".to_owned()
            }
        );
    }

    #[test]
    fn parses_fetch_events_using_current_package() {
        let mut state = ParserState::default();
        let _ = parse_emerge_line(
            ">>> Emerging (1 of 1) gui-wm/niri-25.11-r1::guru",
            &mut state,
        );

        let event = parse_emerge_line(
            ">>> Downloading 'https://example.invalid/src.tar.xz'",
            &mut state,
        );

        assert_eq!(
            event,
            EmergeLine::FetchStart {
                package: "gui-wm/niri".to_owned()
            }
        );
    }

    #[test]
    fn parses_fetch_complete_after_fetch_start() {
        let mut state = ParserState::default();
        let _ = parse_emerge_line(">>> Fetching gui-wm/niri-25.11-r1::guru", &mut state);

        let event = parse_emerge_line(">>> Fetch completed for gui-wm/niri", &mut state);

        assert_eq!(
            event,
            EmergeLine::FetchComplete {
                package: "gui-wm/niri".to_owned()
            }
        );
    }

    #[test]
    fn parses_error_lines_and_tracks_failed_package() {
        let mut state = ParserState::default();
        let _ = parse_emerge_line(
            ">>> Emerging (1 of 1) gui-wm/niri-25.11-r1::guru",
            &mut state,
        );

        let event = parse_emerge_line(
            "!!! gui-wm/niri-25.11-r1 failed (compile phase)",
            &mut state,
        );

        assert_eq!(
            event,
            EmergeLine::Error {
                package: Some("gui-wm/niri".to_owned()),
                message: "!!! gui-wm/niri-25.11-r1 failed (compile phase)".to_owned()
            }
        );
    }

    #[test]
    fn treats_plain_failed_text_as_progress() {
        let mut state = ParserState::default();
        let _ = parse_emerge_line(
            ">>> Emerging (1 of 1) gui-wm/niri-25.11-r1::guru",
            &mut state,
        );

        let event = parse_emerge_line("0 failed, 128 passed", &mut state);

        assert_eq!(
            event,
            EmergeLine::BuildProgress {
                package: Some("gui-wm/niri".to_owned()),
                line: "0 failed, 128 passed".to_owned()
            }
        );
        assert_eq!(state.failed_package, None);
        assert_eq!(state.last_error_message, None);
    }

    #[test]
    fn parses_prefixed_error_line() {
        let mut state = ParserState::default();

        let event = parse_emerge_line(" * ERROR: gui-wm/niri failed", &mut state);

        assert_eq!(
            event,
            EmergeLine::Error {
                package: Some("gui-wm/niri".to_owned()),
                message: " * ERROR: gui-wm/niri failed".to_owned()
            }
        );
    }

    #[test]
    fn falls_back_to_progress_for_unrecognized_lines() {
        let mut state = ParserState::default();

        let event = parse_emerge_line(" * running postinst hooks", &mut state);

        assert_eq!(
            event,
            EmergeLine::BuildProgress {
                package: None,
                line: " * running postinst hooks".to_owned()
            }
        );
    }
}
