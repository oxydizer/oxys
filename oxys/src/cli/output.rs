use std::io;

use colored::Colorize;
use oxys::{diff::PackageChange, use_resolver::PortagePlan, Package};

use crate::Result;

pub(crate) fn print_changes(changes: &[PackageChange]) {
    if changes.is_empty() {
        println!("{}", "No manifest package changes.".green().bold());
        return;
    }

    let mut added = Vec::new();
    let mut removed = Vec::new();
    let mut changed = Vec::new();

    for change in changes {
        match (&change.current, &change.desired) {
            (None, Some(pkg)) => added.push(pkg),
            (Some(pkg), None) => removed.push(pkg),
            (Some(current_pkg), Some(desired_pkg)) => {
                changed.push((change.package.as_str(), current_pkg, desired_pkg))
            }
            _ => {}
        }
    }

    if !added.is_empty() {
        println!("{}", "Packages to add:".green().bold());
        for pkg in added {
            println!("  {} {}", "+".green(), pkg_display(pkg).green());
        }
    }
    if !removed.is_empty() {
        println!("{}", "Packages to remove:".red().bold());
        for pkg in removed {
            println!("  {} {}", "-".red(), pkg_display(pkg).red());
        }
    }
    if !changed.is_empty() {
        println!("{}", "Packages to change:".yellow().bold());
        for (package, current_pkg, desired_pkg) in changed {
            println!("  {} {}", "~".yellow(), package.yellow().bold());
            println!("    from: {}", pkg_display(current_pkg).dimmed());
            println!("    to  : {}", pkg_display(desired_pkg).yellow());
        }
    }
}

fn pkg_display(pkg: &Package) -> String {
    let mut out = pkg.package.clone();
    if let Some(version) = &pkg.version {
        out.push_str(&format!(" @{version}"));
    }
    if !pkg.use_flags.is_empty() {
        out.push_str(&format!(" [{}]", pkg.use_flags.join(" ")));
    }
    if !pkg.keywords.is_empty() {
        out.push_str(&format!(" keywords={}", pkg.keywords.join(",")));
    }
    if !pkg.accept_licenses.is_empty() {
        out.push_str(&format!(" licenses={}", pkg.accept_licenses.join(",")));
    }
    if pkg.binary {
        out.push_str(" binary");
    }
    out
}

pub(crate) fn print_plan(plan: &PortagePlan) {
    println!("{}", "Portage plan".yellow().bold());
    if plan.targets.is_empty() {
        println!("  {}", "No packages selected".yellow());
    } else {
        println!("{}", "Targets:".yellow().bold());
        for target in &plan.targets {
            println!("  {}", target.green());
        }
    }
    println!("{}", "USE flags:".yellow().bold());
    let mut package_use = plan.resolution.package_use.iter().collect::<Vec<_>>();
    package_use.sort_by(|left, right| left.0.cmp(right.0));
    if package_use.is_empty() {
        println!("  {}", "(none)".yellow());
    } else {
        for (package, flags) in package_use {
            println!("  {} {}", package.blue().bold(), flags.join(" "));
        }
    }
    println!("{}", "Global USE:".yellow().bold());
    if plan.resolution.global_use.is_empty() {
        println!("  {}", "(none)".yellow());
    } else {
        println!("  {}", plan.resolution.global_use.join(" "));
    }
    println!("{}", "Keywords:".yellow().bold());
    if plan.resolution.accept_keywords.is_empty() {
        println!("  {}", "(none)".yellow());
    } else {
        for keyword in &plan.resolution.accept_keywords {
            println!("  {}", keyword);
        }
    }
    if plan.resolution.warnings.is_empty() {
        println!("{}", "Warnings: none".yellow().bold());
    } else {
        println!("{}", "Warnings:".yellow().bold());
        for warning in &plan.resolution.warnings {
            println!(
                "  {} {}",
                warning.package.yellow().bold(),
                warning.message.yellow()
            );
        }
    }
    if plan.resolution.conflicts.is_empty() {
        println!("{}", "Conflicts: none".green().bold());
    } else {
        println!("{}", "Conflicts:".red().bold());
        for conflict in &plan.resolution.conflicts {
            println!("  {} {}", conflict.flag.red().bold(), conflict.reason.red());
            println!("  {}", conflict.packages.join(", ").red());
        }
    }
}

pub(crate) fn fail_on_conflicts(plan: &PortagePlan) -> Result<()> {
    if plan.resolution.conflicts.is_empty() {
        Ok(())
    } else {
        Err(io::Error::other("hard conflicts detected; refusing to continue").into())
    }
}
