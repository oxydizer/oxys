use super::*;

pub fn plan_system_install(
    manifest: &SystemManifest,
    source_root: &Path,
    target_mount: &Path,
    config_source: Option<&Path>,
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
    if manifest.prompts_timezone() {
        return Err(SystemInstallError::InvalidPlan(
            "timezone was not collected before install".to_owned(),
        ));
    }
    // Validate a declared zone against the source image now, so a typo'd
    // timezone fails before any destructive disk work instead of mid-install.
    let timezone = manifest.os.timezone.as_str().trim().to_owned();
    if !timezone.is_empty()
        && !crate::timezones::timezone_exists(&source_root.join("usr/share/zoneinfo"), &timezone)
    {
        return Err(SystemInstallError::InvalidPlan(format!(
            "unknown timezone {timezone:?}: no such entry in the source image zoneinfo"
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
    let resolved_kernel_cmdline =
        crate::kernel_cmdline::resolve_kernel_cmdline_with_graphics(&manifest, &resolved_graphics)?;
    let resolved_graphics = resolved_graphics.validate_source(source_root)?;
    let manifest = resolved_graphics.materialize_manifest(&manifest);
    let resolved_swap = manifest.resolved_swap()?;
    if resolved_swap.zram.is_some() && manifest.init_system != InitSystem::Openrc {
        return Err(SystemInstallError::InvalidPlan(
            "zram swap provisioning currently supports OpenRC through sys-block/zram-init"
                .to_owned(),
        ));
    }
    // Resolve the exact glibc catalogue entry up front. This prevents a typo
    // from surviving disk setup only to fail when locale-gen runs in chroot.
    let locale = manifest.os.locale.trim().to_owned();
    if !locale.is_empty()
        && crate::locales::supported_locale_line(
            &source_root.join("usr/share/i18n/SUPPORTED"),
            &locale,
        )
        .is_none()
    {
        return Err(SystemInstallError::InvalidPlan(format!(
            "unsupported locale {locale:?}: no matching entry in the source image /usr/share/i18n/SUPPORTED"
        )));
    }
    let authoritative_openrc = manifest
        .services
        .openrc
        .runlevels()
        .any(|(_, services)| !services.is_empty());
    if authoritative_openrc
        && resolved_swap.zram.is_some()
        && !manifest
            .services
            .openrc
            .boot
            .iter()
            .any(|s| s == "zram-init")
    {
        return Err(SystemInstallError::InvalidPlan(
            "swap policy requires zram-init in services.openrc.boot".to_owned(),
        ));
    }
    // Fail on an unloadable firewall policy before any destructive disk work:
    // an enabled firewall must declare the nftables package and its OpenRC
    // default-runlevel service, or the installed system boots unprotected.
    manifest.validate_firewall()?;
    if authoritative_openrc && manifest.disk.layout == DiskLayout::Zfs {
        for required in ["zfs-import", "zfs-mount"] {
            if !manifest
                .services
                .openrc
                .boot
                .iter()
                .any(|service| service == required)
            {
                return Err(SystemInstallError::InvalidPlan(format!(
                    "ZFS requires {required} in services.openrc.boot"
                )));
            }
        }
    }
    let manifest = resolved_swap.materialize_manifest(&manifest);

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
        resolved_swap: resolved_swap.clone(),
        target_mount: target_mount.to_path_buf(),
    });
    steps.push(SystemInstallStep::ConfigureSwap {
        description: "Configure target swap policy".to_owned(),
        resolved_swap: resolved_swap.clone(),
        target_mount: target_mount.to_path_buf(),
    });
    steps.push(SystemInstallStep::ConfigureFirewall {
        description: "Configure target firewall policy".to_owned(),
        manifest: manifest.clone(),
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
    if !timezone.is_empty() {
        steps.push(SystemInstallStep::ConfigureTimezone {
            description: "Configure target timezone".to_owned(),
            timezone: timezone.clone(),
            target_mount: target_mount.to_path_buf(),
        });
    }
    if !locale.is_empty() {
        steps.push(SystemInstallStep::ConfigureLocale {
            description: "Configure target locale".to_owned(),
            locale: locale.clone(),
            target_mount: target_mount.to_path_buf(),
        });
    }
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
    if (manifest.init_system == InitSystem::Openrc
        && (authoritative_openrc
            || !manifest.services.enabled.is_empty()
            || manifest.disk.layout == DiskLayout::Zfs))
        || !manifest.services.enabled.is_empty()
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
    steps.push(SystemInstallStep::SeedOxysConfig {
        description: "Seed /etc/oxys declarative config and applied state".to_owned(),
        source_fe2o3: config_source.map(Path::to_path_buf),
        manifest: manifest.clone(),
        target_mount: target_mount.to_path_buf(),
    });
    steps.push(SystemInstallStep::Finalize {
        description: "Finalize installation (unmount and export)".to_owned(),
        manifest: manifest.clone(),
        resolved_swap: resolved_swap.clone(),
        target_mount: target_mount.to_path_buf(),
    });
    Ok(SystemInstallPlan {
        source_root: source_root.to_path_buf(),
        target_mount: target_mount.to_path_buf(),
        steps,
        resolved_session,
        resolved_graphics,
        resolved_kernel_cmdline,
        resolved_swap,
    })
}
