use super::*;

#[test]
fn explicit_use_flags_that_fight_manifest_policy_are_reported_not_overridden()
-> Result<(), Box<dyn std::error::Error>> {
    let root = test_root("explicit_vs_policy_conflict");
    let portage_tree = root.join("repo");
    let cache_dir = root.join("cache");

    write_md5_cache(
        &portage_tree,
        "app-admin/example",
        "1.0.0",
        "IUSE=systemd openrc wayland X pipewire pulseaudio\nKEYWORDS=amd64\n",
    )?;

    let manifest = SystemManifest {
        init_system: InitSystem::Systemd,
        display_stack: Some(DisplayStack::Wayland),
        audio_stack: Some(AudioStack::Pipewire),
        packages: vec![Package::new("app-admin/example").use_flags(["openrc", "X", "pulseaudio"])],
        ..SystemManifest::default()
    };

    let plan = plan_portage(&manifest, &portage_tree, &cache_dir)?;
    let package_use = plan
        .resolution
        .package_use
        .get("app-admin/example")
        .ok_or("missing package.use entry")?;

    // The explicit user flags win (package.use precedence); manifest policy does
    // not silently override them.
    assert!(package_use.contains(&"openrc".to_owned()));
    assert!(package_use.contains(&"X".to_owned()));
    assert!(package_use.contains(&"pulseaudio".to_owned()));
    // Policy still fills in the flags the user did not pin.
    assert!(package_use.contains(&"systemd".to_owned()));
    assert!(package_use.contains(&"wayland".to_owned()));
    assert!(package_use.contains(&"pipewire".to_owned()));

    // Init/audio disagreements are surfaced as hard conflicts. X support is
    // compatible with a Wayland-first desktop and is intentionally retained.
    let conflicts = &plan.resolution.conflicts;
    let init_conflict = conflicts
        .iter()
        .find(|conflict| conflict.flag == "openrc")
        .ok_or("missing openrc policy conflict")?;
    assert!(init_conflict.reason.contains("app-admin/example"));
    assert!(init_conflict.reason.contains("+openrc"));
    assert!(init_conflict.reason.contains("init_system = systemd"));
    assert!(
        init_conflict
            .packages
            .contains(&"app-admin/example".to_owned())
    );

    assert!(!conflicts.iter().any(|conflict| conflict.flag == "X"));
    assert!(
        conflicts
            .iter()
            .any(|conflict| conflict.flag == "pulseaudio"
                && conflict.reason.contains("audio_stack = pipewire"))
    );

    cleanup(&root)?;
    Ok(())
}

#[test]
fn explicit_init_flag_vs_policy_reports_one_conflict_not_two()
-> Result<(), Box<dyn std::error::Error>> {
    let root = test_root("single_conflict_for_init_clash");
    let portage_tree = root.join("repo");
    let cache_dir = root.join("cache");

    write_md5_cache(
        &portage_tree,
        "app-admin/example",
        "1.0.0",
        "IUSE=systemd openrc\nKEYWORDS=amd64\n",
    )?;

    // OpenRC manifest, but the package pins +systemd. The specific
    // explicit-vs-policy conflict and the generic "both systemd and openrc
    // remain enabled" conflict describe the same root cause, so only the
    // specific one should be reported.
    let manifest = SystemManifest {
        init_system: InitSystem::Openrc,
        packages: vec![Package::new("app-admin/example").use_flags(["systemd"])],
        ..SystemManifest::default()
    };

    let plan = plan_portage(&manifest, &portage_tree, &cache_dir)?;
    let conflicts = &plan.resolution.conflicts;

    assert_eq!(
        conflicts.len(),
        1,
        "expected exactly one conflict, got: {conflicts:?}"
    );
    assert_eq!(conflicts[0].flag, "systemd");
    assert!(conflicts[0].reason.contains("init_system = openrc"));
    assert!(
        !conflicts
            .iter()
            .any(|conflict| conflict.flag == "systemd/openrc"),
        "generic dual-init conflict should be suppressed by the specific one"
    );

    cleanup(&root)?;
    Ok(())
}

