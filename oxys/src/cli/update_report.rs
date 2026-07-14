use std::{
    fs,
    path::{Path, PathBuf},
};

use chrono::{DateTime, Utc};
use colored::Colorize;
use oxys::use_resolver::{
    PortagePlan, PretendOperation, PretendPackage, PretendPackageSource, WorldUpdatePlan,
    WorldUpdateWarning,
};
use serde::Serialize;

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

/// Writes a new report, or updates the pre-merge report after emerge exits.
pub(super) fn write_update_report(
    existing_path: Option<&Path>,
    started_at: DateTime<Utc>,
    dry_run: bool,
    force: bool,
    update_plan: &WorldUpdatePlan,
    preflight_plan: Option<&PortagePlan>,
    status: &str,
) -> Option<PathBuf> {
    let report = UpdateReport {
        started_at: started_at.to_rfc3339(),
        sync_ran: update_plan.sync_ran,
        dry_run,
        force,
        real_update_status: status.to_owned(),
        parsed_packages: update_plan.packages.iter().map(report_package).collect(),
        wrapper_warnings: update_plan.warnings.iter().map(report_warning).collect(),
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
    let report_dir = update_report_dir();
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
    let action = if existing_path.is_some() {
        "Updated update report:"
    } else {
        "Saved update report:"
    };
    println!(
        "{} {}",
        action.green().bold(),
        path.display().to_string().green()
    );
    Some(path)
}

fn update_report_dir() -> PathBuf {
    std::env::var_os("OXYS_UPDATE_LOG_DIR")
        .map(PathBuf::from)
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or_else(|| PathBuf::from("/var/log/oxys"))
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

fn report_warning(warning: &WorldUpdateWarning) -> UpdateReportWarning {
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
