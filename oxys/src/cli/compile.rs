use std::{
    io,
    path::{Path, PathBuf},
    process::Command,
};

use colored::Colorize;

use super::manifest_io::load_manifest;
use crate::{LOCAL_MANIFEST, Result};

pub(crate) fn run(file: Option<PathBuf>) -> Result<()> {
    match file {
        Some(path) => compile_file(&path),
        None => compile_cwd(),
    }
}

fn compile_cwd() -> Result<()> {
    let cwd = std::env::current_dir()?;
    if !cwd.join("src/main.rs").exists() {
        return Err(io::Error::new(io::ErrorKind::NotFound,
            "src/main.rs not found in current directory (pass a config .rs file to compile a single file instead)").into());
    }
    println!("{}", "Compiling config crate...".cyan());
    let build = Command::new("cargo").arg("build").output()?;
    if !build.status.success() {
        eprintln!("{}", "Compilation failed".red().bold());
        print_command_output(&build.stdout, &build.stderr);
        return Err(io::Error::other("config compilation failed").into());
    }
    println!("{}", "Compilation succeeded".green().bold());
    println!(
        "{}",
        "Executing project binary to generate manifest.toml...".cyan()
    );
    let run = Command::new("cargo").arg("run").arg("--quiet").output()?;
    if !run.status.success() {
        eprintln!("{}", "Compiled binary execution failed".red().bold());
        print_command_output(&run.stdout, &run.stderr);
        return Err(io::Error::other("project execution failed").into());
    }
    report_manifest(&cwd.join(LOCAL_MANIFEST))
}

fn compile_file(file: &Path) -> Result<()> {
    println!(
        "{} {}",
        "Compiling config".cyan(),
        file.display().to_string().cyan()
    );
    match oxys::compile::compile_config_file(
        file,
        &oxys::compile::oxys_crate_path(),
        &std::env::current_dir()?,
    ) {
        Ok(outcome) => {
            println!("{}", "Compilation succeeded".green().bold());
            for notice in &outcome.notices {
                println!("{}", notice.yellow());
            }
            if let Ok(manifest) = load_manifest(&outcome.manifest_path) {
                print_defaults_report(&manifest);
            }
            println!(
                "{} {}",
                "Success:".green().bold(),
                outcome.manifest_path.display().to_string().green()
            );
            Ok(())
        }
        Err(err) => {
            eprintln!("{}", "Compilation failed".red().bold());
            if !err.output.is_empty() {
                eprint!("{}", err.output);
            }
            Err(io::Error::other(err.to_string()).into())
        }
    }
}

fn report_manifest(manifest: &Path) -> Result<()> {
    if !manifest.exists() {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            "compilation completed but manifest.toml was not created",
        )
        .into());
    }
    let parsed = load_manifest(manifest)?;
    print_defaults_report(&parsed);
    println!(
        "{} {}",
        "Success:".green().bold(),
        manifest.display().to_string().green()
    );
    Ok(())
}

/// Show which notable settings are running on built-in defaults, so nothing
/// opinionated is applied invisibly.
fn print_defaults_report(manifest: &oxys::SystemManifest) {
    let settings = oxys::defaults_report::defaulted_settings(manifest);
    if settings.is_empty() {
        return;
    }
    println!("{}", "Defaults in effect:".cyan());
    for line in oxys::defaults_report::render_defaults_report(&settings) {
        println!("  {}", line.dimmed());
    }
}

fn print_command_output(stdout: &[u8], stderr: &[u8]) {
    if !stdout.is_empty() {
        print!("{}", String::from_utf8_lossy(stdout));
    }
    if !stderr.is_empty() {
        eprint!("{}", String::from_utf8_lossy(stderr));
    }
}
