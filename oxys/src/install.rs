use std::{
    fmt, fs,
    path::{Path, PathBuf},
    sync::mpsc::Sender,
};

use thiserror::Error;

use crate::exec::{self, ExecError, StepEvent, StepStream};
use crate::manifest::{Bootloader, Disk, DiskLayout, InitSystem, SystemManifest, User};
use crate::use_resolver::UseResolverError;

mod boot;
mod filesystem;
mod host;
mod login;
mod portage;
mod services;
mod users;

pub(super) use host::{blkid_value, run_chroot, write_file, zfs_dataset_name, DiskPartitionMap};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SystemInstallPlan {
    pub source_root: PathBuf,
    pub target_mount: PathBuf,
    pub steps: Vec<SystemInstallStep>,
}

impl SystemInstallPlan {
    pub fn render(&self) -> String {
        self.steps
            .iter()
            .enumerate()
            .map(|(idx, step)| {
                format!(
                    "{:>2}. {}\n    {}",
                    idx + 1,
                    step.description(),
                    step.render()
                )
            })
            .collect::<Vec<_>>()
            .join("\n")
    }
}

impl fmt::Display for SystemInstallPlan {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.render())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SystemInstallStep {
    Command {
        description: String,
        program: String,
        args: Vec<String>,
    },
    GenerateFstab {
        description: String,
        disk: Disk,
        target_mount: PathBuf,
    },
    ResetMachineId {
        description: String,
        target_mount: PathBuf,
    },
    SetupUsers {
        description: String,
        users: Vec<User>,
        target_mount: PathBuf,
    },
    InstallBootAssets {
        description: String,
        target_mount: PathBuf,
        efi_mount: String,
    },
    GenerateSystemdBoot {
        description: String,
        manifest: SystemManifest,
        target_mount: PathBuf,
    },
    GenerateGrubConfig {
        description: String,
        manifest: SystemManifest,
        target_mount: PathBuf,
    },
    ActivateSystemdServices {
        description: String,
        manifest: SystemManifest,
        target_mount: PathBuf,
    },
    ActivateOpenrcServices {
        description: String,
        manifest: SystemManifest,
        target_mount: PathBuf,
    },
    BindMountPseudo {
        description: String,
        target_mount: PathBuf,
    },
    EmergePackages {
        description: String,
        manifest: SystemManifest,
        target_mount: PathBuf,
    },
    SetupLogin {
        description: String,
        manifest: SystemManifest,
        target_mount: PathBuf,
    },
    GenerateInitramfs {
        description: String,
        target_mount: PathBuf,
        kver: String,
    },
    Finalize {
        description: String,
        manifest: SystemManifest,
        target_mount: PathBuf,
    },
}

impl SystemInstallStep {
    fn command(
        description: impl Into<String>,
        program: impl Into<String>,
        args: impl IntoIterator<Item = impl Into<String>>,
    ) -> Self {
        Self::Command {
            description: description.into(),
            program: program.into(),
            args: args.into_iter().map(Into::into).collect(),
        }
    }

    fn description(&self) -> &str {
        match self {
            Self::Command { description, .. }
            | Self::GenerateFstab { description, .. }
            | Self::ResetMachineId { description, .. }
            | Self::SetupUsers { description, .. }
            | Self::InstallBootAssets { description, .. }
            | Self::GenerateSystemdBoot { description, .. }
            | Self::GenerateGrubConfig { description, .. }
            | Self::ActivateSystemdServices { description, .. }
            | Self::ActivateOpenrcServices { description, .. }
            | Self::BindMountPseudo { description, .. }
            | Self::EmergePackages { description, .. }
            | Self::SetupLogin { description, .. }
            | Self::GenerateInitramfs { description, .. }
            | Self::Finalize { description, .. } => description,
        }
    }

