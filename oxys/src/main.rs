use std::{
    error::Error,
    fs, io,
    path::{Path, PathBuf},
    process::{Command, ExitCode},
};

use clap::{Parser, Subcommand};
use colored::Colorize;
use oxys::{
    apply_disk_plan, apply_system_install_plan, compile,
    diff::{diff_packages, PackageChange},
    manifest_to_toml, parse_generated_manifest_toml, plan_disk, plan_system_install, preflight,
    runtime::sync_runtime_config,
    use_resolver::{apply_portage_plan, plan_portage, EmergeLine, PortagePlan},
    Package, ProvisionEvent, SystemInstallEvent, SystemManifest,
};

mod cli;

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
        Commands::Compile { file } => cmd_compile(file),
        Commands::Check => cmd_check(),
        Commands::Help => cmd_help(),
        Commands::Diff => cmd_diff(),
        Commands::Apply => cmd_apply(),
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
        } => cmd_install(target, confirm, device, copy_system, &source_root),
    };

    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("{} {}", "error:".red().bold(), err);
            ExitCode::from(1)
        }
    }
}

fn cmd_compile(file: Option<PathBuf>) -> Result<()> {
    match file {
        Some(path) => cmd_compile_file(&path),
        None => cmd_compile_cwd(),
    }
}

fn cmd_compile_cwd() -> Result<()> {
    let cwd = std::env::current_dir()?;
    let main_rs = cwd.join("src/main.rs");
    if !main_rs.exists() {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            "src/main.rs not found in current directory (pass a config .rs file to compile a single file instead)",
        )
        .into());
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

/// Compile a single standalone config `.rs` file (no crate needed).
///
/// Thin CLI wrapper over [`compile::compile_config_file`], which scaffolds a
/// persistent crate that depends on the oxys crate, builds the config, and runs
/// it so `manifest.toml` lands in the caller's working directory.
fn cmd_compile_file(file: &Path) -> Result<()> {
    let user_cwd = std::env::current_dir()?;
    println!(
        "{} {}",
        "Compiling config".cyan(),
        file.display().to_string().cyan()
    );
    match compile::compile_config_file(file, &compile::oxys_crate_path(), &user_cwd) {
        Ok(manifest) => {
            println!("{}", "Compilation succeeded".green().bold());
            println!(
                "{} {}",
                "Success:".green().bold(),
                manifest.display().to_string().green()
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
    if manifest.exists() {
        let _ = load_manifest(manifest)?;
        println!(
            "{} {}",
            "Success:".green().bold(),
            manifest.display().to_string().green()
        );
        Ok(())
    } else {
        Err(io::Error::new(
            io::ErrorKind::NotFound,
            "compilation completed but manifest.toml was not created",
        )
        .into())
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

fn cmd_apply() -> Result<()> {
    let desired_path = Path::new(LOCAL_MANIFEST);
    let desired = load_manifest(desired_path)?;

    println!("{}", "Applying manifest to running system".cyan().bold());

    let plan = create_plan(&desired)?;
    print_plan(&plan);
    fail_on_conflicts(&plan)?;

    println!("{}", "Applying Portage plan".yellow().bold());
    let mut stream = apply_portage_plan(
        &plan,
        Path::new(DEFAULT_PORTAGE_CONFIG_DIR),
        Path::new(DEFAULT_ROOT),
        Path::new(DEFAULT_PORTAGE_TMPDIR),
        plan.manifest.compiler.emerge_jobs,
    )?;

    for event in &mut stream {
        print_emerge_event(event);
    }
    stream.wait()?;

    let runtime_outcome = sync_runtime_config(&desired, Path::new(DEFAULT_ROOT))?;
    if runtime_outcome.prime_offload_configured {
        println!(
            "{}",
            "Configured NVIDIA PRIME render offload runtime files"
                .green()
                .bold()
        );
    }

    persist_manifest(desired_path, Path::new(SYSTEM_MANIFEST))?;
    println!("{}", "Apply completed successfully".green().bold());
    Ok(())
}

fn cmd_install(
    target: Vec<String>,
    confirm: bool,
    device: Option<String>,
    copy_system: bool,
    source_root: &Path,
) -> Result<()> {
    let Some(first) = target.first() else {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "choose what to install: `oxys install <package>` or `oxys install system --copy-system --device /dev/...`",
        )
        .into());
    };

    if first == "system" {
        if target.len() > 1 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "`oxys install system` does not accept package atoms; use `oxys install <package>` on an installed system",
            )
            .into());
        }
        return cmd_install_system(confirm, device, copy_system, source_root);
    }

    if confirm || device.is_some() || copy_system || source_root != Path::new(DEFAULT_ROOT) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "disk install flags are only valid with `oxys install system`",
        )
        .into());
    }

    cmd_install_packages(&target)
}

