use std::{
    fmt,
    path::{Path, PathBuf},
};

use thiserror::Error;

use crate::exec::{ExecError, StepEvent, StepStream};
use crate::graphics::{GraphicsResolveError, ResolvedGraphics};
use crate::kernel_cmdline::{KernelCmdlineResolveError, ResolvedKernelCmdline};
use crate::manifest::{
    Bootloader, Disk, DiskLayout, InitSystem, ResolvedSwap, SwapResolveError, SystemManifest, User,
};
use crate::runtime::RuntimeConfigError;
use crate::session::{ResolvedSession, SessionResolveError};
use crate::use_resolver::UseResolverError;

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
    pub resolved_swap: ResolvedSwap,
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
// A plan holds a few dozen steps at most, so the size skew from variants
// carrying a SystemManifest is not worth the churn of boxing them.
#[allow(clippy::large_enum_variant)]
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
        resolved_swap: ResolvedSwap,
        target_mount: PathBuf,
    },
    ConfigureSwap {
        description: String,
        resolved_swap: ResolvedSwap,
        target_mount: PathBuf,
    },
    ConfigureFirewall {
        description: String,
        manifest: SystemManifest,
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
    ConfigureTimezone {
        description: String,
        timezone: String,
        target_mount: PathBuf,
    },
    ConfigureLocale {
        description: String,
        locale: String,
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
    SeedOxysConfig {
        description: String,
        /// The `.fe2o3` source that was compiled for this install, copied to
        /// `/etc/oxys/config.fe2o3`. `None` when the source path is unknown (the
        /// `current-manifest.toml` applied state is still written either way).
        source_fe2o3: Option<PathBuf>,
        manifest: SystemManifest,
        target_mount: PathBuf,
    },
    Finalize {
        description: String,
        manifest: SystemManifest,
        resolved_swap: ResolvedSwap,
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
            | Self::ConfigureSwap { description, .. }
            | Self::ConfigureFirewall { description, .. }
            | Self::ResetMachineId { description, .. }
            | Self::ConfigureHostname { description, .. }
            | Self::ConfigureTimezone { description, .. }
            | Self::ConfigureLocale { description, .. }
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
            | Self::SeedOxysConfig { description, .. }
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
            Self::ConfigureSwap {
                resolved_swap,
                target_mount,
                ..
            } => format!(
                "write swap policy under {} (zram: {}, disk: {}, swappiness: {})",
                target_mount.join("etc").display(),
                resolved_swap.zram.is_some(),
                resolved_swap.disk.is_some(),
                resolved_swap.swappiness
            ),
            Self::ConfigureFirewall {
                manifest,
                target_mount,
                ..
            } => {
                if manifest.firewall.enabled() {
                    format!(
                        "render nftables policy to {} (validated, mode 0600)",
                        target_mount
                            .join(crate::runtime::NFTABLES_RULES_PATH)
                            .display()
                    )
                } else {
                    format!(
                        "firewall disabled: remove any Oxys-generated {}",
                        target_mount
                            .join(crate::runtime::NFTABLES_RULES_PATH)
                            .display()
                    )
                }
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
            Self::ConfigureTimezone {
                timezone,
                target_mount,
                ..
            } => format!(
                "link {} to zoneinfo {timezone}",
                target_mount.join("etc/localtime").display()
            ),
            Self::ConfigureLocale {
                locale,
                target_mount,
                ..
            } => format!(
                "generate locale {locale} and set it as LANG under {}",
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
                resolved_kernel_cmdline
                    .values()
                    .collect::<Vec<_>>()
                    .join(" ")
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
                let enabled = services::openrc_enabled_service_count(manifest);
                format!(
                    "apply openrc service state: reconcile authoritative runlevels ({enabled} enabled)"
                )
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
            Self::SeedOxysConfig {
                source_fe2o3,
                target_mount,
                ..
            } => {
                let oxys_dir = target_mount.join("etc/oxys");
                match source_fe2o3 {
                    Some(src) => format!(
                        "seed {} from {} and write current-manifest.toml",
                        oxys_dir.join("config.fe2o3").display(),
                        src.display()
                    ),
                    None => format!(
                        "write {} (applied state)",
                        oxys_dir.join("current-manifest.toml").display()
                    ),
                }
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
    #[error("package installation failed: {0}")]
    PackageInstall(String),
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
    #[error(transparent)]
    Swap(#[from] SwapResolveError),
    #[error(transparent)]
    Firewall(#[from] crate::manifest::FirewallValidationError),
}

mod planner;

pub use planner::plan_system_install;

#[cfg(test)]
mod tests;