    fn render(&self) -> String {
        match self {
            Self::Command { program, args, .. } => command_line(program, args),
            Self::GenerateFstab { target_mount, .. } => {
                format!(
                    "write generated {}",
                    target_mount.join("etc/fstab").display()
                )
            }
            Self::ResetMachineId { target_mount, .. } => {
                format!("truncate {}", target_mount.join("etc/machine-id").display())
            }
            // Deliberately omits any password material so secrets never reach
            // the rendered plan or install log.
            Self::SetupUsers { users, .. } => {
                let names: Vec<&str> = users.iter().map(|user| user.name.as_str()).collect();
                format!("create user account(s): {}", names.join(", "))
            }
            Self::InstallBootAssets {
                target_mount,
                efi_mount,
                ..
            } => format!(
                "copy latest kernel/initramfs from {} to {}",
                target_mount.join("boot").display(),
                target_mount
                    .join(efi_mount.trim_start_matches('/'))
                    .display()
            ),
            Self::GenerateSystemdBoot {
                manifest,
                target_mount,
                ..
            } => format!(
                "write ESP loader config and oxys boot entry under {}",
                target_mount
                    .join(manifest.disk.partitions.efi.mount.trim_start_matches('/'))
                    .display()
            ),
            Self::GenerateGrubConfig { target_mount, .. } => format!(
                "write grub.cfg under {}",
                target_mount.join("boot/grub").display()
            ),
            Self::ActivateSystemdServices { manifest, .. } => {
                let enabled = manifest.services.enabled.len();
                let disabled = manifest.services.disabled.len();
                format!("apply systemd service state ({enabled} enable, {disabled} disable)")
            }
            Self::ActivateOpenrcServices { manifest, .. } => {
                let enabled = services::openrc_enabled_services(manifest).len();
                let disabled = manifest.services.disabled.len();
                format!("apply openrc service state ({enabled} enable, {disabled} disable)")
            }
            Self::BindMountPseudo { target_mount, .. } => {
                format!(
                    "bind mount /dev, /sys, /proc, /run to {}",
                    target_mount.display()
                )
            }
            Self::EmergePackages {
                manifest,
                target_mount,
                ..
            } => {
                let packages = manifest
                    .packages
                    .iter()
                    .map(|package| package.package.as_str())
                    .collect::<Vec<_>>();
                format!(
                    "resolve and emerge manifest package(s) into {}: {}",
                    target_mount.display(),
                    packages.join(", ")
                )
            }
            Self::SetupLogin {
                manifest,
                target_mount,
                ..
            } => {
                let kind = if login::manifest_wants_graphical(manifest)
                    && login::graphical_login_user(manifest).is_some()
                {
                    "Niri autologin session"
                } else if login::manifest_wants_graphical(manifest) {
                    "text login (no graphical user configured)"
                } else {
                    "text login"
                };
                format!("configure tty1 for {kind} under {}", target_mount.display())
            }
            Self::GenerateInitramfs { kver, .. } => {
                format!("generate ZFS-root initramfs for kernel {kver}")
            }
            Self::Finalize { target_mount, .. } => {
                format!(
                    "finalize installation (unmount target and export pools under {})",
                    target_mount.display()
                )
            }
        }
    }
}

pub type SystemInstallEvent = StepEvent;
pub type SystemInstallStream = StepStream<SystemInstallError>;