fn cmd_install_packages(packages: &[String]) -> Result<()> {
    let manifest_path = effective_system_manifest_path();
    let mut desired = load_manifest_optional(&manifest_path)?.ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::NotFound,
            format!(
                "current oxys manifest not found at {}; package installs require an installed Oxys system. Use `oxys install system` for first-time OS installation.",
                manifest_path.display()
            ),
        )
    })?;

    let added = add_packages_to_manifest(&mut desired, packages)?;
    if added.is_empty() {
        println!(
            "{}",
            "All requested packages are already in the manifest"
                .green()
                .bold()
        );
        return Ok(());
    }

    println!(
        "{}",
        "Installing package(s) on running system".cyan().bold()
    );
    println!("{}", "Packages to add:".green().bold());
    for package in &added {
        println!("  {} {}", "+".green(), package.green());
    }

    let plan = create_plan(&desired)?;
    print_plan(&plan);
    fail_on_conflicts(&plan)?;

    println!("{}", "Applying Portage plan".yellow().bold());
    let mut stream = apply_portage_plan(
        &plan,
        Path::new(DEFAULT_PORTAGE_CONFIG_DIR),
        Path::new(DEFAULT_ROOT),
        Path::new(DEFAULT_PORTAGE_TMPDIR),
        plan.manifest.compiler.emerge_jobs,
    )?;

    for event in &mut stream {
        print_emerge_event(event);
    }
    stream.wait()?;

    persist_manifest_value(&desired, &manifest_path)?;
    println!(
        "{}",
        "Package install completed successfully".green().bold()
    );
    Ok(())
}

fn add_packages_to_manifest(
    manifest: &mut SystemManifest,
    packages: &[String],
) -> Result<Vec<String>> {
    let mut added = Vec::new();
    for package in packages {
        let atom = package.trim();
        if atom.is_empty() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "package atom cannot be empty",
            )
            .into());
        }
        if atom.starts_with('-') {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("invalid package atom `{atom}`"),
            )
            .into());
        }
        if manifest
            .packages
            .iter()
            .any(|existing| existing.package == atom)
        {
            continue;
        }
        manifest.packages.push(Package::new(atom));
        added.push(atom.to_owned());
    }
    Ok(added)
}

fn cmd_install_system(
    confirm: bool,
    device: Option<String>,
    copy_system: bool,
    source_root: &Path,
) -> Result<()> {
    let mut desired = load_manifest(Path::new(LOCAL_MANIFEST))?;
    if let Some(device) = device {
        desired.disk.device = device;
    }

    println!(
        "{}",
        "Running first-time disk provisioning flow (live ISO assumed)"
            .cyan()
            .bold()
    );
    preflight(&desired.disk)?;
    let plan = plan_disk(&desired.disk, Path::new(DEFAULT_TARGET_MOUNT))?;
    print_disk_plan(&plan);
    confirm_disk_plan(&plan.device, confirm)?;

    println!("{}", "Provisioning disk".yellow().bold());
    let mut stream = apply_disk_plan(&plan);
    for event in &mut stream {
        print_provision_event(event);
    }
    stream.wait()?;

    println!(
        "{} {}",
        "Target root ready at".green().bold(),
        plan.target_mount.display().to_string().green()
    );

    if !copy_system {
        println!(
            "{}",
            "Disk-only scope complete; pass --copy-system to copy the live Gentoo system and install systemd-boot.".yellow()
        );
        return Ok(());
    }

    println!("{}", "Planning live system copy".yellow().bold());
    let system_plan = plan_system_install(&desired, source_root, &plan.target_mount)?;
    print_system_install_plan(&system_plan);
    confirm_system_install(confirm)?;

    println!(
        "{}",
        "Copying live system and installing systemd-boot"
            .yellow()
            .bold()
    );
    let mut stream = apply_system_install_plan(&system_plan);
    for event in &mut stream {
        print_system_install_event(event);
    }
    stream.wait()?;

    println!(
        "{}",
        "Install copy phase complete; target should have a systemd-boot entry."
            .green()
            .bold()
    );
    Ok(())
}

fn create_plan(manifest: &SystemManifest) -> Result<PortagePlan> {
    let cache_dir = oxys::util::default_use_resolver_cache_dir();
    Ok(plan_portage(
        manifest,
        &effective_portage_tree(),
        &cache_dir,
    )?)
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

fn load_manifest(path: &Path) -> Result<SystemManifest> {
    let text = fs::read_to_string(path)?;
    parse_generated_manifest_toml(&text).map_err(|err| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "failed to parse {} as generated oxys manifest: {err}. Regenerate it with the real oxys crate",
                path.display()
            ),
        )
        .into()
    })
}

fn load_manifest_optional(path: &Path) -> Result<Option<SystemManifest>> {
    if !path.exists() {
        return Ok(None);
    }
    load_manifest(path).map(Some)
}

