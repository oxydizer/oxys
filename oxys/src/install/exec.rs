use std::sync::mpsc::Sender;

use crate::exec;

use super::{
    SystemInstallError, SystemInstallEvent, SystemInstallPlan, SystemInstallStep,
    SystemInstallStream, boot, filesystem, host, login, portage, services, users,
};

pub fn apply_system_install_plan(plan: &SystemInstallPlan) -> SystemInstallStream {
    let plan = plan.clone();
    exec::StepStream::spawn(move |sender| run_plan(plan, sender))
}

fn run_plan(
    plan: SystemInstallPlan,
    sender: Sender<SystemInstallEvent>,
) -> Result<(), SystemInstallError> {
    for step in &plan.steps {
        if let Err(error) = run_step(step, &sender) {
            let _ = sender.send(SystemInstallEvent::Error {
                step: step.description().to_owned(),
                message: error.to_string(),
            });
            // Release the target we mounted so a re-run isn't blocked by
            // preflight's mounted-disk guard (mirrors Finalize's unmount, which
            // only runs on success).
            crate::disk::release_target_mounts(&plan.target_mount);
            return Err(error);
        }
    }

    Ok(())
}

fn run_step(
    step: &SystemInstallStep,
    sender: &Sender<SystemInstallEvent>,
) -> Result<(), SystemInstallError> {
    let description = step.description().to_owned();

    if let SystemInstallStep::Command { program, args, .. } = step {
        return exec::run_command(&description, program, args, sender)
            .map_err(SystemInstallError::from);
    }

    let _ = sender.send(SystemInstallEvent::StepStart {
        description: description.clone(),
    });

    match step {
        SystemInstallStep::ResolveSession { .. } => {}
        SystemInstallStep::ResolveGraphics { .. } => {}
        SystemInstallStep::ResolveKernelCmdline { .. } => {}
        SystemInstallStep::Command { .. } => unreachable!("command steps return above"),
        SystemInstallStep::GenerateFstab {
            disk,
            resolved_swap,
            target_mount,
            ..
        } => filesystem::write_fstab(disk, resolved_swap, target_mount)?,
        SystemInstallStep::ConfigureSwap {
            resolved_swap,
            target_mount,
            ..
        } => {
            crate::runtime::sync_swap_config(resolved_swap, target_mount)?;
        }
        SystemInstallStep::ConfigureFirewall {
            manifest,
            target_mount,
            ..
        } => {
            // Renders and validates the target's rules-save only; the live
            // host's ruleset is never touched during installation.
            crate::runtime::sync_firewall_config(manifest, target_mount)?;
        }
        SystemInstallStep::ResetMachineId { target_mount, .. } => {
            filesystem::reset_machine_id(target_mount)?
        }
        SystemInstallStep::ConfigureHostname {
            hostname,
            target_mount,
            ..
        } => filesystem::write_hostname(hostname, target_mount)?,
        SystemInstallStep::ConfigureTimezone {
            timezone,
            target_mount,
            ..
        } => filesystem::write_timezone(timezone, target_mount)?,
        SystemInstallStep::ConfigureLocale {
            locale,
            target_mount,
            ..
        } => {
            filesystem::write_locale(locale, target_mount)?;
            let target = target_mount.display().to_string();
            host::run_chroot(&target, &["locale-gen".to_owned()], sender)?;
            host::run_chroot(&target, &["env-update".to_owned()], sender)?;
        }
        SystemInstallStep::SetupUsers {
            users,
            target_mount,
            ..
        } => users::setup_users(users, target_mount, sender)?,
        SystemInstallStep::InstallBootAssets {
            target_mount,
            efi_mount,
            ..
        } => boot::install_boot_assets(target_mount, efi_mount, sender)?,
        SystemInstallStep::GenerateSystemdBoot {
            manifest,
            resolved_kernel_cmdline,
            target_mount,
            ..
        } => boot::write_systemd_boot(manifest, resolved_kernel_cmdline, target_mount)?,
        SystemInstallStep::GenerateGrubConfig {
            manifest,
            resolved_kernel_cmdline,
            target_mount,
            ..
        } => boot::write_grub_config(manifest, resolved_kernel_cmdline, target_mount)?,
        SystemInstallStep::ActivateSystemdServices {
            manifest,
            target_mount,
            ..
        } => services::activate_systemd_services(manifest, target_mount, sender)?,
        SystemInstallStep::ActivateOpenrcServices {
            manifest,
            target_mount,
            ..
        } => services::activate_openrc_services(manifest, target_mount, sender)?,
        SystemInstallStep::BindMountPseudo { target_mount, .. } => {
            host::bind_mount_pseudo(target_mount, sender)?;
        }
        SystemInstallStep::VerifyTargetLayout { target_mount, .. } => {
            filesystem::verify_target_layout(target_mount, sender)?;
        }
        SystemInstallStep::EmergePackages {
            manifest,
            target_mount,
            ..
        } => portage::emerge_manifest_packages(manifest, target_mount, sender)?,
        SystemInstallStep::SetupLogin {
            manifest,
            resolved,
            resolved_graphics,
            target_mount,
            ..
        } => login::setup_login(manifest, resolved, resolved_graphics, target_mount, sender)?,
        SystemInstallStep::ConfigureGraphicsRuntime {
            manifest,
            target_mount,
            ..
        } => {
            crate::runtime::sync_graphics_runtime_config(manifest, target_mount)?;
        }
        SystemInstallStep::GenerateInitramfs {
            target_mount,
            kver,
            drivers,
            ..
        } => boot::generate_initramfs(target_mount, kver, drivers, sender)?,
        SystemInstallStep::SeedOxysConfig {
            source_fe2o3,
            manifest,
            target_mount,
            ..
        } => filesystem::seed_oxys_config(source_fe2o3.as_deref(), manifest, target_mount)?,
        SystemInstallStep::Finalize {
            manifest,
            resolved_swap,
            target_mount,
            ..
        } => host::finalize_install(manifest, resolved_swap, target_mount, sender)?,
    }

    let _ = sender.send(SystemInstallEvent::StepComplete { description });
    Ok(())
}