#[derive(Debug, Error)]
pub enum SystemInstallError {
    #[error("target mount does not exist: {0}")]
    TargetMissing(String),
    #[error("source root does not exist: {0}")]
    SourceMissing(String),
    #[error("unsupported layout for bootable system copy: {0:?}")]
    UnsupportedLayout(DiskLayout),
    #[error("invalid install plan: {0}")]
    InvalidPlan(String),
    #[error("install I/O failed: {0}")]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Portage(#[from] UseResolverError),
    #[error(transparent)]
    Exec(#[from] ExecError),
}

pub fn plan_system_install(
    manifest: &SystemManifest,
    source_root: &Path,
    target_mount: &Path,
) -> Result<SystemInstallPlan, SystemInstallError> {
    if !source_root.exists() {
        return Err(SystemInstallError::SourceMissing(
            source_root.display().to_string(),
        ));
    }
    if !target_mount.exists() {
        return Err(SystemInstallError::TargetMissing(
            target_mount.display().to_string(),
        ));
    }
    if let Some(index) = manifest.prompt_usernames().first() {
        return Err(SystemInstallError::InvalidPlan(format!(
            "username for user {index} was not collected before install"
        )));
    }
    if !matches!(manifest.disk.layout, DiskLayout::Ext4 | DiskLayout::Zfs) {
        return Err(SystemInstallError::UnsupportedLayout(manifest.disk.layout));
    }

    let source = ensure_trailing_slash(source_root);
    let target = ensure_trailing_slash(target_mount);
    let source_boot = ensure_trailing_slash(&source_root.join("boot"));
    let target_boot = ensure_trailing_slash(&target_mount.join("boot"));
    let efi_mount = manifest.disk.partitions.efi.mount.clone();
    let target_esp = target_mount.join(efi_mount.trim_start_matches('/'));
    let mut steps = vec![SystemInstallStep::command(
        "Copy live system into target",
        "rsync",
        rsync_args(&source, &target),
    )];
    steps.push(SystemInstallStep::command(
        "Copy live boot files into target",
        "rsync",
        boot_rsync_args(&source_boot, &target_boot),
    ));

    steps.push(SystemInstallStep::command(
        "Create target runtime directories",
        "mkdir",
        [
            "-p",
            &target_mount.join("dev").display().to_string(),
            &target_mount.join("proc").display().to_string(),
            &target_mount.join("sys").display().to_string(),
            &target_mount.join("run").display().to_string(),
            &target_mount.join("tmp").display().to_string(),
            &target_mount.join("mnt").display().to_string(),
            &target_mount.join("media").display().to_string(),
        ],
    ));
    steps.push(SystemInstallStep::command(
        "Set target /tmp permissions",
        "chmod",
        ["1777", &target_mount.join("tmp").display().to_string()],
    ));
    steps.push(SystemInstallStep::BindMountPseudo {
        description: "Bind mount pseudo filesystems".to_owned(),
        target_mount: target_mount.to_path_buf(),
    });
    if !manifest.packages.is_empty() {
        steps.push(SystemInstallStep::EmergePackages {
            description: format!("Install {} manifest package(s)", manifest.packages.len()),
            manifest: manifest.clone(),
            target_mount: target_mount.to_path_buf(),
        });
    }
    steps.push(SystemInstallStep::GenerateFstab {
        description: "Write target fstab".to_owned(),
        disk: manifest.disk.clone(),
        target_mount: target_mount.to_path_buf(),
    });
    steps.push(SystemInstallStep::ResetMachineId {
        description: "Reset target machine-id".to_owned(),
        target_mount: target_mount.to_path_buf(),
    });
    if !manifest.users.is_empty() {
        steps.push(SystemInstallStep::SetupUsers {
            description: format!("Create {} user account(s)", manifest.users.len()),
            users: manifest.users.clone(),
            target_mount: target_mount.to_path_buf(),
        });
    }
    if manifest.disk.layout == DiskLayout::Zfs {
        steps.push(SystemInstallStep::command(
            "Create target ZFS cache directory",
            "mkdir",
            ["-p", &target_mount.join("etc/zfs").display().to_string()],
        ));
        steps.push(SystemInstallStep::command(
            "Copy hostid to target",
            "cp",
            [
                "/etc/hostid",
                &target_mount.join("etc/hostid").display().to_string(),
            ],
        ));
        steps.push(SystemInstallStep::command(
            "Refresh root pool import cache",
            "zpool",
            [
                "set",
                &format!(
                    "cachefile={}",
                    target_mount.join("etc/zfs/zpool.cache").display()
                ),
                &manifest.disk.zfs.pool,
            ],
        ));
        steps.push(SystemInstallStep::command(
            "Refresh boot pool import cache",
            "zpool",
            [
                "set",
                &format!(
                    "cachefile={}",
                    target_mount.join("etc/zfs/zpool.cache").display()
                ),
                &manifest.disk.zfs.boot_pool,
            ],
        ));
        let kver = boot::derive_kernel_version(source_root)?;
        steps.push(SystemInstallStep::GenerateInitramfs {
            description: format!("Generate ZFS-root initramfs ({kver})"),
            target_mount: target_mount.to_path_buf(),
            kver,
        });
    }
    match manifest.resolved_bootloader() {
        Bootloader::SystemdBoot => {
            steps.push(SystemInstallStep::command(
                "Install systemd-boot",
                "bootctl",
                ["--esp-path", &target_esp.display().to_string(), "install"],
            ));
            steps.push(SystemInstallStep::InstallBootAssets {
                description: "Copy kernel and initramfs to ESP".to_owned(),
                target_mount: target_mount.to_path_buf(),
                efi_mount: efi_mount.clone(),
            });
            steps.push(SystemInstallStep::GenerateSystemdBoot {
                description: "Write systemd-boot loader entry".to_owned(),
                manifest: manifest.clone(),
                target_mount: target_mount.to_path_buf(),
            });
        }
        Bootloader::Grub => {
            if manifest.disk.layout != DiskLayout::Zfs {
                steps.push(SystemInstallStep::InstallBootAssets {
                    description: "Copy kernel and initramfs to ESP".to_owned(),
                    target_mount: target_mount.to_path_buf(),
                    efi_mount: efi_mount.clone(),
                });
            }
            steps.push(SystemInstallStep::command(
                "Install GRUB",
                "grub-install",
                [
                    "--target=x86_64-efi".to_owned(),
                    format!("--efi-directory={}", target_esp.display()),
                    format!("--boot-directory={}", target_mount.join("boot").display()),
                    "--removable".to_owned(),
                ],
            ));
            steps.push(SystemInstallStep::GenerateGrubConfig {
                description: "Write grub.cfg".to_owned(),
                manifest: manifest.clone(),
                target_mount: target_mount.to_path_buf(),
            });
        }
    }
    if !services::openrc_enabled_services(manifest).is_empty()
        || !manifest.services.disabled.is_empty()
    {
        match manifest.init_system {
            InitSystem::Systemd => steps.push(SystemInstallStep::ActivateSystemdServices {
                description: "Apply systemd service state".to_owned(),
                manifest: manifest.clone(),
                target_mount: target_mount.to_path_buf(),
            }),
            InitSystem::Openrc => steps.push(SystemInstallStep::ActivateOpenrcServices {
                description: "Apply openrc service state".to_owned(),
                manifest: manifest.clone(),
                target_mount: target_mount.to_path_buf(),
            }),
        }
    }
    steps.push(SystemInstallStep::SetupLogin {
        description: "Configure console login".to_owned(),
        manifest: manifest.clone(),
        target_mount: target_mount.to_path_buf(),
    });
    steps.push(SystemInstallStep::Finalize {
        description: "Finalize installation (unmount and export)".to_owned(),
        manifest: manifest.clone(),
        target_mount: target_mount.to_path_buf(),
    });
    Ok(SystemInstallPlan {
        source_root: source_root.to_path_buf(),
        target_mount: target_mount.to_path_buf(),
        steps,
    })
}

pub fn apply_system_install_plan(plan: &SystemInstallPlan) -> SystemInstallStream {
    let plan = plan.clone();
    StepStream::spawn(move |sender| run_plan(plan, sender))
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
        SystemInstallStep::Command { .. } => unreachable!("command steps return above"),
        SystemInstallStep::GenerateFstab {
            disk, target_mount, ..
        } => filesystem::write_fstab(disk, target_mount)?,
        SystemInstallStep::ResetMachineId { target_mount, .. } => {
            filesystem::reset_machine_id(target_mount)?
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
            target_mount,
            ..
        } => boot::write_systemd_boot(manifest, target_mount)?,
        SystemInstallStep::GenerateGrubConfig {
            manifest,
            target_mount,
            ..
        } => boot::write_grub_config(manifest, target_mount)?,
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
        SystemInstallStep::EmergePackages {
            manifest,
            target_mount,
            ..
        } => portage::emerge_manifest_packages(manifest, target_mount, sender)?,
        SystemInstallStep::SetupLogin {
            manifest,
            target_mount,
            ..
        } => login::setup_login(manifest, target_mount, sender)?,
        SystemInstallStep::GenerateInitramfs {
            target_mount, kver, ..
        } => boot::generate_initramfs(target_mount, kver, sender)?,
        SystemInstallStep::Finalize {
            manifest,
            target_mount,
            ..
        } => host::finalize_install(manifest, target_mount, sender)?,
    }

