use std::{
    error::Error,
    fs, io,
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

use chrono::{DateTime, Utc};
use colored::Colorize;
use oxys::{
    parse_generated_manifest_toml,
    use_resolver::{
        build_world_update_plan, parse_pretend_world_update, plan_update_preflight, PortagePlan,
        PretendOperation, PretendPackage, PretendPackageSource, WorldUpdatePlan,
        WorldUpdateWarning,
    },
    util::default_use_resolver_cache_dir,
    SystemManifest,
};
use serde::Serialize;

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
    crate::print_plan(&plan);
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
        crate::fail_on_conflicts(&plan)?;
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

#[derive(Debug, Serialize)]
struct UpdateReport {
    started_at: String,
    sync_ran: bool,
    dry_run: bool,
    force: bool,
    real_update_status: String,
    parsed_packages: Vec<UpdateReportPackage>,
    wrapper_warnings: Vec<UpdateReportWarning>,
    resolver_warnings: Vec<UpdateReportResolverWarning>,
    resolver_conflicts: Vec<UpdateReportConflict>,
}
#[derive(Debug, Serialize)]
struct UpdateReportPackage {
    package: String,
    version: String,
    source: String,
    operation: String,
}
#[derive(Debug, Serialize)]
struct UpdateReportWarning {
    kind: String,
    package: String,
}
#[derive(Debug, Serialize)]
struct UpdateReportResolverWarning {
    package: String,
    message: String,
}
#[derive(Debug, Serialize)]
struct UpdateReportConflict {
    packages: Vec<String>,
    flag: String,
    reason: String,
}

/// Writes (or rewrites) the update report. When `existing_path` is set, that
/// file is updated in place so a real merge can record preflight state before
/// emerge starts and the final status after it exits.
fn write_update_report(
    existing_path: Option<&Path>,
    started_at: DateTime<Utc>,
    dry_run: bool,
    force: bool,
    update_plan: &WorldUpdatePlan,
    preflight_plan: Option<&PortagePlan>,
    real_update_status: &str,
) -> Option<PathBuf> {
    let Some(report_dir) = update_report_dir() else {
        return None;
    };
    let report = UpdateReport {
        started_at: started_at.to_rfc3339(),
        sync_ran: update_plan.sync_ran,
        dry_run,
        force,
        real_update_status: real_update_status.to_owned(),
        parsed_packages: update_plan.packages.iter().map(report_package).collect(),
        wrapper_warnings: update_plan
            .warnings
            .iter()
            .map(report_wrapper_warning)
            .collect(),
        resolver_warnings: preflight_plan
            .map(|plan| {
                plan.resolution
                    .warnings
                    .iter()
                    .map(|warning| UpdateReportResolverWarning {
                        package: warning.package.clone(),
                        message: warning.message.clone(),
                    })
                    .collect()
            })
            .unwrap_or_default(),
        resolver_conflicts: preflight_plan
            .map(|plan| {
                plan.resolution
                    .conflicts
                    .iter()
                    .map(|conflict| UpdateReportConflict {
                        packages: conflict.packages.clone(),
                        flag: conflict.flag.clone(),
                        reason: conflict.reason.clone(),
                    })
                    .collect()
            })
            .unwrap_or_default(),
    };
    if let Err(err) = fs::create_dir_all(&report_dir) {
        eprintln!(
            "{} failed to create update report directory {}: {}",
            "warning:".yellow().bold(),
            report_dir.display(),
            err
        );
        return None;
    }
    let path = existing_path.map(Path::to_path_buf).unwrap_or_else(|| {
        report_dir.join(format!(
            "update-{}.toml",
            started_at.format("%Y%m%d-%H%M%S")
        ))
    });
    let rendered = match toml::to_string_pretty(&report) {
        Ok(rendered) => rendered,
        Err(err) => {
            eprintln!(
                "{} failed to render update report: {}",
                "warning:".yellow().bold(),
                err
            );
            return None;
        }
    };
    if let Err(err) = fs::write(&path, rendered) {
        eprintln!(
            "{} failed to write update report {}: {}",
            "warning:".yellow().bold(),
            path.display(),
            err
        );
        return None;
    }
    println!(
        "{} {}",
        if existing_path.is_some() {
            "Updated update report:"
        } else {
            "Saved update report:"
        }
        .green()
        .bold(),
        path.display().to_string().green()
    );
    Some(path)
}

fn update_report_dir() -> Option<PathBuf> {
    std::env::var_os("OXYS_UPDATE_LOG_DIR")
        .map(PathBuf::from)
        .and_then(|path| (!path.as_os_str().is_empty()).then_some(path))
        .or_else(|| Some(PathBuf::from("/var/log/oxys")))
}

fn report_package(package: &PretendPackage) -> UpdateReportPackage {
    UpdateReportPackage {
        package: package.package.clone(),
        version: package.version.clone(),
        source: match package.source {
            PretendPackageSource::Ebuild => "ebuild",
            PretendPackageSource::Binary => "binary",
        }
        .to_owned(),
        operation: match package.operation {
            PretendOperation::Merge => "merge",
            PretendOperation::Uninstall => "uninstall",
        }
        .to_owned(),
    }
}

fn report_wrapper_warning(warning: &WorldUpdateWarning) -> UpdateReportWarning {
    match warning {
        WorldUpdateWarning::NotInManifest { package } => UpdateReportWarning {
            kind: "not_in_manifest".to_owned(),
            package: package.clone(),
        },
        WorldUpdateWarning::RemovedByUpdate { package } => UpdateReportWarning {
            kind: "removed_by_update".to_owned(),
            package: package.clone(),
        },
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
        crate::print_command_output(&output.stdout, &output.stderr);
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
