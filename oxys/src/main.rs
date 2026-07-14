use std::{
    error::Error,
    path::{Path, PathBuf},
    process::ExitCode,
};

use clap::{Parser, Subcommand};
use colored::Colorize;
#[cfg(test)]
use oxys::{Package, SystemManifest};
use oxys::{diff::diff_packages, use_resolver::EmergeLine};

mod cli;

use cli::{
    manifest_io::{
        create_plan, effective_portage_config_dir, effective_system_manifest_path, load_manifest,
        load_manifest_optional, persist_manifest, persist_manifest_value,
    },
    output::{fail_on_conflicts, print_changes, print_plan},
};

const LOCAL_MANIFEST: &str = "manifest.toml";
const SYSTEM_MANIFEST: &str = "/etc/oxys/current-manifest.toml";
const DEFAULT_PORTAGE_TREE: &str = "/var/db/repos";
const DEFAULT_PORTAGE_CONFIG_DIR: &str = "/etc/portage";
const DEFAULT_ROOT: &str = "/";
const DEFAULT_TARGET_MOUNT: &str = "/mnt/oxys";
const DEFAULT_PORTAGE_TMPDIR: &str = "/var/tmp/portage";

type Result<T> = std::result::Result<T, Box<dyn Error>>;

#[derive(Parser, Debug)]
#[command(
    name = "oxys",
    version,
    about = "Oxys OS declarative CLI",
    disable_help_subcommand = true
)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// Compile a config into manifest.toml: pass a single .rs file, or omit to
    /// compile the crate in the current directory.
    Compile {
        /// Path to a standalone config .rs file (no crate needed)
        file: Option<PathBuf>,
    },
    /// Show the Portage plan for the local manifest without touching disk
    Check,
    /// Show the Oxys quick manual
    Help,
    /// Diff local manifest.toml against /etc/oxys/current-manifest.toml and show the Portage plan
    Diff,
    /// Apply local manifest.toml changes to the running system
    Apply,
    /// Safely run emerge --sync and emerge -uDN @world with oxys pre-flight checks
    Update {
        /// Skip sync and pre-flight checks, and run the real update immediately.
        #[arg(long)]
        force: bool,
        /// Run sync, pretend, and oxys pre-flight checks without running the real update.
        #[arg(long)]
        dry_run: bool,
        /// Skip emerge --sync, but still run pretend and oxys pre-flight checks.
        #[arg(long)]
        no_sync: bool,
        /// Run sync and pretend, print the parsed update summary, then stop before oxys pre-flight.
        #[arg(long)]
        pretend_only: bool,
        /// Override the number of parallel emerge package jobs for the real update.
        #[arg(long)]
        jobs: Option<usize>,
        /// Pass --keep-going to emerge after oxys pre-flight passes.
        #[arg(long)]
        keep_going: bool,
    },
    /// Install package(s) on this system, or run first-time OS install with `install system`
    Install {
        /// Package atom(s) to add, or `system` for the first-time OS install flow.
        target: Vec<String>,
        /// Skip the interactive prompt. This still wipes the configured target disk.
        #[arg(long)]
        confirm: bool,
        /// Override disk.device from manifest.toml
        #[arg(long)]
        device: Option<String>,
        /// After disk provisioning, copy the live system and install systemd-boot.
        #[arg(long)]
        copy_system: bool,
        /// Source root to copy when --copy-system is enabled.
        #[arg(long, default_value = "/")]
        source_root: PathBuf,
    },
}

fn main() -> ExitCode {
    let cli = Cli::parse();

    let result = match cli.command {
        Commands::Compile { file } => cli::compile::run(file),
        Commands::Check => cmd_check(),
        Commands::Help => cmd_help(),
        Commands::Diff => cmd_diff(),
        Commands::Apply => cli::apply::run(),
        Commands::Update {
            force,
            dry_run,
            no_sync,
            pretend_only,
            jobs,
            keep_going,
        } => cli::update::run(force, dry_run, no_sync, pretend_only, jobs, keep_going),
        Commands::Install {
            target,
            confirm,
            device,
            copy_system,
            source_root,
        } => cli::install::run(target, confirm, device, copy_system, &source_root),
    };

    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("{} {}", "error:".red().bold(), err);
            ExitCode::from(1)
        }
    }
}

fn cmd_check() -> Result<()> {
    let desired = load_manifest(Path::new(LOCAL_MANIFEST))?;
    let plan = create_plan(&desired)?;
    print_plan(&plan);
    fail_on_conflicts(&plan)?;
    println!("{}", "Plan check passed".green().bold());
    Ok(())
}

fn cmd_help() -> Result<()> {
    println!("{}", OXYS_HELP);
    Ok(())
}

const OXYS_HELP: &str = r#"OXYS(1)

NAME
    oxys - declarative Gentoo/Portage system manager

COMMON COMMANDS
    oxys compile [config.rs]
        Compile a Rust system config into manifest.toml.

    oxys check
        Resolve the local manifest and show the Portage plan without changing the system.

    oxys diff
        Compare local manifest.toml with /etc/oxys/current-manifest.toml.

    oxys apply
        Apply local manifest.toml to the running system and persist it as current.

    oxys install <package> [package...]
        Add package atoms to the installed system manifest, plan them, emerge them,
        and persist the updated /etc/oxys/current-manifest.toml.

    oxys install system --device /dev/... --copy-system
        First-time OS install flow. This provisions the selected disk and copies
        the live system into the target.

    oxys update
        Run emerge --sync and a guarded emerge -uDN @world.

SAFETY NOTES
    Bare `oxys install` is intentionally rejected. Use `oxys install <package>`
    for package installs or `oxys install system` for first-time OS installation.

    `oxys install system` can erase disks. Use --confirm only when the selected
    device is definitely correct.