fn print_changes(changes: &[PackageChange]) {
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
        return Ok(());
    }

    Err(io::Error::other("hard conflicts detected; refusing to continue").into())
}

fn print_disk_plan(plan: &oxys::DiskPlan) {
    println!("{}", "Disk provisioning plan".yellow().bold());
    println!(
        "  {} {}",
        "Device:".yellow().bold(),
        plan.device.red().bold()
    );
    println!(
        "  {} {}",
        "Target:".yellow().bold(),
        plan.target_mount.display().to_string().green()
    );
    println!();
    println!("{}", plan.render());
}

fn print_system_install_plan(plan: &oxys::SystemInstallPlan) {
    println!("{}", "System copy plan".yellow().bold());
    println!(
        "  {} {}",
        "Source:".yellow().bold(),
        plan.source_root.display().to_string().green()
    );
    println!(
        "  {} {}",
        "Target:".yellow().bold(),
        plan.target_mount.display().to_string().green()
    );
    println!();
    println!("{}", plan.render());
}

fn confirm_disk_plan(device: &str, confirm: bool) -> Result<()> {
    if confirm {
        return Ok(());
    }

    println!();
    println!("{} {}", "This will erase".red().bold(), device.red().bold());
    println!("Type the exact device name to proceed:");

    let mut input = String::new();
    io::stdin().read_line(&mut input)?;
    if input.trim() == device {
        Ok(())
    } else {
        Err(io::Error::other("confirmation did not match device; refusing to continue").into())
    }
}

fn confirm_system_install(confirm: bool) -> Result<()> {
    if confirm {
        return Ok(());
    }

    println!();
    println!(
        "{}",
        "This will copy the live system into the mounted target and install systemd-boot."
            .yellow()
            .bold()
    );
    println!("Type copy-system to proceed:");

    let mut input = String::new();
    io::stdin().read_line(&mut input)?;
    if input.trim() == "copy-system" {
        Ok(())
    } else {
        Err(io::Error::other("system copy confirmation did not match; refusing to continue").into())
    }
}

fn print_provision_event(event: ProvisionEvent) {
    match event {
        ProvisionEvent::StepStart { description } => {
            println!("{} {}", "Starting".yellow().bold(), description.yellow());
        }
        ProvisionEvent::StepOutput { line } => {
            println!("  {}", truncate_line(&line));
        }
        ProvisionEvent::StepComplete { description } => {
            println!("{} {}", "Complete".green().bold(), description.green());
        }
        ProvisionEvent::Error { step, message } => {
            println!(
                "{} {} {}",
                "Provision error".red().bold(),
                step.red().bold(),
                truncate_line(&message).red()
            );
        }
    }
}

fn print_system_install_event(event: SystemInstallEvent) {
    match event {
        SystemInstallEvent::StepStart { description } => {
            println!("{} {}", "Starting".yellow().bold(), description.yellow());
        }
        SystemInstallEvent::StepOutput { line } => {
            println!("  {}", truncate_line(&line));
        }
        SystemInstallEvent::StepComplete { description } => {
            println!("{} {}", "Complete".green().bold(), description.green());
        }
        SystemInstallEvent::Error { step, message } => {
            println!(
                "{} {} {}",
                "Install error".red().bold(),
                step.red().bold(),
                truncate_line(&message).red()
            );
        }
    }
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

fn persist_manifest(source: &Path, destination: &Path) -> Result<()> {
    if let Some(parent) = destination.parent() {
        fs::create_dir_all(parent)?;
    }

    fs::copy(source, destination)?;
    println!(
        "{} {}",
        "Saved current manifest:".green().bold(),
        destination.display().to_string().green()
    );
    Ok(())
}

fn persist_manifest_value(manifest: &SystemManifest, destination: &Path) -> Result<()> {
    if let Some(parent) = destination.parent() {
        fs::create_dir_all(parent)?;
    }

    let rendered = manifest_to_toml(manifest)?;
    fs::write(destination, rendered)?;
    println!(
        "{} {}",
        "Saved current manifest:".green().bold(),
        destination.display().to_string().green()
    );
    Ok(())
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

pub(crate) fn print_command_output(stdout: &[u8], stderr: &[u8]) {
    if !stdout.is_empty() {
        print!("{}", String::from_utf8_lossy(stdout));
    }
    if !stderr.is_empty() {
        eprint!("{}", String::from_utf8_lossy(stderr));
    }
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
        let err = cmd_install(Vec::new(), false, None, false, Path::new(DEFAULT_ROOT))
            .expect_err("bare install must be rejected");
        assert!(err.to_string().contains("choose what to install"));
    }

    #[test]
    fn package_install_rejects_disk_flags() {
        let err = cmd_install(
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

        let added = add_packages_to_manifest(
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