#[test]
fn binary_packages_do_not_emit_package_use_flags() -> Result<(), Box<dyn std::error::Error>> {
    let root = test_root("binary_packages_skip_use");
    let portage_tree = root.join("repo");
    let cache_dir = root.join("cache");

    write_md5_cache(
        &portage_tree,
        "www-client/firefox-bin",
        "128.0.1",
        "IUSE=wayland X pulseaudio\nKEYWORDS=amd64\n",
    )?;

    let manifest = SystemManifest {
        packages: vec![Package::new("www-client/firefox-bin").use_flags(["wayland", "-X"])],
        ..SystemManifest::default()
    };

    let plan = plan_portage(&manifest, &portage_tree, &cache_dir)?;
    let package_use = plan
        .resolution
        .package_use
        .get("www-client/firefox-bin")
        .ok_or("missing package.use entry")?;

    assert!(package_use.is_empty());
    // use_flags on a binary-resolved package (via -bin name) now produces hard conflict
    // (plan still computed with stripped USEs for the binary case)
    assert!(plan.resolution.conflicts.iter().any(|c| {
        c.packages.contains(&"www-client/firefox-bin".to_string())
            && c.reason
                .contains("use_flags set but package will install from binary")
    }));

    cleanup(&root)?;
    Ok(())
}

#[test]
fn from_source_with_use_flags_and_global_prefer_binary_resolves_to_source_and_applies_flags()
-> Result<(), Box<dyn std::error::Error>> {
    let root = test_root("from_source_use_flags");
    let portage_tree = root.join("repo");
    let cache_dir = root.join("cache");

    write_md5_cache(
        &portage_tree,
        "gui-wm/niri",
        "25.11-r1",
        "IUSE=+wayland screencast\nKEYWORDS=amd64\n",
    )?;

    let manifest = SystemManifest {
        prefer_binary: true,
        packages: vec![
            Package::new("gui-wm/niri")
                .from_source()
                .use_flags(["screencast"]),
        ],
        ..SystemManifest::default()
    };

    let plan = plan_portage(&manifest, &portage_tree, &cache_dir)?;

    assert!(plan.resolution.conflicts.is_empty());
    let use_flags = plan
        .resolution
        .package_use
        .get("gui-wm/niri")
        .ok_or("missing niri use")?;
    // custom flag applied, and default wayland remains enabled
    assert!(use_flags.contains(&"screencast".to_string()));
    assert!(use_flags.contains(&"wayland".to_string()));
    assert!(!plan.use_binpkgs); // only source package (from_source override) => no binpkg flags

    // prove affects what would be passed to emerge
    let argv = emerge_command_for_test(
        &plan.targets,
        std::path::Path::new("/"),
        1,
        plan.use_binpkgs,
        true,
    );
    assert!(!argv.contains(&"--getbinpkg".to_string()));
    assert!(!argv.contains(&"--usepkg".to_string()));

    cleanup(&root)?;
    Ok(())
}

#[test]
fn use_flags_without_from_source_and_global_prefer_binary_falls_back_to_source_with_warning()
-> Result<(), Box<dyn std::error::Error>> {
    let root = test_root("use_flags_no_from_source_fallback");
    let portage_tree = root.join("repo");
    let cache_dir = root.join("cache");

    write_md5_cache(
        &portage_tree,
        "gui-wm/niri",
        "25.11-r1",
        "IUSE=+wayland screencast\nKEYWORDS=amd64\n",
    )?;

    let manifest = SystemManifest {
        prefer_binary: true,
        packages: vec![Package::new("gui-wm/niri").use_flags(["screencast"])],
        ..SystemManifest::default()
    };

    let plan = plan_portage(&manifest, &portage_tree, &cache_dir)?;

    // No hard conflict: a package that's only binary because of the *global*
    // prefer_binary policy (not an explicit .binary()/-bin pin) quietly falls
    // back to building from source when it carries custom use_flags, instead
    // of blocking the whole plan.
    assert!(plan.resolution.conflicts.is_empty());
    assert!(plan.resolution.warnings.iter().any(|warning| {
        warning.package == "gui-wm/niri" && warning.message.contains("falling back to source")
    }));

    let use_flags = plan
        .resolution
        .package_use
        .get("gui-wm/niri")
        .ok_or("missing niri use")?;
    assert!(use_flags.contains(&"screencast".to_string()));
    assert!(use_flags.contains(&"wayland".to_string()));

    cleanup(&root)?;
    Ok(())
}