pub(super) fn rsync_args(source: &str, target: &str) -> Vec<String> {
    let excludes = [
        "/dev/*",
        "/proc/*",
        "/sys/*",
        "/run/*",
        "/tmp/*",
        "/mnt/*",
        "/media/*",
        "/lost+found",
        "/boot/efi/*",
        "/var/tmp/*",
        "/var/cache/binpkgs/*",
        "/var/cache/distfiles/*",
        "/root/.bash_history",
        "/etc/machine-id",
        "/etc/ssh/ssh_host_*",
    ];

    let mut args = vec![
        "-aHAXx".to_owned(),
        "--numeric-ids".to_owned(),
        "--info=progress2".to_owned(),
    ];
    for exclude in excludes {
        args.push(format!("--exclude={exclude}"));
    }
    args.push(source.to_owned());
    args.push(target.to_owned());
    args
}

pub(super) fn boot_rsync_args(source: &str, target: &str) -> Vec<String> {
    vec![
        "-aHAX".to_owned(),
        "--numeric-ids".to_owned(),
        "--info=progress2".to_owned(),
        "--exclude=/efi/*".to_owned(),
        source.to_owned(),
        target.to_owned(),
    ]
}

pub(super) fn ensure_trailing_slash(path: &std::path::Path) -> String {
    let mut rendered = path.display().to_string();
    if !rendered.ends_with('/') {
        rendered.push('/');
    }
    rendered
}

pub(super) fn command_line(program: &str, args: &[String]) -> String {
    std::iter::once(program)
        .chain(args.iter().map(String::as_str))
        .map(shell_quote)
        .collect::<Vec<_>>()
        .join(" ")
}

fn shell_quote(value: &str) -> String {
    crate::util::shell_quote(value)
}
