use std::{
    error::Error,
    fs, io,
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

use chrono::Utc;
use colored::Colorize;
use oxys::{
    SystemManifest, parse_generated_manifest_toml,
    use_resolver::{
        PretendOperation, PretendPackageSource, WorldUpdatePlan, WorldUpdateWarning,
        build_world_update_plan, parse_pretend_world_update, plan_update_preflight,
    },
    util::default_use_resolver_cache_dir,
};

use super::output::{fail_on_conflicts, print_plan};
use super::update_report::write_update_report;

const DEFAULT_PORTAGE_TREE: &str = "/var/db/repos";
const SYSTEM_MANIFEST: &str = "/etc/oxys/current-manifest.toml";

type Result<T> = std::result::Result<T, Box<dyn Error>>;

pub(crate) fn run(
    force: bool,
    dry_run: bool,
    no_sync: bool,
    pretend_only: bool,
    jobs: Option<usize>,
    keep_going: bool,
) -> Result<()> {
    if force && (dry_run || no_sync || pretend_only) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "--force cannot be combined with --dry-run, --no-sync, or --pretend-only",
        )
        .into());
    }
    if dry_run && pretend_only {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "--dry-run and --pretend-only are separate modes",
        )
        .into());
    }

    let started_at = Utc::now();

    if force {
        println!(
            "{}",
            "Skipping update pre-flight checks because --force was supplied"
                .yellow()
                .bold()
        );
        return run_real_world_update(jobs, keep_going);
    }

    let sync_ran = !no_sync;
    if sync_ran {
        println!("{}", "Syncing Portage tree".cyan().bold());
        run_emerge_passthrough(&["--sync".to_owned()])?;
    } else {
        println!("{}", "Skipping Portage sync".yellow().bold());
    }

    println!("{}", "Calculating world update plan".cyan().bold());
    let pretend_output = run_emerge_capture(&[
        "-uDNp".to_owned(),
        "--columns".to_owned(),
        "--color=n".to_owned(),
        "@world".to_owned(),
    ])?;
    let pretend_packages = parse_pretend_world_update(&pretend_output).map_err(|err| {
        io::Error::other(format!(
            "{err}. Refusing to run emerge -uDN @world because the update plan could not be verified; rerun with --force to bypass oxys checks."
        ))
    })?;

    if pretend_packages.is_empty() {
        let update_plan = build_world_update_plan(None, pretend_packages, sync_ran);
        println!("{}", "No world updates proposed by Portage".green().bold());
        write_update_report(
            None,
            started_at,
            dry_run,
            force,
            &update_plan,
            None,
            "no_updates",
        );
        return Ok(());
    }

    if pretend_only {
        let update_plan = build_world_update_plan(None, pretend_packages, sync_ran);
        print_world_update_plan(&update_plan);
        write_update_report(
            None,
            started_at,
            dry_run,
            force,
            &update_plan,
            None,
            "pretend_only",
        );
        return Ok(());
    }

    let current_path = effective_system_manifest_path();
    let current_manifest = load_manifest_optional(&current_path)?.ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::NotFound,
            format!(
                "current oxys manifest not found at {}; cannot verify binary/source update safety. Rerun with --force to bypass oxys checks.",
                current_path.display()
            ),
        )
    })?;

    let update_plan = build_world_update_plan(Some(&current_manifest), pretend_packages, sync_ran);
    print_world_update_plan(&update_plan);

    let plan = plan_update_preflight(
        &current_manifest,
        &update_plan.packages,
        &effective_portage_tree(),
        &default_use_resolver_cache_dir(),
    )?;
    print_plan(&plan);
    if !plan.resolution.conflicts.is_empty() {
        write_update_report(
            None,
            started_at,
            dry_run,
            force,
            &update_plan,
            Some(&plan),
            "blocked_by_conflicts",
        );
        fail_on_conflicts(&plan)?;
    }

    if dry_run {
        println!(
            "{}",
            "Update pre-flight check passed; dry-run requested, not running emerge -uDN @world"
                .green()
                .bold()
        );
        write_update_report(
            None,
            started_at,
            dry_run,
            force,
            &update_plan,
            Some(&plan),
            "skipped_dry_run",
        );
        return Ok(());
    }

    println!("{}", "Update pre-flight check passed".green().bold());
    let report_path = write_update_report(
        None,
        started_at,
        dry_run,
        force,
        &update_plan,
        Some(&plan),
        "starting",
    );
    let result = run_real_world_update(jobs, keep_going);
    write_update_report(
        report_path.as_deref(),
        started_at,
        dry_run,
        force,
        &update_plan,
        Some(&plan),
        if result.is_ok() {
            "completed"
        } else {
            "failed"
        },
    );
    result
}

