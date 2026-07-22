use std::{
    error::Error,
    path::{Path, PathBuf},
    process::ExitCode,
};

use clap::{Parser, Subcommand};
use colored::Colorize;
use oxys::{diff::diff_packages, use_resolver::EmergeLine};

mod cli;

use cli::{
    manifest_io::{create_plan, load_manifest, load_manifest_optional, persist_manifest_value},
    output::{fail_on_conflicts, print_changes, print_plan},
};

const LOCAL_MANIFEST: &str = "manifest.toml";
const SYSTEM_MANIFEST: &str = "/etc/oxys/current-manifest.toml";
const SYSTEM_CONFIG: &str = "/etc/oxys/config.fe2o3";
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
    /// Print image-builder environment values derived from a compiled manifest.
    GraphicsBuildPolicy {
        /// Generated manifest to resolve.
        #[arg(default_value = "manifest.toml")]
        manifest: PathBuf,
    },
    /// Show the Portage plan for the local manifest without touching disk
    Check,
    /// Show the Oxys quick manual
    Help,
    /// Show a welcome screen with quick commands and documentation links
    Welcome,
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
    /// Build, inspect, verify, or directly install a local .oxys artifact.
    Package {
        #[command(subcommand)]
        command: PackageCommands,
    },
    /// Remove a package previously installed from a .oxys artifact.
    Remove {
        /// Installed package identity in category/PF form.
        package: String,
        /// Alternate target root (defaults to the running system).
        #[arg(long, default_value = "/")]
        root: PathBuf,
    },
}

#[derive(Subcommand, Debug)]
enum PackageCommands {
    /// Capture an installed Portage package and its complete VDB entry.
    Build {
        /// Package atom in category/package form.
        atom: String,
        /// Clean reference root containing the installed package.
        #[arg(long, default_value = "/")]
        root: PathBuf,
        /// Exact artifact path to create. By default the package identity,
        /// architecture, and CPU baseline determine the filename.
        #[arg(short, long)]
        output: Option<PathBuf>,
    },
    /// Print verified artifact metadata.
    Inspect { artifact: PathBuf },
    /// Fully verify framing, metadata, file table, tar, and hashes.
    Verify { artifact: PathBuf },
    /// Install an artifact and its captured Portage VDB entry.
    Install {
        artifact: PathBuf,
        /// Alternate target root (defaults to the running system).
        #[arg(long, default_value = "/")]
        root: PathBuf,
    },
}

fn main() -> ExitCode {
    let cli = Cli::parse();

    let result = match cli.command {
        Commands::Compile { file } => cli::compile::run(file),
        Commands::GraphicsBuildPolicy { manifest } => cmd_graphics_build_policy(&manifest),
        Commands::Check => cmd_check(),
        Commands::Help => cmd_help(),
        Commands::Welcome => cli::welcome::run(),
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
        Commands::Package { command } => match command {
            PackageCommands::Build { atom, root, output } => {
                cli::package::build(&root, &atom, output.as_deref())
            }
            PackageCommands::Inspect { artifact } => cli::package::inspect(&artifact),
            PackageCommands::Verify { artifact } => cli::package::verify(&artifact),
            PackageCommands::Install { artifact, root } => cli::package::install(&artifact, &root),
        },
        Commands::Remove { package, root } => cli::package::remove(&package, &root),
    };

    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("{} {}", "error:".red().bold(), err);
            ExitCode::from(1)
        }
    }
}