MORE DETAIL
    Use `oxys --help` or `oxys <command> --help` for exact flags.
"#;

fn cmd_diff() -> Result<()> {
    let desired = load_manifest(Path::new(LOCAL_MANIFEST))?;
    let current = load_manifest_optional(Path::new(SYSTEM_MANIFEST))?;

    match current {
        Some(current) => {
            let changes = diff_packages(&current.packages, &desired.packages);
            print_changes(&changes);
        }
        None => {
            println!(
                "{}",
                "Fresh install - all planned packages are new"
                    .yellow()
                    .bold()
            );
        }
    }

    let plan = create_plan(&desired)?;
    print_plan(&plan);
    fail_on_conflicts(&plan)?;
    Ok(())
}

fn print_emerge_event(event: EmergeLine) {
    match event {
        EmergeLine::BuildStart { package } => {
            println!("{} {}", "Building".green().bold(), package.green());
        }
        EmergeLine::BuildProgress { package, line } => match package {
            Some(package) => println!(
                "{} {} {}",
                "Progress".yellow().bold(),
                package.blue().bold(),
                truncate_line(&line)
            ),
            None => println!("{} {}", "Progress".yellow().bold(), truncate_line(&line)),
        },
        EmergeLine::BuildComplete { package } => {
            println!("{} {}", "Built".green().bold(), package.green());
        }
        EmergeLine::FetchStart { package } => {
            println!("{} {}", "Fetching".yellow().bold(), package.yellow());
        }
        EmergeLine::FetchComplete { package } => {
            println!("{} {}", "Fetched".green().bold(), package.green());
        }
        EmergeLine::Error { package, message } => match package {
            Some(package) => println!(
                "{} {} {}",
                "Emerge error".red().bold(),
                package.red().bold(),
                truncate_line(&message).red()
            ),
            None => println!(
                "{} {}",
                "Emerge error".red().bold(),
                truncate_line(&message).red()
            ),
        },
    }
}

fn truncate_line(line: &str) -> String {
    const MAX_LEN: usize = 160;
    if line.chars().count() <= MAX_LEN {
        return line.to_owned();
    }

    let mut truncated = line.chars().take(MAX_LEN - 3).collect::<String>();
    truncated.push_str("...");
    truncated
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn install_packages_parse_as_targets() {
        let cli = Cli::try_parse_from(["oxys", "install", "app-editors/vim", "gui-wm/niri"])
            .expect("install packages should parse");

        match cli.command {
            Commands::Install {
                target,
                confirm,
                device,
                copy_system,
                source_root,
            } => {
                assert_eq!(target, vec!["app-editors/vim", "gui-wm/niri"]);
                assert!(!confirm);
                assert_eq!(device, None);
                assert!(!copy_system);
                assert_eq!(source_root, PathBuf::from(DEFAULT_ROOT));
            }
            other => panic!("unexpected command: {other:?}"),
        }
    }

    #[test]
    fn install_system_parse_keeps_disk_flags() {
        let cli = Cli::try_parse_from([
            "oxys",
            "install",
            "system",
            "--confirm",
            "--device",
            "/dev/nvme0n1",
            "--copy-system",
        ])
        .expect("install system should parse");

        match cli.command {
            Commands::Install {
                target,
                confirm,
                device,
                copy_system,
                ..
            } => {
                assert_eq!(target, vec!["system"]);
                assert!(confirm);
                assert_eq!(device.as_deref(), Some("/dev/nvme0n1"));
                assert!(copy_system);
            }
            other => panic!("unexpected command: {other:?}"),
        }
    }

    #[test]
    fn help_subcommand_parses() {
        let cli = Cli::try_parse_from(["oxys", "help"]).expect("help should parse");

        match cli.command {
            Commands::Help => {}
            other => panic!("unexpected command: {other:?}"),
        }
    }

    #[test]
    fn quick_help_mentions_install_split() {
        assert!(OXYS_HELP.contains("oxys install <package>"));
        assert!(OXYS_HELP.contains("oxys install system"));
        assert!(OXYS_HELP.contains("Bare `oxys install` is intentionally rejected"));
    }

    #[test]
    fn bare_install_is_rejected_by_dispatcher() {
        let err = cli::install::run(Vec::new(), false, None, false, Path::new(DEFAULT_ROOT))
            .expect_err("bare install must be rejected");
        assert!(err.to_string().contains("choose what to install"));
    }

    #[test]
    fn package_install_rejects_disk_flags() {
        let err = cli::install::run(
            vec!["app-editors/vim".to_owned()],
            true,
            None,
            false,
            Path::new(DEFAULT_ROOT),
        )
        .expect_err("package install should reject --confirm");
        assert!(err.to_string().contains("disk install flags"));
    }

    #[test]
    fn add_packages_to_manifest_dedupes_existing_atoms() {
        let mut manifest = SystemManifest {
            packages: vec![Package::new("app-editors/vim")],
            ..SystemManifest::default()
        };

        let added = cli::install::add_packages_to_manifest(
            &mut manifest,
            &[
                "app-editors/vim".to_owned(),
                "gui-wm/niri".to_owned(),
                "www-client/firefox-bin".to_owned(),
            ],
        )
        .expect("package add should succeed");

        assert_eq!(added, vec!["gui-wm/niri", "www-client/firefox-bin"]);
        assert_eq!(
            manifest
                .packages
                .iter()
                .map(|package| package.package.as_str())
                .collect::<Vec<_>>(),
            vec!["app-editors/vim", "gui-wm/niri", "www-client/firefox-bin"]
        );
    }
}
