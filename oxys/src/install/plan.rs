use std::{
    fmt,
    path::{Path, PathBuf},
};

use thiserror::Error;

use crate::exec::{ExecError, StepEvent, StepStream};
use crate::graphics::{GraphicsResolveError, ResolvedGraphics};
use crate::kernel_cmdline::{KernelCmdlineResolveError, ResolvedKernelCmdline};
use crate::manifest::{Bootloader, Disk, DiskLayout, InitSystem, SystemManifest, User};
use crate::session::{ResolvedSession, SessionResolveError};
use crate::use_resolver::UseResolverError;
use crate::runtime::RuntimeConfigError;

#[cfg(test)]
use super::login;
use super::{boot, exec, services};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SystemInstallPlan {
    pub source_root: PathBuf,
    pub target_mount: PathBuf,
    pub steps: Vec<SystemInstallStep>,
    pub resolved_session: ResolvedSession,
    pub resolved_graphics: ResolvedGraphics,
    pub resolved_kernel_cmdline: ResolvedKernelCmdline,
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
    ResolveSession {
        description: String,
        resolved: ResolvedSession,
    },
    ResolveKernelCmdline {
        description: String,
        resolved: ResolvedKernelCmdline,
    },
    ResolveGraphics {
        description: String,
        resolved: ResolvedGraphics,
    },
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
    ConfigureHostname {
        description: String,
        hostname: String,
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
        resolved_kernel_cmdline: ResolvedKernelCmdline,
        target_mount: PathBuf,
    },
    GenerateGrubConfig {
        description: String,
        manifest: SystemManifest,
        resolved_kernel_cmdline: ResolvedKernelCmdline,
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
    VerifyTargetLayout {
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
        resolved: ResolvedSession,
        resolved_graphics: ResolvedGraphics,
        target_mount: PathBuf,
    },
    ConfigureGraphicsRuntime {
        description: String,
        manifest: SystemManifest,
        resolved: ResolvedGraphics,
        target_mount: PathBuf,
    },
    GenerateInitramfs {
        description: String,
        target_mount: PathBuf,
        kver: String,
        drivers: Vec<String>,
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

    pub(super) fn description(&self) -> &str {
        match self {
            Self::ResolveSession { description, .. }
            | Self::ResolveGraphics { description, .. }
            | Self::ResolveKernelCmdline { description, .. }
            | Self::Command { description, .. }
            | Self::GenerateFstab { description, .. }
            | Self::ResetMachineId { description, .. }
            | Self::ConfigureHostname { description, .. }
            | Self::SetupUsers { description, .. }
            | Self::InstallBootAssets { description, .. }
            | Self::GenerateSystemdBoot { description, .. }
            | Self::GenerateGrubConfig { description, .. }
            | Self::ActivateSystemdServices { description, .. }
            | Self::ActivateOpenrcServices { description, .. }
            | Self::BindMountPseudo { description, .. }
            | Self::VerifyTargetLayout { description, .. }
            | Self::EmergePackages { description, .. }
            | Self::SetupLogin { description, .. }
            | Self::ConfigureGraphicsRuntime { description, .. }
            | Self::GenerateInitramfs { description, .. }
            | Self::Finalize { description, .. } => description,
        }
    }

    fn render(&self) -> String {
        match self {
            Self::ResolveSession { resolved, .. } => resolved.render(),
            Self::ResolveGraphics { resolved, .. } => resolved.render(),
            Self::ResolveKernelCmdline { resolved, .. } => resolved.render(),
            Self::Command { program, args, .. } => exec::command_line(program, args),
            Self::GenerateFstab { target_mount, .. } => {
                format!(
                    "write generated {}",
                    target_mount.join("etc/fstab").display()
                )
            }
            Self::ResetMachineId { target_mount, .. } => {
                format!("truncate {}", target_mount.join("etc/machine-id").display())
            }
            Self::ConfigureHostname {
                hostname,
                target_mount,
                ..
            } => format!(
                "write hostname {hostname} under {}",
                target_mount.join("etc").display()
            ),
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
                resolved_kernel_cmdline,
                target_mount,
                ..
            } => format!(
                "write ESP loader config and oxys boot entry under {} with kernel arguments: {}",
                target_mount
                    .join(manifest.disk.partitions.efi.mount.trim_start_matches('/'))
                    .display(),
                resolved_kernel_cmdline.values().collect::<Vec<_>>().join(" ")
            ),
            Self::GenerateGrubConfig {
                resolved_kernel_cmdline,
                target_mount,
                ..
            } => format!(
                "write grub.cfg under {} with kernel arguments: {}",
                target_mount.join("boot/grub").display(),
                resolved_kernel_cmdline
                    .values()
                    .collect::<Vec<_>>()
                    .join(" ")
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
            Self::VerifyTargetLayout { target_mount, .. } => {
                format!(
                    "verify critical system dirs copied and root-owned under {}",
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
                resolved,
                target_mount,
                ..
            } => {
                let kind = if resolved.policy.mode == crate::session::ResolvedSessionMode::Graphical
                {
                    "oxys-login (Niri session)"
                } else {
                    "text login"
                };
                format!("configure tty1 for {kind} under {}", target_mount.display())
            }
            Self::ConfigureGraphicsRuntime {
                resolved,
                target_mount,
                ..
            } => format!(
                "write resolved NVIDIA/PRIME policy and graphics diagnostics under {} ({:?})",
                target_mount.display(),
                resolved.policy.nvidia.map(|nvidia| nvidia.prime)
            ),
            Self::GenerateInitramfs { kver, drivers, .. } => {
                let suffix = if drivers.is_empty() {
                    String::new()
                } else {
                    format!(" with drivers {}", drivers.join(","))
                };
                format!("generate ZFS-root initramfs for kernel {kver}{suffix}")
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
    #[error("target sanity check failed after copy: {0}")]
    TargetValidationFailed(String),
    #[error("install I/O failed: {0}")]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Portage(#[from] UseResolverError),
    #[error(transparent)]
    Exec(#[from] ExecError),
    #[error(transparent)]
    Session(#[from] SessionResolveError),
    #[error(transparent)]
    Graphics(#[from] GraphicsResolveError),
    #[error(transparent)]
    KernelCmdline(#[from] KernelCmdlineResolveError),
    #[error(transparent)]
    Runtime(#[from] RuntimeConfigError),
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

    // Resolve and validate all session policy before a plan containing target
    // mutations can be returned.
    let resolved_session = manifest.resolved_session()?;
    resolved_session.validate_source(source_root)?;
    let manifest = resolved_session.materialize_manifest(manifest);
    let resolved_graphics = manifest.resolved_graphics()?.resolve_runtime_nodes()?;
    let resolved_kernel_cmdline = crate::kernel_cmdline::resolve_kernel_cmdline_with_graphics(
        &manifest,
        &resolved_graphics,
    )?;
    let resolved_graphics = resolved_graphics.validate_source(source_root)?;
    let manifest = resolved_graphics.materialize_manifest(&manifest);

    let source = exec::ensure_trailing_slash(source_root);
    let target = exec::ensure_trailing_slash(target_mount);
    let source_boot = exec::ensure_trailing_slash(&source_root.join("boot"));
    let target_boot = exec::ensure_trailing_slash(&target_mount.join("boot"));
    let efi_mount = manifest.disk.partitions.efi.mount.clone();
    let target_esp = target_mount.join(efi_mount.trim_start_matches('/'));
    let mut steps = vec![
        SystemInstallStep::ResolveSession {
            description: "Resolve and validate session policy".to_owned(),
            resolved: resolved_session.clone(),
        },
        SystemInstallStep::ResolveGraphics {
            description: "Resolve and validate graphics policy".to_owned(),
            resolved: resolved_graphics.clone(),
        },
        SystemInstallStep::ResolveKernelCmdline {
            description: "Resolve and validate kernel command line".to_owned(),
            resolved: resolved_kernel_cmdline.clone(),
        },
        SystemInstallStep::command(
            "Copy live system into target",
            "rsync",
            exec::rsync_args(&source, &target),
        ),
    ];
    steps.push(SystemInstallStep::command(
        "Copy live boot files into target",
        "rsync",
        exec::boot_rsync_args(&source_boot, &target_boot),
    ));
    // Catch a truncated copy (e.g. a full target disk) or a mis-owned /var
    // before we chroot in and build on top of a broken root.
    steps.push(SystemInstallStep::VerifyTargetLayout {
        description: "Verify copied target layout".to_owned(),
        target_mount: target_mount.to_path_buf(),
    });

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
    steps.push(SystemInstallStep::ConfigureHostname {
        description: "Configure target hostname".to_owned(),
        hostname: manifest.os.hostname.clone(),
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
            drivers: resolved_graphics.requirements.initramfs_modules.clone(),
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
                resolved_kernel_cmdline: resolved_kernel_cmdline.clone(),
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
                resolved_kernel_cmdline: resolved_kernel_cmdline.clone(),
                target_mount: target_mount.to_path_buf(),
            });
        }
    }
    if !services::openrc_enabled_services(&manifest).is_empty()
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
        resolved: resolved_session.clone(),
        resolved_graphics: resolved_graphics.clone(),
        target_mount: target_mount.to_path_buf(),
    });
    steps.push(SystemInstallStep::ConfigureGraphicsRuntime {
        description: "Configure graphics runtime policy and diagnostics".to_owned(),
        manifest: manifest.clone(),
        resolved: resolved_graphics.clone(),
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
        resolved_session,
        resolved_graphics,
        resolved_kernel_cmdline,
    })
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        path::{Path, PathBuf},
        sync::mpsc,
        time::{SystemTime, UNIX_EPOCH},
    };

    use crate::manifest::{DiskPartitions, EfiPartition, Ext4Options, GB, MB, Package, Password};

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
            plan.steps.iter().rev().nth(3), // ..., <step>, SetupLogin, GraphicsRuntime, Finalize
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
        assert!(
            plan.steps
                .iter()
                .any(|step| matches!(step, SystemInstallStep::GenerateGrubConfig { .. }))
        );
        assert!(
            !plan
                .steps
                .iter()
                .any(|step| matches!(step, SystemInstallStep::GenerateSystemdBoot { .. }))
        );
    }

    #[test]
    fn kernel_cmdline_conflicts_are_rejected_before_a_plan_is_returned() {
        let temp = TempTree::new("kernel-cmdline-conflict");
        let source = temp.path().join("source");
        let target = temp.path().join("target");
        fs::create_dir_all(source.join("boot")).unwrap();
        fs::create_dir_all(&target).unwrap();

        let manifest = SystemManifest {
            hardware: crate::manifest::Hardware {
                graphics: crate::manifest::Graphics {
                    nvidia: Some(crate::manifest::Nvidia::default()),
                    ..crate::manifest::Graphics::default()
                },
                ..crate::manifest::Hardware::default()
            },
            kernel: crate::manifest::Kernel {
                cmdline: vec!["nvidia_drm.modeset=0".to_owned()],
            },
            ..SystemManifest::default()
        };

        let error = plan_system_install(&manifest, &source, &target).unwrap_err();
        assert!(error.to_string().contains("conflicting kernel arguments"));
        assert!(error.to_string().contains("hardware.graphics.nvidia.modeset"));
    }

    #[test]
    fn graphics_capabilities_are_validated_and_rendered_before_copy() {
        let temp = TempTree::new("graphics-capabilities");
        let source = temp.path().join("source");
        let target = temp.path().join("target");
        fs::create_dir_all(source.join("boot")).unwrap();
        fs::create_dir_all(source.join("usr/lib64/dri")).unwrap();
        fs::create_dir_all(&target).unwrap();
        fs::write(source.join("usr/lib64/dri/virtio_gpu_dri.so"), "fixture").unwrap();
        fs::write(
            source.join("boot/config-test"),
            "CONFIG_DRM=y\nCONFIG_DRM_KMS_HELPER=y\nCONFIG_DRM_GEM_SHMEM_HELPER=y\nCONFIG_DRM_VIRTIO_GPU=m\nCONFIG_VIRTIO=y\nCONFIG_VIRTIO_PCI=y\n",
        )
        .unwrap();
        let manifest = SystemManifest {
            hardware: crate::manifest::Hardware {
                graphics: crate::manifest::Graphics {
                    mesa: crate::manifest::MesaGraphics {
                        video_cards: crate::manifest::VideoCards::Explicit(vec![
                            crate::manifest::VideoCard::Virgl,
                        ]),
                        ..crate::manifest::MesaGraphics::default()
                    },
                    vm_support: crate::manifest::VmGraphics::Virgl,
                    ..crate::manifest::Graphics::default()
                },
                ..crate::manifest::Hardware::default()
            },
            ..SystemManifest::default()
        };

        let plan = plan_system_install(&manifest, &source, &target).unwrap();
        assert!(matches!(
            plan.steps.get(1),
            Some(SystemInstallStep::ResolveGraphics { .. })
        ));
        let rendered = plan.render();
        assert!(rendered.contains("Mesa capability check: passed"));
        assert!(rendered.contains("boot/config-test"));
        assert!(rendered.find("graphics policy:").unwrap() < rendered.find("rsync").unwrap());
    }

    #[test]
    fn missing_graphics_capability_rejects_install_plan() {
        let temp = TempTree::new("missing-graphics-capability");
        let source = temp.path().join("source");
        let target = temp.path().join("target");
        fs::create_dir_all(source.join("boot")).unwrap();
        fs::create_dir_all(&target).unwrap();
        fs::write(source.join("boot/config-test"), "CONFIG_DRM=y\n").unwrap();
        let manifest = SystemManifest {
            hardware: crate::manifest::Hardware {
                graphics: crate::manifest::Graphics {
                    mesa: crate::manifest::MesaGraphics {
                        video_cards: crate::manifest::VideoCards::Explicit(vec![
                            crate::manifest::VideoCard::Virgl,
                        ]),
                        ..crate::manifest::MesaGraphics::default()
                    },
                    vm_support: crate::manifest::VmGraphics::Virgl,
                    ..crate::manifest::Graphics::default()
                },
                ..crate::manifest::Hardware::default()
            },
            ..SystemManifest::default()
        };

        let error = plan_system_install(&manifest, &source, &target).unwrap_err();
        assert!(error.to_string().contains("video_cards_virgl"));
        assert!(error.to_string().contains("CONFIG_DRM_VIRTIO_GPU"));
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
        assert!(
            plan.steps
                .iter()
                .any(|step| matches!(step, SystemInstallStep::GenerateGrubConfig { .. }))
        );
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
            plan.steps.iter().rev().nth(3), // ..., <step>, SetupLogin, GraphicsRuntime, Finalize
            Some(SystemInstallStep::ActivateOpenrcServices { .. })
        ));
        assert!(plan.render().contains("apply openrc service state"));
        assert!(
            !plan
                .steps
                .iter()
                .any(|step| matches!(step, SystemInstallStep::ActivateSystemdServices { .. }))
        );
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
            plan.steps.iter().rev().nth(3), // ..., <step>, SetupLogin, GraphicsRuntime, Finalize
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
        assert!(
            !plan
                .steps
                .iter()
                .any(|step| matches!(step, SystemInstallStep::EmergePackages { .. }))
        );
    }

    #[test]
    fn graphical_config_wires_oxys_login_and_clears_installer_autostart() {
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
            session: crate::manifest::Session {
                mode: crate::manifest::SessionMode::Graphical,
                desktop_shell: Some(crate::manifest::DesktopShell::Noctalia),
                ..crate::manifest::Session::default()
            },
            packages: vec![
                Package::new("gui-wm/niri"),
                Package::new("gui-shells/noctalia"),
                Package::new("media-video/pipewire"),
            ],
            users: vec![User::new("testuser")],
            ..SystemManifest::default()
        };
        let (tx, _rx) = std::sync::mpsc::channel();
        setup_login_for_test(&manifest, &target, &tx);

        let inittab = fs::read_to_string(target.join("etc/inittab")).unwrap();
        // tty1 hands off to oxys-login (its own PAM prompt), not an autologin.
        assert!(inittab.contains("--login-program /usr/local/bin/oxys-login"));
        assert!(inittab.contains("--skip-login"));
        assert!(!inittab.contains("--autologin"));
        // Unrelated tty entries are preserved.
        assert!(inittab.contains("tty2"));
        // The installer no longer relaunches on the installed system.
        assert!(!target.join("root/.bash_profile").exists());
        // The session is launched by oxys-login on tty1, not the shell profile:
        // .bash_profile only sources .bashrc, and `startniri` is a manual helper
        // that lives in .bashrc.
        let profile = fs::read_to_string(target.join("home/testuser/.bash_profile")).unwrap();
        assert!(profile.contains(".bashrc"));
        assert!(!profile.contains("startniri"));
        let bashrc = fs::read_to_string(target.join("home/testuser/.bashrc")).unwrap();
        assert!(bashrc.contains("startniri"));
        assert!(bashrc.contains("dbus-run-session -- niri"));
        let noctalia =
            fs::read_to_string(target.join("home/testuser/.config/noctalia/config.toml")).unwrap();
        assert!(noctalia.contains("setup_wizard_enabled = false"));
        let niri =
            fs::read_to_string(target.join("home/testuser/.config/niri/config.kdl")).unwrap();
        assert!(niri.contains("exec gentoo-pipewire-launcher"));
        assert!(niri.contains("until noctalia; do sleep 2; done"));
        let session_env = fs::read_to_string(target.join("etc/oxys/session.env")).unwrap();
        assert!(session_env.contains("LIBSEAT_BACKEND=seatd"));
        assert!(session_env.contains("OXYS_FALLBACK_TTY_LOGIN=true"));
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
        setup_login_for_test(&manifest, &target, &tx);

        let inittab = fs::read_to_string(target.join("etc/inittab")).unwrap();
        assert!(inittab.contains("c1:12345:respawn:/sbin/agetty --noclear 38400 tty1 linux"));
        assert!(!inittab.contains("--autologin"));
        assert!(!inittab.contains("oxys-login"));
    }

    #[test]
    fn explicit_session_requirements_are_materialized_and_rendered_before_copy() {
        let temp = TempTree::new("explicit-session-plan");
        let source = temp.path().join("source");
        let target = temp.path().join("target");
        fs::create_dir_all(source.join("boot")).unwrap();
        write_graphical_source_requirements(&source);
        fs::create_dir_all(&target).unwrap();

        let manifest = SystemManifest {
            session: crate::manifest::Session {
                mode: crate::manifest::SessionMode::Graphical,
                user: crate::manifest::SessionUser::Named("desktop".to_owned()),
                desktop_shell: Some(crate::manifest::DesktopShell::Noctalia),
                seat: crate::manifest::SeatBackend::Seatd,
                session_tracker: crate::manifest::SessionTracker::Elogind,
                ..crate::manifest::Session::default()
            },
            users: vec![User::new("admin"), User::new("desktop")],
            ..SystemManifest::default()
        };

        let plan = plan_system_install(&manifest, &source, &target).unwrap();
        assert!(matches!(
            plan.steps.first(),
            Some(SystemInstallStep::ResolveSession { .. })
        ));
        assert_eq!(
            plan.resolved_session.policy.user_name.as_deref(),
            Some("desktop")
        );
        let rendered = plan.render();
        assert!(rendered.contains("session.mode = graphical [explicit]"));
        assert!(rendered.contains("services: dbus, seatd, elogind"));
        assert!(rendered.contains("user groups: video, input, audio"));

        let users = plan
            .steps
            .iter()
            .find_map(|step| match step {
                SystemInstallStep::SetupUsers { users, .. } => Some(users),
                _ => None,
            })
            .unwrap();
        assert!(!users[0].groups.contains(&"video".to_owned()));
        assert!(users[1].groups.contains(&"video".to_owned()));
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
        setup_login_for_test(&manifest, &target, &tx);

        let inittab = fs::read_to_string(target.join("etc/inittab")).unwrap();
        assert!(inittab.contains("c1:12345:respawn:/sbin/agetty --noclear 38400 tty1 linux"));
        assert!(!inittab.contains("oxys-login"));
        assert!(!inittab.contains("--autologin root"));
    }

    fn setup_login_for_test(
        manifest: &SystemManifest,
        target: &Path,
        sender: &std::sync::mpsc::Sender<SystemInstallEvent>,
    ) {
        let resolved = manifest.resolved_session().unwrap();
        let materialized = resolved.materialize_manifest(manifest);
        let resolved_graphics = materialized.resolved_graphics().unwrap();
        login::setup_login(
            &materialized,
            &resolved,
            &resolved_graphics,
            target,
            sender,
        )
        .unwrap();
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
        assert!(
            !plan
                .steps
                .iter()
                .any(|step| matches!(step, SystemInstallStep::SetupUsers { .. }))
        );
    }

    #[test]
    fn verify_target_layout_step_runs_immediately_after_copy() {
        let temp = TempTree::new("verify-step-order");
        let source = temp.path().join("source");
        let target = temp.path().join("target");
        fs::create_dir_all(source.join("boot")).unwrap();
        fs::create_dir_all(&target).unwrap();

        let plan = plan_system_install(&SystemManifest::default(), &source, &target).unwrap();
        let verify_at = plan
            .steps
            .iter()
            .position(|step| matches!(step, SystemInstallStep::VerifyTargetLayout { .. }))
            .expect("verify step present");
        // Sits right after the two rsync copies, before anything chroots in.
        assert!(verify_at >= 2);
        assert!(matches!(
            plan.steps[verify_at - 1],
            SystemInstallStep::Command { .. }
        ));
        let bind_at = plan
            .steps
            .iter()
            .position(|step| matches!(step, SystemInstallStep::BindMountPseudo { .. }))
            .expect("bind step present");
        assert!(verify_at < bind_at);
    }

    #[test]
    fn verify_target_layout_flags_missing_dir_and_bad_owner() {
        let temp = TempTree::new("verify-layout");
        let target = temp.path().join("target");
        // A complete-looking tree EXCEPT var/db/pkg, to simulate a truncated copy.
        for dir in ["etc", "usr", "var/tmp", "bin", "sbin", "lib", "root"] {
            fs::create_dir_all(target.join(dir)).unwrap();
        }

        let (sender, _receiver) = mpsc::channel();
        let err = super::super::filesystem::verify_target_layout(&target, &sender)
            .expect_err("missing var/db/pkg must fail");
        let message = err.to_string();
        assert!(message.contains("var/db/pkg"), "got: {message}");
        // Dirs created by the (non-root) test user are not root-owned, so the
        // ownership pass also fires -- proving it detects a mis-owned /var.
        assert!(message.contains("expected root"), "got: {message}");
    }

    struct TempTree {
        path: PathBuf,
    }

    fn write_graphical_source_requirements(source: &Path) {
        use std::os::unix::fs::PermissionsExt;

        for relative in [
            "usr/local/bin/oxys-login",
            "usr/bin/agetty",
            "usr/bin/login",
        ] {
            let path = source.join(relative);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(&path, "fixture").unwrap();
            fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
        }
        fs::create_dir_all(source.join("etc/pam.d")).unwrap();
        fs::write(source.join("etc/pam.d/login"), "auth include system-auth\n").unwrap();
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