fn cmd_graphics_build_policy(path: &Path) -> Result<()> {
    let manifest = load_manifest(path)?;
    let resolved = manifest.resolved_graphics()?;
    let video_cards = resolved.mesa_build_values();
    let drm_drivers = resolved.drm_build_values();
    if video_cards.is_empty() {
        return Err(format!(
            "{} resolves no Mesa VIDEO_CARDS; compile the manifest on the target hardware or select hardware.graphics.mesa.video_cards explicitly",
            path.display()
        )
        .into());
    }
    if drm_drivers.is_empty() {
        return Err(format!(
            "{} resolves no kernel DRM drivers; select hardware.graphics.drm.drivers explicitly",
            path.display()
        )
        .into());
    }
    println!("OXYS_VIDEO_CARDS='{}'", video_cards.join(" "));
    println!("OXYS_DRM_DRIVERS='{}'", drm_drivers.join(" "));
    Ok(())
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

    oxys graphics-build-policy [manifest.toml]
        Export Mesa and kernel build inputs from resolved graphics policy.

    oxys diff
        Compare local manifest.toml with /etc/oxys/current-manifest.toml.

    oxys apply
        Apply local manifest.toml to the running system and persist it as current.

    oxys install <package> [package...]
        Add package atoms to the declarative source at /etc/oxys/config.fe2o3,
        recompile it, then emerge the resulting plan and persist the applied state
        to /etc/oxys/current-manifest.toml. The .fe2o3 source stays authoritative.

    oxys install system --device /dev/... --copy-system
        First-time OS install flow. This provisions the selected disk and copies
        the live system into the target.

    oxys update
        Run emerge --sync and a guarded emerge -uDN @world.

    oxys package build gui-apps/wl-clipboard --root /
        Capture package files plus the complete Portage VDB entry, naming the
        artifact from its package identity, architecture, and CPU baseline.

    oxys package verify <artifact.oxys>
        Verify the framed container, canonical file table, tar, and hashes.

    oxys package install <artifact.oxys> [--root /]
        Install a local artifact, write its VDB entry, cache it, and write a receipt.

    oxys remove <category/PF> [--root /]
        Safely remove an artifact-installed package after verifying all file hashes.

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
        EmergeLine::BuildComplete {
            package,
            completed,
            total,
        } => {
            let progress = total
                .map(|total| format!(" ({completed}/{total})"))
                .unwrap_or_default();
            println!("{} {}{}", "Built".green().bold(), package.green(), progress);
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
    fn welcome_subcommand_parses() {
        let cli = Cli::try_parse_from(["oxys", "welcome"]).expect("welcome should parse");

        match cli.command {
            Commands::Welcome => {}
            other => panic!("unexpected command: {other:?}"),
        }
    }

    #[test]
    fn graphics_build_policy_subcommand_parses_manifest_path() {
        let cli = Cli::try_parse_from([
            "oxys",
            "graphics-build-policy",
            "/tmp/desktop-manifest.toml",
        ])
        .expect("graphics build policy command should parse");

        match cli.command {
            Commands::GraphicsBuildPolicy { manifest } => {
                assert_eq!(manifest, PathBuf::from("/tmp/desktop-manifest.toml"));
            }
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
    fn package_build_parses_reference_root_and_output() {
        let cli = Cli::try_parse_from([
            "oxys",
            "package",
            "build",
            "gui-apps/wl-clipboard",
            "--root",
            "/reference",
            "--output",
            "wl.oxys",
        ])
        .expect("package build should parse");

        match cli.command {
            Commands::Package {
                command: PackageCommands::Build { atom, root, output },
            } => {
                assert_eq!(atom, "gui-apps/wl-clipboard");
                assert_eq!(root, PathBuf::from("/reference"));
                assert_eq!(output, Some(PathBuf::from("wl.oxys")));
            }
            other => panic!("unexpected command: {other:?}"),
        }
    }

    #[test]
    fn package_build_has_no_generic_default_filename() {
        let cli = Cli::try_parse_from(["oxys", "package", "build", "gui-apps/wl-clipboard"])
            .expect("package build should parse");

        match cli.command {
            Commands::Package {
                command: PackageCommands::Build { output, .. },
            } => assert_eq!(output, None),
            other => panic!("unexpected command: {other:?}"),
        }
    }

    #[test]
    fn package_remove_parses_exact_pf_and_root() {
        let cli = Cli::try_parse_from([
            "oxys",
            "remove",
            "gui-apps/wl-clipboard-2.2.1",
            "--root",
            "/target",
        ])
        .expect("package remove should parse");

        match cli.command {
            Commands::Remove { package, root } => {
                assert_eq!(package, "gui-apps/wl-clipboard-2.2.1");
                assert_eq!(root, PathBuf::from("/target"));
            }
            other => panic!("unexpected command: {other:?}"),
        }
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
}
