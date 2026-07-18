use std::{fs, io, path::Path};

use colored::Colorize;
use oxys::{
    InitSystem, ProvisionEvent, SystemInstallEvent, apply_disk_plan, apply_system_install_plan,
    compile::{compile_config_file, oxys_crate_path},
    config_source::add_package_to_source,
    plan_disk_with_swap, plan_system_install, preflight_with_swap,
};

use super::{apply::apply_manifest, manifest_io::effective_system_config_path};
use crate::{DEFAULT_ROOT, DEFAULT_TARGET_MOUNT, LOCAL_MANIFEST, Result, load_manifest};

pub(crate) fn run(
    target: Vec<String>,
    confirm: bool,
    device: Option<String>,
    copy_system: bool,
    source_root: &Path,
) -> Result<()> {
    let Some(first) = target.first() else {
        return Err(io::Error::new(io::ErrorKind::InvalidInput,
            "choose what to install: `oxys install <package>` or `oxys install system --copy-system --device /dev/...`").into());
    };
    if first == "system" {
        if target.len() > 1 {
            return Err(io::Error::new(io::ErrorKind::InvalidInput,
                "`oxys install system` does not accept package atoms; use `oxys install <package>` on an installed system").into());
        }
        return install_system(confirm, device, copy_system, source_root);
    }
    if confirm || device.is_some() || copy_system || source_root != Path::new(DEFAULT_ROOT) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "disk install flags are only valid with `oxys install system`",
        )
        .into());
    }
    install_packages(&target)
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
    println!("\n{}", plan.render());
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
    println!("\n{}", plan.render());
}

fn confirm_disk_plan(device: &str, confirmed: bool) -> Result<()> {
    if confirmed {
        return Ok(());
    }
    println!(
        "\n{} {}",
        "This will erase".red().bold(),
        device.red().bold()
    );
    println!("Type the exact device name to proceed:");
    let mut input = String::new();
    io::stdin().read_line(&mut input)?;
    if input.trim() == device {
        Ok(())
    } else {
        Err(io::Error::other("confirmation did not match device; refusing to continue").into())
    }
}

fn confirm_system_install(confirmed: bool) -> Result<()> {
    if confirmed {
        return Ok(());
    }
    println!(
        "\n{}",
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
            println!("{} {}", "Starting".yellow().bold(), description.yellow())
        }
        ProvisionEvent::StepOutput { line } => println!("  {}", truncate_line(&line)),
        ProvisionEvent::StepComplete { description } => {
            println!("{} {}", "Complete".green().bold(), description.green())
        }
        ProvisionEvent::Error { step, message } => println!(
            "{} {} {}",
            "Provision error".red().bold(),
            step.red().bold(),
            truncate_line(&message).red()
        ),
    }
}