fn run_real_world_update(jobs: Option<usize>, keep_going: bool) -> Result<()> {
    println!("{}", "Running emerge -uDN @world".yellow().bold());
    let mut args = Vec::new();
    if let Some(jobs) = jobs {
        args.push("--jobs".to_owned());
        args.push(jobs.to_string());
    }
    if keep_going {
        args.push("--keep-going".to_owned());
    }
    args.push("-uDN".to_owned());
    args.push("@world".to_owned());
    run_emerge_passthrough(&args)
}

fn print_world_update_plan(plan: &WorldUpdatePlan) {
    let merges = plan
        .packages
        .iter()
        .filter(|package| package.operation == PretendOperation::Merge)
        .count();
    let uninstalls = plan
        .packages
        .iter()
        .filter(|package| package.operation == PretendOperation::Uninstall)
        .count();
    let source_builds = plan
        .packages
        .iter()
        .filter(|package| {
            package.operation == PretendOperation::Merge
                && package.source == PretendPackageSource::Ebuild
        })
        .count();
    let binary_packages = plan
        .packages
        .iter()
        .filter(|package| {
            package.operation == PretendOperation::Merge
                && package.source == PretendPackageSource::Binary
        })
        .count();

    println!("{}", "World update".yellow().bold());
    println!(
        "  {} {}",
        "sync:".yellow().bold(),
        if plan.sync_ran { "ran" } else { "skipped" }
    );
    println!("  {} {}", "proposed merges:".yellow().bold(), merges);
    println!(
        "  {} {}",
        "proposed uninstalls:".yellow().bold(),
        uninstalls
    );
    println!("  {} {}", "source builds:".yellow().bold(), source_builds);
    println!(
        "  {} {}",
        "binary packages:".yellow().bold(),
        binary_packages
    );

    if !plan.packages.is_empty() {
        println!("{}", "Important changes:".yellow().bold());
        for package in &plan.packages {
            let operation = match package.operation {
                PretendOperation::Merge => "merge",
                PretendOperation::Uninstall => "uninstall",
            };
            let source = match package.source {
                PretendPackageSource::Ebuild => "source",
                PretendPackageSource::Binary => "binary",
            };
            println!(
                "  {} {} {} {}",
                package.package.green(),
                package.version.yellow(),
                operation,
                source
            );
        }
    }

    if plan.warnings.is_empty() {
        println!("{}", "Update warnings: none".green().bold());
    } else {
        println!("{}", "Update warnings:".yellow().bold());
        for warning in &plan.warnings {
            match warning {
                WorldUpdateWarning::NotInManifest { package } => println!(
                    "  {} {}",
                    package.yellow().bold(),
                    "is in the Portage world update plan but not in the Oxys manifest".yellow()
                ),
                WorldUpdateWarning::RemovedByUpdate { package } => println!(
                    "  {} {}",
                    package.yellow().bold(),
                    "would be uninstalled by the Portage world update plan".yellow()
                ),
            }
        }
    }
}

fn effective_portage_tree() -> PathBuf {
    std::env::var_os("OXYS_PORTAGE_TREE")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(DEFAULT_PORTAGE_TREE))
}
fn effective_system_manifest_path() -> PathBuf {
    std::env::var_os("OXYS_SYSTEM_MANIFEST")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(SYSTEM_MANIFEST))
}
fn emerge_binary() -> String {
    std::env::var("OXYS_EMERGE").unwrap_or_else(|_| "emerge".to_owned())
}

fn run_emerge_passthrough(args: &[String]) -> Result<()> {
    let status = Command::new(emerge_binary())
        .args(args)
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .status()?;
    if status.success() {
        Ok(())
    } else {
        Err(io::Error::other(format!("emerge {} failed: {status}", args.join(" "))).into())
    }
}

fn run_emerge_capture(args: &[String]) -> Result<String> {
    let output = Command::new(emerge_binary()).args(args).output()?;
    if !output.status.success() {
        if !output.stdout.is_empty() {
            print!("{}", String::from_utf8_lossy(&output.stdout));
        }
        if !output.stderr.is_empty() {
            eprint!("{}", String::from_utf8_lossy(&output.stderr));
        }
        return Err(io::Error::other(format!(
            "emerge {} failed: {}",
            args.join(" "),
            output.status
        ))
        .into());
    }
    Ok(format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    ))
}

fn load_manifest_optional(path: &Path) -> Result<Option<SystemManifest>> {
    if !path.exists() {
        return Ok(None);
    }
    let text = fs::read_to_string(path)?;
    parse_generated_manifest_toml(&text).map(Some).map_err(|err| io::Error::new(io::ErrorKind::InvalidData, format!("failed to parse {} as generated oxys manifest: {err}. Regenerate it with the real oxys crate", path.display())).into())
}