    let _ = sender.send(SystemInstallEvent::StepComplete { description });
    Ok(())
}

pub(super) const OXYS_SHELL_BOOT_FLAG: &str = "oxys.shell";

/// The graphical autologin only makes sense when a Wayland compositor is
/// actually installed. Key off niri specifically, since that is what the
/// generated login-shell profile launches.
pub(super) fn write_wheel_sudoers(target_mount: &Path) -> Result<(), SystemInstallError> {
    use std::os::unix::fs::PermissionsExt;

    let path = target_mount.join("etc/sudoers.d/wheel");
    write_file(&path, "# generated by oxys\n%wheel ALL=(ALL:ALL) ALL\n")?;
    // sudo refuses to read drop-ins that are group/world writable.
    fs::set_permissions(&path, fs::Permissions::from_mode(0o440))?;
    Ok(())
}

fn rsync_args(source: &str, target: &str) -> Vec<String> {
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

fn boot_rsync_args(source: &str, target: &str) -> Vec<String> {
    vec![
        "-aHAX".to_owned(),
        "--numeric-ids".to_owned(),
        "--exclude=/efi/*".to_owned(),
        source.to_owned(),
        target.to_owned(),
    ]
}

fn ensure_trailing_slash(path: &Path) -> String {
    let mut rendered = path.display().to_string();
    if !rendered.ends_with('/') {
        rendered.push('/');
    }
    rendered
}

fn command_line(program: &str, args: &[String]) -> String {
    std::iter::once(program)
        .chain(args.iter().map(String::as_str))
        .map(shell_quote)
        .collect::<Vec<_>>()
        .join(" ")
}

fn shell_quote(value: &str) -> String {
    crate::util::shell_quote(value)
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        path::{Path, PathBuf},
        sync::mpsc,
        time::{SystemTime, UNIX_EPOCH},
    };

    use crate::manifest::{DiskPartitions, EfiPartition, Ext4Options, Package, Password, GB, MB};

    use super::*;

    #[test]
    fn system_install_plan_uses_custom_efi_mount() {
        let temp = TempTree::new("custom-efi");
        let source = temp.path().join("source");
        let target = temp.path().join("target");
        fs::create_dir_all(source.join("boot")).unwrap();
        fs::create_dir_all(&target).unwrap();

        let manifest = SystemManifest {
            disk: Disk {
                device: "/dev/vda".to_owned(),
                layout: DiskLayout::Ext4,
                partitions: DiskPartitions {
                    efi: EfiPartition {
                        size: 512 * MB,
                        mount: "/efi".to_owned(),
                    },
                    ..DiskPartitions::default()
                },
                ext4: Ext4Options {
                    separate_home: false,
                    root_size: 32 * GB,
                },
                ..Disk::default()
            },
            bootloader: Some(crate::manifest::Bootloader::SystemdBoot),
            ..SystemManifest::default()
        };

        let plan = plan_system_install(&manifest, &source, &target).unwrap();
        assert!(plan.render().contains("--esp-path"));
        assert!(plan.render().contains("/efi"));
        assert!(matches!(
            plan.steps.iter().rev().nth(2), // ..., <step>, SetupLogin, Finalize
            Some(SystemInstallStep::GenerateSystemdBoot { .. })
        ));
    }

    #[test]
    fn grub_bootloader_replaces_systemd_boot_steps() {
        let temp = TempTree::new("grub-bootloader");
        let source = temp.path().join("source");
        let target = temp.path().join("target");
        fs::create_dir_all(source.join("boot")).unwrap();
        fs::create_dir_all(&target).unwrap();

        let manifest = SystemManifest {
            disk: Disk {
                device: "/dev/vda".to_owned(),
                layout: DiskLayout::Ext4,
                ..Disk::default()
            },
            bootloader: Some(crate::manifest::Bootloader::Grub),
            ..SystemManifest::default()
        };

        let plan = plan_system_install(&manifest, &source, &target).unwrap();
        let rendered = plan.render();
        assert!(rendered.contains("grub-install"));
        assert!(rendered.contains("--removable"));
        assert!(!rendered.contains("bootctl"));
        assert!(plan
            .steps
            .iter()
            .any(|step| matches!(step, SystemInstallStep::GenerateGrubConfig { .. })));
        assert!(!plan
            .steps
            .iter()
            .any(|step| matches!(step, SystemInstallStep::GenerateSystemdBoot { .. })));
    }

    #[test]
    fn grub_is_the_default_bootloader() {
        let temp = TempTree::new("default-bootloader");
        let source = temp.path().join("source");
        let target = temp.path().join("target");
        fs::create_dir_all(source.join("boot")).unwrap();
        fs::create_dir_all(&target).unwrap();

        let manifest = SystemManifest {
            disk: Disk {
                device: "/dev/vda".to_owned(),
                layout: DiskLayout::Ext4,
                ..Disk::default()
            },
            ..SystemManifest::default()
        };

        let plan = plan_system_install(&manifest, &source, &target).unwrap();
        assert!(plan
            .steps
            .iter()
            .any(|step| matches!(step, SystemInstallStep::GenerateGrubConfig { .. })));
    }

    #[test]
    fn openrc_manifest_adds_symlink_service_activation() {
        let temp = TempTree::new("openrc-services");
        let source = temp.path().join("source");
        let target = temp.path().join("target");
        fs::create_dir_all(source.join("boot")).unwrap();
        fs::create_dir_all(&target).unwrap();

        let manifest = SystemManifest {
            disk: Disk {
                device: "/dev/vda".to_owned(),
                layout: DiskLayout::Ext4,
                ..Disk::default()
            },
            init_system: InitSystem::Openrc,
            services: crate::manifest::Services {
                enabled: vec!["NetworkManager".to_owned()],
                disabled: vec!["sshd".to_owned()],
            },
            ..SystemManifest::default()
        };

        let plan = plan_system_install(&manifest, &source, &target).unwrap();
        assert!(matches!(
            plan.steps.iter().rev().nth(2), // ..., <step>, SetupLogin, Finalize
            Some(SystemInstallStep::ActivateOpenrcServices { .. })
        ));
        assert!(plan.render().contains("apply openrc service state"));
        assert!(!plan
            .steps
            .iter()
            .any(|step| matches!(step, SystemInstallStep::ActivateSystemdServices { .. })));
    }

    #[test]
    fn openrc_service_activation_manages_runlevel_symlinks() {
        let temp = TempTree::new("openrc-symlinks");
        let target = temp.path().join("target");
        let runlevel_dir = target.join("etc/runlevels/default");
        // A stale entry that should be removed by the disable pass.
        fs::create_dir_all(&runlevel_dir).unwrap();
        std::os::unix::fs::symlink("/etc/init.d/sshd", runlevel_dir.join("sshd")).unwrap();

        let manifest = SystemManifest {
            services: crate::manifest::Services {
                enabled: vec!["NetworkManager".to_owned()],
                disabled: vec!["sshd".to_owned()],
            },
            ..SystemManifest::default()
        };

        let (sender, _receiver) = mpsc::channel();
        services::activate_openrc_services(&manifest, &target, &sender).unwrap();

        let enabled_link = runlevel_dir.join("NetworkManager");
        assert_eq!(
            fs::read_link(&enabled_link).unwrap(),
            Path::new("/etc/init.d/NetworkManager")
        );
        assert!(fs::symlink_metadata(runlevel_dir.join("sshd")).is_err());
    }

    #[test]
    fn zfs_openrc_services_are_implicit_boot_runlevel_links() {
        let temp = TempTree::new("zfs-openrc-symlinks");
        let target = temp.path().join("target");

        let manifest = SystemManifest {
            disk: Disk {
                device: "/dev/vda".to_owned(),
                layout: DiskLayout::Zfs,
                ..Disk::default()
            },
            init_system: InitSystem::Openrc,
            ..SystemManifest::default()
        };

        let (sender, _receiver) = mpsc::channel();
        services::activate_openrc_services(&manifest, &target, &sender).unwrap();

        let boot_runlevel = target.join("etc/runlevels/boot");
        assert_eq!(
            fs::read_link(boot_runlevel.join("zfs-import")).unwrap(),
            Path::new("/etc/init.d/zfs-import")
        );
        assert_eq!(
            fs::read_link(boot_runlevel.join("zfs-mount")).unwrap(),
            Path::new("/etc/init.d/zfs-mount")
        );
        assert!(fs::symlink_metadata(target.join("etc/runlevels/default/zfs-import")).is_err());
        assert!(fs::symlink_metadata(target.join("etc/runlevels/default/zfs-mount")).is_err());
    }

    #[test]
    fn explicit_systemd_manifest_adds_service_activation_step() {
        let temp = TempTree::new("systemd-services");
        let source = temp.path().join("source");
        let target = temp.path().join("target");
        fs::create_dir_all(source.join("boot")).unwrap();
        fs::create_dir_all(&target).unwrap();

        let manifest = SystemManifest {
            disk: Disk {
                device: "/dev/vda".to_owned(),
                layout: DiskLayout::Ext4,
                ..Disk::default()
            },
            init_system: InitSystem::Systemd,
            services: crate::manifest::Services {
                enabled: vec!["systemd-networkd.service".to_owned()],
                disabled: vec!["sshd.service".to_owned()],
            },
            ..SystemManifest::default()
        };

        let plan = plan_system_install(&manifest, &source, &target).unwrap();
        assert!(matches!(
            plan.steps.iter().rev().nth(2), // ..., <step>, SetupLogin, Finalize
            Some(SystemInstallStep::ActivateSystemdServices { .. })
        ));
        assert!(plan.render().contains("1 enable, 1 disable"));
    }

    #[test]
    fn zfs_system_install_plan_generates_initramfs_step() {
        let temp = TempTree::new("zfs-initramfs");
        let source = temp.path().join("source");
        let target = temp.path().join("target");
        fs::create_dir_all(source.join("boot")).unwrap();
        fs::create_dir_all(&target).unwrap();
        fs::write(source.join("boot/vmlinuz-6.6.21-gentoo"), "mock-kernel").unwrap();

        let manifest = SystemManifest {
            disk: Disk {
                device: "/dev/vda".to_owned(),
                layout: DiskLayout::Zfs,
                ..Disk::default()
            },
            bootloader: Some(crate::manifest::Bootloader::Grub),
            ..SystemManifest::default()
        };

        let plan = plan_system_install(&manifest, &source, &target).unwrap();
        let rendered = plan.render();
        assert!(rendered.contains("generate ZFS-root initramfs for kernel 6.6.21-gentoo"));
        assert!(plan
            .steps
            .iter()
            .any(|step| matches!(step, SystemInstallStep::GenerateInitramfs { kver, .. } if kver == "6.6.21-gentoo")));
    }

    #[test]
    fn manifest_packages_are_emerged_after_bind_mounts_before_initramfs() {
        let temp = TempTree::new("emerge-packages-order");
        let source = temp.path().join("source");
        let target = temp.path().join("target");
        fs::create_dir_all(source.join("boot")).unwrap();
        fs::create_dir_all(&target).unwrap();
        fs::write(source.join("boot/vmlinuz-6.6.21-gentoo"), "mock-kernel").unwrap();

        let manifest = SystemManifest {
            disk: Disk {
                device: "/dev/vda".to_owned(),
                layout: DiskLayout::Zfs,
                ..Disk::default()
            },
            packages: vec![Package::new("gui-wm/niri")],
            ..SystemManifest::default()
        };

        let plan = plan_system_install(&manifest, &source, &target).unwrap();
        let bind_idx = plan
            .steps
            .iter()
            .position(|step| matches!(step, SystemInstallStep::BindMountPseudo { .. }))
            .expect("bind mount step missing");
        let emerge_idx = plan
            .steps
            .iter()
            .position(|step| matches!(step, SystemInstallStep::EmergePackages { .. }))
            .expect("emerge packages step missing");
        let initramfs_idx = plan
            .steps
            .iter()
            .position(|step| matches!(step, SystemInstallStep::GenerateInitramfs { .. }))
            .expect("initramfs step missing");
        let finalize_idx = plan
            .steps
            .iter()
            .position(|step| matches!(step, SystemInstallStep::Finalize { .. }))
            .expect("finalize step missing");

        assert_eq!(emerge_idx, bind_idx + 1);
        assert!(emerge_idx < initramfs_idx);
        assert!(emerge_idx < finalize_idx);
    }

    #[test]
    fn package_emerge_step_is_omitted_without_manifest_packages() {
        let temp = TempTree::new("no-emerge-packages");
        let source = temp.path().join("source");
        let target = temp.path().join("target");
        fs::create_dir_all(source.join("boot")).unwrap();
        fs::create_dir_all(&target).unwrap();

        let manifest = SystemManifest {
            disk: Disk {
                device: "/dev/vda".to_owned(),
                layout: DiskLayout::Ext4,
                ..Disk::default()
            },
            ..SystemManifest::default()
        };

        let plan = plan_system_install(&manifest, &source, &target).unwrap();
        assert!(!plan
            .steps
            .iter()
            .any(|step| matches!(step, SystemInstallStep::EmergePackages { .. })));
    }

    #[test]
    fn graphical_config_autologins_first_user_and_clears_installer_autostart() {
        let temp = TempTree::new("setup-login-graphical");
        let target = temp.path().join("target");
        fs::create_dir_all(target.join("etc")).unwrap();
        fs::create_dir_all(target.join("root")).unwrap();
        fs::create_dir_all(target.join("home/testuser")).unwrap();
        // Mirror what the live-medium fsscript leaves behind and gets rsync'd.
        fs::write(
            target.join("etc/inittab"),
            "c1:12345:respawn:/sbin/agetty --autologin root --noclear 38400 tty1 linux\n\
             c2:2345:respawn:/sbin/agetty 38400 tty2 linux\n",
        )
        .unwrap();
        fs::write(
            target.join("root/.bash_profile"),
            "if [[ \"$(tty)\" == \"/dev/tty1\" ]]; then /usr/local/bin/oxys-installer; fi\n",
        )
        .unwrap();

        let manifest = SystemManifest {
            packages: vec![Package::new("gui-wm/niri")],
            users: vec![User::new("testuser")],
            ..SystemManifest::default()
        };
        let (tx, _rx) = std::sync::mpsc::channel();
        login::setup_login(&manifest, &target, &tx).unwrap();

        let inittab = fs::read_to_string(target.join("etc/inittab")).unwrap();
        assert!(inittab.contains("--autologin testuser"));
        assert!(!inittab.contains("--autologin root"));
        // Unrelated tty entries are preserved.
        assert!(inittab.contains("tty2"));
        // The installer no longer relaunches on the installed system.
        assert!(!target.join("root/.bash_profile").exists());
        let profile = fs::read_to_string(target.join("home/testuser/.bash_profile")).unwrap();
        assert!(profile.contains("startniri"));
        assert!(profile.contains(OXYS_SHELL_BOOT_FLAG));
        let bashrc = fs::read_to_string(target.join("home/testuser/.bashrc")).unwrap();
        assert!(bashrc.contains("dbus-run-session -- niri"));
        let noctalia =
            fs::read_to_string(target.join("home/testuser/.config/noctalia/config.toml")).unwrap();
        assert!(noctalia.contains("setup_wizard_enabled = false"));
        let niri = fs::read_to_string(target.join("home/testuser/.config/niri/config.kdl")).unwrap();
        assert!(niri.contains("until noctalia; do sleep 2; done"));
    }

    #[test]
    fn graphical_config_without_users_falls_back_to_text_login() {
        let temp = TempTree::new("setup-login-graphical-no-user");
        let target = temp.path().join("target");
        fs::create_dir_all(target.join("etc")).unwrap();
        fs::write(
            target.join("etc/inittab"),
            "c1:12345:respawn:/sbin/agetty --autologin root --noclear 38400 tty1 linux\n",
        )
        .unwrap();

        let manifest = SystemManifest {
            packages: vec![Package::new("gui-wm/niri")],
            ..SystemManifest::default()
        };
        let (tx, _rx) = std::sync::mpsc::channel();
        login::setup_login(&manifest, &target, &tx).unwrap();

        let inittab = fs::read_to_string(target.join("etc/inittab")).unwrap();
        assert!(inittab.contains("c1:12345:respawn:/sbin/agetty --noclear 38400 tty1 linux"));
        assert!(!inittab.contains("--autologin"));
        assert!(!inittab.contains("oxys-login"));
    }

    #[test]
    fn non_graphical_config_uses_plain_text_login() {
        let temp = TempTree::new("setup-login-text");
        let target = temp.path().join("target");
        fs::create_dir_all(target.join("etc")).unwrap();
        fs::write(
            target.join("etc/inittab"),
            "c1:12345:respawn:/sbin/agetty --autologin root --noclear 38400 tty1 linux\n",
        )
        .unwrap();

        let manifest = SystemManifest::default();
        let (tx, _rx) = std::sync::mpsc::channel();
        login::setup_login(&manifest, &target, &tx).unwrap();

        let inittab = fs::read_to_string(target.join("etc/inittab")).unwrap();
        assert!(inittab.contains("c1:12345:respawn:/sbin/agetty --noclear 38400 tty1 linux"));
        assert!(!inittab.contains("oxys-login"));
        assert!(!inittab.contains("--autologin root"));
    }

    #[test]
    fn users_add_a_setup_step_that_never_renders_secrets() {
        let temp = TempTree::new("users");
        let source = temp.path().join("source");
        let target = temp.path().join("target");
        fs::create_dir_all(source.join("boot")).unwrap();
        fs::create_dir_all(&target).unwrap();

        let manifest = SystemManifest {
            disk: Disk {
                device: "/dev/vda".to_owned(),
                layout: DiskLayout::Ext4,
                ..Disk::default()
            },
            users: vec![
                User::new("testuser")
                    .wheel()
                    .password(Password::Plain("super-secret".to_owned())),
                User::new("bot").password(Password::Prompt),
            ],
            ..SystemManifest::default()
        };

        let plan = plan_system_install(&manifest, &source, &target).unwrap();
        let setup = plan
            .steps
            .iter()
            .find(|step| matches!(step, SystemInstallStep::SetupUsers { .. }))
            .expect("plan should contain a SetupUsers step");

        // The plan carries the secret in memory but must never expose it when
        // rendered for the confirm screen or install log.
        let rendered = setup.render();
        assert!(rendered.contains("testuser"));
        assert!(rendered.contains("bot"));
        assert!(!rendered.contains("super-secret"));
        assert!(!plan.render().contains("super-secret"));
    }

    #[test]
    fn unresolved_prompt_username_is_rejected_before_planning() {
        let temp = TempTree::new("unresolved-username");
        let source = temp.path().join("source");
        let target = temp.path().join("target");
        fs::create_dir_all(source.join("boot")).unwrap();
        fs::create_dir_all(&target).unwrap();

        let manifest = SystemManifest {
            disk: Disk {
                device: "/dev/vda".to_owned(),
                layout: DiskLayout::Ext4,
                ..Disk::default()
            },
            users: vec![User::prompt().password(Password::Plain("super-secret".to_owned()))],
            ..SystemManifest::default()
        };

        let error = plan_system_install(&manifest, &source, &target).unwrap_err();
        assert!(matches!(error, SystemInstallError::InvalidPlan(_)));
    }

    #[test]
    fn users_are_omitted_when_none_configured() {
        let temp = TempTree::new("no-users");
        let source = temp.path().join("source");
        let target = temp.path().join("target");
        fs::create_dir_all(source.join("boot")).unwrap();
        fs::create_dir_all(&target).unwrap();

        let manifest = SystemManifest {
            disk: Disk {
                device: "/dev/vda".to_owned(),
                layout: DiskLayout::Ext4,
                ..Disk::default()
            },
            ..SystemManifest::default()
        };

        let plan = plan_system_install(&manifest, &source, &target).unwrap();
        assert!(!plan
            .steps
            .iter()
            .any(|step| matches!(step, SystemInstallStep::SetupUsers { .. })));
    }

    struct TempTree {
        path: PathBuf,
    }

    impl TempTree {
        fn new(name: &str) -> Self {
            let nanos = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let path = std::env::temp_dir().join(format!("oxys-install-test-{name}-{nanos}"));
            fs::create_dir_all(&path).unwrap();
            Self { path }
        }

        fn path(&self) -> &Path {
            &self.path
        }
    }

    impl Drop for TempTree {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.path);
        }
    }
}