fn print_system_install_event(event: SystemInstallEvent) {
    match event {
        SystemInstallEvent::StepStart { description } => {
            println!("{} {}", "Starting".yellow().bold(), description.yellow())
        }
        SystemInstallEvent::StepOutput { line } => println!("  {}", truncate_line(&line)),
        SystemInstallEvent::StepComplete { description } => {
            println!("{} {}", "Complete".green().bold(), description.green())
        }
        SystemInstallEvent::Error { step, message } => println!(
            "{} {} {}",
            "Install error".red().bold(),
            step.red().bold(),
            truncate_line(&message).red()
        ),
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

fn install_packages(packages: &[String]) -> Result<()> {
    let config_path = effective_system_config_path();
    let original = fs::read_to_string(&config_path).map_err(|err| {
        io::Error::new(
            err.kind(),
            format!(
                "declarative config not found at {} ({err}); package installs require an \
                 installed Oxys system. Use `oxys install system` for first-time OS installation.",
                config_path.display()
            ),
        )
    })?;

    // Edit the *source of truth*: append each atom to its `packages: vec![…]`
    // block. TOML is a compiled artifact, so it is never written directly here --
    // it is regenerated by recompiling the edited source below.
    let mut source = original.clone();
    let mut added = Vec::new();
    for atom in packages {
        if let Some(updated) = add_package_to_source(&source, atom)? {
            source = updated;
            added.push(atom.trim().to_owned());
        }
    }
    if added.is_empty() {
        println!(
            "{}",
            "All requested packages are already in the config"
                .green()
                .bold()
        );
        return Ok(());
    }

    println!(
        "{}",
        "Adding package(s) to declarative config".cyan().bold()
    );
    println!(
        "{}",
        format!("Editing {}", config_path.display()).green().bold()
    );
    for package in &added {
        println!("  {} {}", "+".green(), package.green());
    }
    fs::write(&config_path, &source)?;

    // Recompile the edited source and apply the result. If compile or apply fails,
    // restore the original source so config.fe2o3 is never left in a broken state.
    if let Err(err) = compile_source_and_apply(&config_path) {
        match fs::write(&config_path, &original) {
            Ok(()) => eprintln!(
                "{} restored {} to its previous contents",
                "note:".yellow().bold(),
                config_path.display()
            ),
            Err(restore_err) => eprintln!(
                "{} failed to restore {}: {restore_err}",
                "error:".red().bold(),
                config_path.display()
            ),
        }
        return Err(err);
    }

    println!(
        "{}",
        "Package install completed successfully".green().bold()
    );
    Ok(())
}

/// Compile the edited `.fe2o3` source into a manifest, then apply it to the running
/// system (plan → emerge → `@world` → persist `current-manifest.toml`). Requires the
/// `oxys` crate source (`/usr/src/oxys`) + cargo, the same as `oxys compile`/`apply`.
fn compile_source_and_apply(config_path: &Path) -> Result<()> {
    println!("{}", "Compiling config".yellow().bold());
    let out_dir = std::env::temp_dir().join(format!("oxys-install-{}", std::process::id()));
    fs::create_dir_all(&out_dir)?;
    let outcome = compile_config_file(config_path, &oxys_crate_path(), &out_dir)?;
    for notice in &outcome.notices {
        println!("{}", notice.yellow());
    }
    let manifest = load_manifest(&outcome.manifest_path)?;
    apply_manifest(manifest)
}

/// The installer copies the live source root to the target verbatim (rsync), so
/// the target's PID 1 is whatever init the live medium ships -- `init_system` in
/// the config only wires up service activation / bootloader, not the base init.
/// Guard against the silent mismatch where the config asks for OpenRC but the
/// live root has full `sys-apps/systemd` installed, which would own `/sbin/init`
/// on the target. `sys-apps/systemd-utils` (the standalone udev/tmpfiles that an
/// OpenRC system legitimately ships) does NOT install the manager binary at
/// `/usr/lib/systemd/systemd`, so its presence is a reliable "full systemd" tell.
fn ensure_source_init_matches(source_root: &Path, init_system: InitSystem) -> Result<()> {
    if init_system != InitSystem::Openrc {
        return Ok(());
    }
    let manager = ["usr/lib/systemd/systemd", "lib/systemd/systemd"]
        .into_iter()
        .map(|rel| source_root.join(rel))
        .find(|path| path.exists());
    if let Some(path) = manager {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "config sets init_system: Openrc, but the live source root has full \
                 sys-apps/systemd installed ({}). The install copies the live root \
                 verbatim, so the target would boot systemd, not OpenRC. Rebuild the ISO \
                 with USE=\"-systemd\" (mask sys-apps/systemd to surface whatever pulls it \
                 in), or set init_system: Systemd to match the medium.",
                path.display()
            ),
        )
        .into());
    }
    Ok(())
}

fn install_system(
    confirm: bool,
    device: Option<String>,
    copy_system: bool,
    source_root: &Path,
) -> Result<()> {
    let mut desired = load_manifest(Path::new(LOCAL_MANIFEST))?;
    if let Some(device) = device {
        desired.disk.device = device;
    }
    if copy_system {
        // Fail before touching the disk: the copy inherits the live root's init.
        ensure_source_init_matches(source_root, desired.init_system)?;
    }
    println!(
        "{}",
        "Running first-time disk provisioning flow (live ISO assumed)"
            .cyan()
            .bold()
    );
    let resolved_swap = desired.resolved_swap()?;
    preflight_with_swap(&desired.disk, &resolved_swap)?;
    let plan = plan_disk_with_swap(
        &desired.disk,
        &resolved_swap,
        Path::new(DEFAULT_TARGET_MOUNT),
    )?;
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
        println!("{}", "Disk-only scope complete; pass --copy-system to copy the live Gentoo system and install systemd-boot.".yellow());
        return Ok(());
    }
    println!("{}", "Planning live system copy".yellow().bold());
    let system_plan = plan_system_install(&desired, source_root, &plan.target_mount, None)?;
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
