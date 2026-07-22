use super::*;

fn ext4_manifest() -> SystemManifest {
    SystemManifest {
        disk: Disk {
            device: "/dev/vda".to_owned(),
            layout: DiskLayout::Ext4,
            ..Disk::default()
        },
        ..SystemManifest::default()
    }
}

#[test]
fn unresolved_prompt_timezone_is_rejected_before_planning() {
    let temp = TempTree::new("unresolved-timezone");
    let source = temp.path().join("source");
    let target = temp.path().join("target");
    fs::create_dir_all(source.join("boot")).unwrap();
    fs::create_dir_all(&target).unwrap();

    let mut manifest = ext4_manifest();
    manifest.os.timezone = crate::manifest::Timezone::Prompt;

    let error = plan_system_install(&manifest, &source, &target, None).unwrap_err();
    assert!(
        error.to_string().contains("timezone was not collected"),
        "got: {error}"
    );
}

#[test]
fn unknown_timezone_is_rejected_before_planning() {
    let temp = TempTree::new("unknown-timezone");
    let source = temp.path().join("source");
    let target = temp.path().join("target");
    fs::create_dir_all(source.join("boot")).unwrap();
    fs::create_dir_all(source.join("usr/share/zoneinfo/Europe")).unwrap();
    fs::write(source.join("usr/share/zoneinfo/Europe/London"), "TZif").unwrap();
    fs::create_dir_all(&target).unwrap();

    let mut manifest = ext4_manifest();
    manifest.os.timezone = "Europe/Lodnon".into();

    let error = plan_system_install(&manifest, &source, &target, None).unwrap_err();
    assert!(
        error
            .to_string()
            .contains("unknown timezone \"Europe/Lodnon\""),
        "got: {error}"
    );
}

#[test]
fn declared_timezone_adds_a_configure_step_after_hostname() {
    let temp = TempTree::new("timezone-step");
    let source = temp.path().join("source");
    let target = temp.path().join("target");
    fs::create_dir_all(source.join("boot")).unwrap();
    fs::create_dir_all(source.join("usr/share/zoneinfo/Europe")).unwrap();
    fs::write(source.join("usr/share/zoneinfo/Europe/London"), "TZif").unwrap();
    fs::create_dir_all(&target).unwrap();

    let mut manifest = ext4_manifest();
    manifest.os.timezone = "Europe/London".into();

    let plan = plan_system_install(&manifest, &source, &target, None).unwrap();
    let hostname_at = plan
        .steps
        .iter()
        .position(|step| matches!(step, SystemInstallStep::ConfigureHostname { .. }))
        .expect("hostname step present");
    let timezone_at = plan
        .steps
        .iter()
        .position(|step| {
            matches!(step, SystemInstallStep::ConfigureTimezone { timezone, .. } if timezone == "Europe/London")
        })
        .expect("timezone step present");
    assert!(hostname_at < timezone_at);
}

#[test]
fn empty_timezone_adds_no_configure_step() {
    let temp = TempTree::new("no-timezone-step");
    let source = temp.path().join("source");
    let target = temp.path().join("target");
    fs::create_dir_all(source.join("boot")).unwrap();
    fs::create_dir_all(&target).unwrap();

    let plan = plan_system_install(&ext4_manifest(), &source, &target, None).unwrap();
    assert!(
        !plan
            .steps
            .iter()
            .any(|step| matches!(step, SystemInstallStep::ConfigureTimezone { .. }))
    );
}

#[test]
fn unsupported_locale_is_rejected_before_planning() {
    let temp = TempTree::new("unsupported-locale");
    let source = temp.path().join("source");
    let target = temp.path().join("target");
    fs::create_dir_all(source.join("boot")).unwrap();
    fs::create_dir_all(source.join("usr/share/i18n")).unwrap();
    fs::write(
        source.join("usr/share/i18n/SUPPORTED"),
        "en_US.UTF-8 UTF-8\n",
    )
    .unwrap();
    fs::create_dir_all(&target).unwrap();

    let mut manifest = ext4_manifest();
    manifest.os.locale = "not_A_LOCALE.UTF-8".to_owned();

    let error = plan_system_install(&manifest, &source, &target, None).unwrap_err();
    assert!(
        error
            .to_string()
            .contains("unsupported locale \"not_A_LOCALE.UTF-8\""),
        "got: {error}"
    );
}

