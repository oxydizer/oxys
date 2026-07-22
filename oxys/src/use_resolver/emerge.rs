use std::{path::Path, process::Command};

use crate::exec::{self, ProcessStream};

use super::{UseResolverError, util::strip_version_suffix};

/// Resolves the `emerge` binary to invoke, honoring `OXYS_EMERGE` so tests can
/// substitute a fake `emerge` script instead of touching a real Portage tree.
fn emerge_binary() -> String {
    std::env::var("OXYS_EMERGE").unwrap_or_else(|_| "emerge".to_owned())
}

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
    BuildComplete {
        package: String,
        /// Number of packages that have actually completed in this emerge run.
        completed: u32,
        /// Total merge operations reported by Portage's `(N of total)` marker.
        total: Option<u32>,
    },
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
    oneshot: bool,
) -> Result<EmergeStream, UseResolverError> {
    let mut command = Command::new(emerge_binary());
    command
        .arg("--root")
        .arg(root)
        .arg("--jobs")
        .arg(jobs.to_string());
    if oneshot {
        command.arg("--oneshot");
    }
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
///
/// Runs with `--update --changed-use`, so packages the target already has at
/// the requested version with unchanged effective USE are skipped rather than
/// unconditionally re-emerged (the target root is rsync'd from the live image,
/// so most manifest packages are already installed). Skipped packages are NOT
/// recorded in @world by emerge — callers that need world registration must
/// follow up with [`emerge_select`].
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
        .arg("--update")
        .arg("--changed-use")
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

/// Records already-installed packages in the Portage world set without rebuilding them.
/// No-op when `atoms` is empty.
pub fn emerge_select(atoms: &[String], root: &Path) -> Result<String, UseResolverError> {
    if atoms.is_empty() {
        return Ok(String::new());
    }
    run_world_command(&["--noreplace", "--select"], atoms, root)
}

/// Removes packages from the Portage world set. No-op when `atoms` is empty.
pub fn emerge_deselect(atoms: &[String], root: &Path) -> Result<String, UseResolverError> {
    if atoms.is_empty() {
        return Ok(String::new());
    }
    run_world_command(&["--deselect"], atoms, root)
}

/// Reports what `emerge --depclean` would remove without removing anything.
pub fn emerge_depclean_pretend(root: &Path) -> Result<String, UseResolverError> {
    run_world_command(&["--depclean", "--pretend"], &[], root)
}

fn world_command_argv(args: &[&str], atoms: &[String], root: &Path) -> Vec<String> {
    let mut argv = vec![
        "emerge".to_string(),
        "--root".to_string(),
        root.to_string_lossy().to_string(),
    ];
    argv.extend(args.iter().map(|arg| arg.to_string()));
    argv.extend(atoms.iter().cloned());
    argv
}

fn run_world_command(
    args: &[&str],
    atoms: &[String],
    root: &Path,
) -> Result<String, UseResolverError> {
    let argv = world_command_argv(args, atoms, root);
    let output = exec::capture_command(&emerge_binary(), &argv[1..])?;
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    if !output.status.success() {
        return Err(exec::ExecError::StepFailed {
            step: argv.join(" "),
            status: output.status,
        }
        .into());
    }

    Ok(combined)
}

/// Returns the argv vector that `emerge_select` would construct.
pub fn emerge_select_command_for_test(atoms: &[String], root: &Path) -> Vec<String> {
    world_command_argv(&["--noreplace", "--select"], atoms, root)
}

/// Returns the argv vector that `emerge_deselect` would construct.
pub fn emerge_deselect_command_for_test(atoms: &[String], root: &Path) -> Vec<String> {
    world_command_argv(&["--deselect"], atoms, root)
}

/// Returns the argv vector that `emerge_depclean_pretend` would construct.
pub fn emerge_depclean_pretend_command_for_test(root: &Path) -> Vec<String> {
    world_command_argv(&["--depclean", "--pretend"], &[], root)
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
    planned_total: Option<u32>,
    completed_packages: u32,
}

fn parse_emerge_line(line: &str, state: &mut ParserState) -> EmergeLine {
    if let Some(package) = parse_prefixed_package(line, ">>> Emerging") {
        if let Some((_, total)) = parse_emerge_position(line) {
            state.planned_total = Some(total);
        }
        state.current_package = Some(package.clone());
        return EmergeLine::BuildStart { package };
    }

    if let Some(package) = parse_prefixed_package(line, ">>> Fetching") {
        state.fetch_package = Some(package.clone());
        return EmergeLine::FetchStart { package };
    }

    if line.starts_with(">>> Downloading")
        && let Some(package) = state.current_package.clone()
    {
        state.fetch_package = Some(package.clone());
        return EmergeLine::FetchStart { package };
    }

    if let Some(package) = parse_prefixed_package(line, ">>> Completed installing") {
        state.current_package = Some(package.clone());
        state.completed_packages = state.completed_packages.saturating_add(1);
        let completed = state
            .planned_total
            .map_or(state.completed_packages, |total| {
                state.completed_packages.min(total)
            });
        return EmergeLine::BuildComplete {
            package,
            completed,
            total: state.planned_total,
        };
    }

    if (line.starts_with(">>> Fetch completed") || line.starts_with(">>> Checking"))
        && let Some(package) = state
            .fetch_package
            .clone()
            .or_else(|| state.current_package.clone())
    {
        return EmergeLine::FetchComplete { package };
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

/// Parse Portage's merge-position marker, e.g. `(12 of 133)`.
///
/// The completion counter is still driven by `>>> Completed installing`; the
/// position marker supplies only the denominator because parallel jobs can
/// start out of completion order.
fn parse_emerge_position(line: &str) -> Option<(u32, u32)> {
    let start = line.find('(')? + 1;
    let end = line[start..].find(')')? + start;
    let mut fields = line[start..end].split_whitespace();
    let current = fields.next()?.parse::<u32>().ok()?;
    if fields.next()? != "of" {
        return None;
    }
    let total = fields.next()?.parse::<u32>().ok()?;
    if fields.next().is_some() || current == 0 || total == 0 || current > total {
        return None;
    }
    Some((current, total))
}

fn parse_prefixed_package(line: &str, prefix: &str) -> Option<String> {
    if !line.starts_with(prefix) {
        return None;
    }

    parse_package_token(line)
}

fn parse_package_token(line: &str) -> Option<String> {
    line.split_whitespace().find_map(normalize_package_token)
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
    oneshot: bool,
) -> Vec<String> {
    let mut args = vec![
        "emerge".to_string(),
        "--root".to_string(),
        root.to_string_lossy().to_string(),
        "--jobs".to_string(),
        jobs.to_string(),
    ];
    if oneshot {
        args.push("--oneshot".to_string());
    }
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
        "--update".to_string(),
        "--changed-use".to_string(),
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
mod tests;