#[test]
fn no_from_source_no_use_flags_global_prefer_binary_resolves_binary_happy_path()
-> Result<(), Box<dyn std::error::Error>> {
    let root = test_root("prefer_binary_no_flags");
    let portage_tree = root.join("repo");
    let cache_dir = root.join("cache");

    write_md5_cache(
        &portage_tree,
        "gui-wm/niri",
        "25.11-r1",
        "IUSE=+wayland\nKEYWORDS=amd64\n",
    )?;

    let manifest = SystemManifest {
        prefer_binary: true,
        packages: vec![Package::new("gui-wm/niri")],
        ..SystemManifest::default()
    };

    let plan = plan_portage(&manifest, &portage_tree, &cache_dir)?;

    assert!(plan.resolution.conflicts.is_empty());
    // for binary resolved, package_use has only l10n or is present but stripped to non-custom
    // here since no use requested, and defaults kept? but for binary resolved, available only l10n so only those if any
    let uses = plan
        .resolution
        .package_use
        .get("gui-wm/niri")
        .map(|v| v.as_slice())
        .unwrap_or(&[]);
    // no non-l10n, so either absent or empty-ish
    assert!(uses.iter().all(
        |f| f.starts_with("l10n_") || f == "-wayland" /*but wayland stripped*/
    ));
    assert!(plan.use_binpkgs);

    let argv = emerge_command_for_test(
        &plan.targets,
        std::path::Path::new("/"),
        2,
        plan.use_binpkgs,
        true,
    );
    assert!(argv.contains(&"--getbinpkg".to_string()));
    assert!(argv.contains(&"--usepkg".to_string()));

    cleanup(&root)?;
    Ok(())
}

#[test]
fn emerge_chroot_command_uses_target_root_and_binpkg_flags()
-> Result<(), Box<dyn std::error::Error>> {
    let argv = emerge_chroot_command_for_test(
        &[
            "=gui-wm/niri-25.11-r1".to_owned(),
            "=gui-shells/noctalia-9999".to_owned(),
        ],
        Path::new("/mnt/gentoo"),
        Path::new("/var/tmp"),
        2,
        true,
    );

    assert_eq!(
        argv,
        vec![
            "chroot",
            "/mnt/gentoo",
            "env",
            "PORTAGE_TMPDIR=/var/tmp",
            "emerge",
            "--root",
            "/",
            "--update",
            "--changed-use",
            "--jobs",
            "2",
            "--getbinpkg",
            "--usepkg",
            "=gui-wm/niri-25.11-r1",
            "=gui-shells/noctalia-9999",
        ]
    );

    Ok(())
}

#[test]
fn from_source_no_use_flags_resolves_to_source_with_defaults_no_error()
-> Result<(), Box<dyn std::error::Error>> {
    let root = test_root("from_source_no_flags");
    let portage_tree = root.join("repo");
    let cache_dir = root.join("cache");

    write_md5_cache(
        &portage_tree,
        "gui-wm/niri",
        "25.11-r1",
        "IUSE=+wayland foo\nKEYWORDS=amd64\n",
    )?;

    let manifest = SystemManifest {
        prefer_binary: false,
        packages: vec![Package::new("gui-wm/niri").from_source()],
        ..SystemManifest::default()
    };

    let plan = plan_portage(&manifest, &portage_tree, &cache_dir)?;

    assert!(plan.resolution.conflicts.is_empty());
    let uses = plan
        .resolution
        .package_use
        .get("gui-wm/niri")
        .ok_or("missing")?;
    // defaults rendered: wayland enabled (no -), foo disabled as -foo
    assert!(uses.contains(&"wayland".to_string()));
    assert!(uses.contains(&"-foo".to_string()));
    assert!(!plan.use_binpkgs);

    let argv = emerge_command_for_test(
        &plan.targets,
        std::path::Path::new("/"),
        1,
        plan.use_binpkgs,
        true,
    );
    assert!(!argv.contains(&"--getbinpkg".to_string()));

    cleanup(&root)?;
    Ok(())
}

#[test]
fn resolve_latest_version_excludes_live_9999_versions() -> Result<(), Box<dyn std::error::Error>> {
    let root = test_root("exclude_live_versions");
    let portage_tree = root.join("repo");

    write_md5_cache(
        &portage_tree,
        "gui-wm/niri",
        "25.11.0",
        "IUSE=\nKEYWORDS=amd64\n",
    )?;
    write_md5_cache(
        &portage_tree,
        "gui-wm/niri",
        "9999",
        "IUSE=\nKEYWORDS=amd64\n",
    )?;
    write_md5_cache(
        &portage_tree,
        "gui-wm/niri",
        "1.6.9999",
        "IUSE=\nKEYWORDS=amd64\n",
    )?;

    let version = resolve_latest_version("gui-wm/niri", &portage_tree)?;

    assert_eq!(version, "25.11.0");

    cleanup(&root)?;
    Ok(())
}

#[test]
fn resolve_latest_version_uses_live_version_when_it_is_the_only_choice()
-> Result<(), Box<dyn std::error::Error>> {
    let root = test_root("only_live_version");
    let portage_tree = root.join("repo");

    write_md5_cache(
        &portage_tree,
        "gui-shells/noctalia",
        "9999",
        "IUSE=+jemalloc\nKEYWORDS=\n",
    )?;

    let version = resolve_latest_version("gui-shells/noctalia", &portage_tree)?;

    assert_eq!(version, "9999");

    cleanup(&root)?;
    Ok(())
}