#[test]
fn declared_locale_adds_a_configure_step_after_timezone() {
    let temp = TempTree::new("locale-step");
    let source = temp.path().join("source");
    let target = temp.path().join("target");
    fs::create_dir_all(source.join("boot")).unwrap();
    fs::create_dir_all(source.join("usr/share/zoneinfo/Europe")).unwrap();
    fs::write(source.join("usr/share/zoneinfo/Europe/London"), "TZif").unwrap();
    fs::create_dir_all(source.join("usr/share/i18n")).unwrap();
    fs::write(
        source.join("usr/share/i18n/SUPPORTED"),
        "en_US.UTF-8 UTF-8\n",
    )
    .unwrap();
    fs::create_dir_all(&target).unwrap();

    let mut manifest = ext4_manifest();
    manifest.os.timezone = "Europe/London".into();
    manifest.os.locale = "en_US.UTF-8".to_owned();

    let plan = plan_system_install(&manifest, &source, &target, None).unwrap();
    let timezone_at = plan
        .steps
        .iter()
        .position(|step| matches!(step, SystemInstallStep::ConfigureTimezone { .. }))
        .expect("timezone step present");
    let locale_at = plan
        .steps
        .iter()
        .position(|step| {
            matches!(step, SystemInstallStep::ConfigureLocale { locale, .. } if locale == "en_US.UTF-8")
        })
        .expect("locale step present");
    assert!(timezone_at < locale_at);
}

#[test]
fn empty_locale_adds_no_configure_step() {
    let temp = TempTree::new("no-locale-step");
    let source = temp.path().join("source");
    let target = temp.path().join("target");
    fs::create_dir_all(source.join("boot")).unwrap();
    fs::create_dir_all(&target).unwrap();

    let plan = plan_system_install(&ext4_manifest(), &source, &target, None).unwrap();
    assert!(
        !plan
            .steps
            .iter()
            .any(|step| matches!(step, SystemInstallStep::ConfigureLocale { .. }))
    );
}

#[test]
fn write_timezone_links_localtime_and_records_the_zone() {
    let temp = TempTree::new("write-timezone");
    let target = temp.path().join("target");
    fs::create_dir_all(target.join("usr/share/zoneinfo/Europe")).unwrap();
    fs::write(target.join("usr/share/zoneinfo/Europe/London"), "TZif").unwrap();
    // The rsync'd live root brings its own localtime along; it must be replaced.
    fs::create_dir_all(target.join("etc")).unwrap();
    fs::write(target.join("etc/localtime"), "stale UTC").unwrap();

    super::super::super::filesystem::write_timezone("Europe/London", &target).unwrap();

    let recorded = fs::read_to_string(target.join("etc/timezone")).unwrap();
    assert_eq!(recorded, "Europe/London\n");
    let link = fs::read_link(target.join("etc/localtime")).unwrap();
    assert_eq!(link, PathBuf::from("../usr/share/zoneinfo/Europe/London"));

    let err = super::super::super::filesystem::write_timezone("Europe/Lodnon", &target)
        .expect_err("unknown zone must fail");
    assert!(err.to_string().contains("Europe/Lodnon"), "got: {err}");
}

#[test]
fn write_locale_enables_catalogue_entry_and_sets_lang() {
    let temp = TempTree::new("write-locale");
    let target = temp.path().join("target");
    fs::create_dir_all(target.join("usr/share/i18n")).unwrap();
    fs::write(
        target.join("usr/share/i18n/SUPPORTED"),
        "en_GB.UTF-8 UTF-8\nen_US.UTF-8 UTF-8\n",
    )
    .unwrap();

    super::super::super::filesystem::write_locale("en_US.UTF-8", &target).unwrap();

    assert_eq!(
        fs::read_to_string(target.join("etc/locale.gen")).unwrap(),
        "# generated by oxys\nen_US.UTF-8 UTF-8\n"
    );
    assert_eq!(
        fs::read_to_string(target.join("etc/env.d/02locale")).unwrap(),
        "# generated by oxys\nLANG=\"en_US.UTF-8\"\n"
    );
}

#[test]
fn seed_oxys_config_step_carries_source_and_precedes_finalize() {
    let temp = TempTree::new("seed-oxys-config");
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
    let config = temp.path().join("config.fe2o3");

    let plan = plan_system_install(&manifest, &source, &target, Some(&config)).unwrap();
    let seed_idx = plan
            .steps
            .iter()
            .position(|step| {
                matches!(step, SystemInstallStep::SeedOxysConfig { source_fe2o3: Some(path), .. } if path == &config)
            })
            .expect("seed step with source path missing");
    let finalize_idx = plan
        .steps
        .iter()
        .position(|step| matches!(step, SystemInstallStep::Finalize { .. }))
        .expect("finalize step missing");
    assert!(seed_idx < finalize_idx, "seed must run before finalize");
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

    let plan = plan_system_install(&manifest, &source, &target, None).unwrap();
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
        swap: crate::manifest::Swap {
            strategy: crate::manifest::SwapStrategy::Disabled,
            swappiness: 180,
        },
        ..SystemManifest::default()
    };

    let plan = plan_system_install(&manifest, &source, &target, None).unwrap();
    assert!(
        !plan
            .steps
            .iter()
            .any(|step| matches!(step, SystemInstallStep::EmergePackages { .. }))
    );
}

#[test]
fn swap_policy_is_resolved_and_configured_before_service_activation() {
    let temp = TempTree::new("swap-plan");
    let source = temp.path().join("source");
    let target = temp.path().join("target");
    fs::create_dir_all(source.join("boot")).unwrap();
    fs::create_dir_all(&target).unwrap();
    let manifest = SystemManifest {
        disk: Disk {
            device: "/dev/vda".to_owned(),
            ..Disk::default()
        },
        swap: crate::manifest::Swap {
            strategy: crate::manifest::SwapStrategy::Hybrid {
                zram: crate::manifest::ZramOptions::default(),
                disk: crate::manifest::SwapDiskOptions {
                    size: crate::manifest::SwapSize::Fixed(4 * GB),
                },
            },
            swappiness: 180,
        },
        services: crate::manifest::Services {
            enabled: vec!["sshd".to_owned()],
            disabled: Vec::new(),
            ..Default::default()
        },
        ..SystemManifest::default()
    };

    let plan = plan_system_install(&manifest, &source, &target, None).unwrap();
    assert_eq!(plan.resolved_swap.zram.as_ref().unwrap().priority, 100);
    assert_eq!(plan.resolved_swap.disk.as_ref().unwrap().priority, 10);
    let swap_idx = plan
        .steps
        .iter()
        .position(|step| matches!(step, SystemInstallStep::ConfigureSwap { .. }))
        .unwrap();
    let service_idx = plan
        .steps
        .iter()
        .position(|step| matches!(step, SystemInstallStep::ActivateOpenrcServices { .. }))
        .unwrap();
    assert!(swap_idx < service_idx);
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
    let niri = fs::read_to_string(target.join("home/testuser/.config/niri/config.kdl")).unwrap();
    assert!(niri.contains("exec gentoo-pipewire-launcher"));
    assert!(niri.contains("until noctalia; do sleep 2; done"));
    assert!(niri.contains("spawn-at-startup \"/usr/local/bin/oxys-welcome-once\""));
    assert!(niri.contains("match app-id=\"^oxys-welcome$\""));
    let welcome = target.join("usr/local/bin/oxys-welcome-once");
    let welcome_script = fs::read_to_string(&welcome).unwrap();
    assert!(welcome_script.contains("${XDG_STATE_HOME:-${HOME}/.local/state}"));
    assert!(welcome_script.contains("oxys/welcome-v1"));
    assert!(welcome_script.contains("/usr/bin/oxys welcome"));
    assert_eq!(
        std::os::unix::fs::PermissionsExt::mode(&fs::metadata(welcome).unwrap().permissions())
            & 0o777,
        0o755
    );
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

    let plan = plan_system_install(&manifest, &source, &target, None).unwrap();
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
