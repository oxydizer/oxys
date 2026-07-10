use std::{fs, path::Path};

use oxys::manifest::{AudioStack, DisplayStack, InitSystem, Libc, Os, Package, SystemManifest};
use oxys::use_resolver::{
    emerge_chroot_command_for_test, emerge_command_for_test, plan_portage, resolve_latest_version,
    write_portage_plan_config, DecisionAction, DecisionScope, DecisionSource,
};

#[test]
fn resolves_versions_from_portage_tree_when_package_version_is_omitted(
) -> Result<(), Box<dyn std::error::Error>> {
    let root = test_root("resolve_latest_version");
    let portage_tree = root.join("repo");
    let cache_dir = root.join("cache");

    write_md5_cache(
        &portage_tree,
        "app-admin/example",
        "0.9.0",
        "IUSE=+openrc +elibc_glibc\nKEYWORDS=amd64\n",
    )?;
    write_md5_cache(
        &portage_tree,
        "app-admin/example",
        "1.0.0",
        "IUSE=+openrc +elibc_glibc\nKEYWORDS=~amd64\n",
    )?;

    let manifest = SystemManifest {
        os: Os {
            libc: Libc::Glibc,
            ..Os::default()
        },
        packages: vec![Package::new("app-admin/example")],
        ..SystemManifest::default()
    };

    let plan = plan_portage(&manifest, &portage_tree, &cache_dir)?;
    let package_use = plan
        .resolution
        .package_use
        .get("app-admin/example")
        .ok_or("missing package.use entry")?;

    assert!(package_use.contains(&"elibc_glibc".to_owned()));
    assert_eq!(plan.targets, vec!["=app-admin/example-1.0.0".to_owned()]);

    assert!(plan.resolution.decisions.iter().any(|decision| {
        decision.scope == DecisionScope::PlannerPolicy
            && decision.subject == "libc"
            && decision.action == DecisionAction::Note
            && decision.source == DecisionSource::ManifestPolicy
            && decision.reason.contains("manifest libc policy is glibc")
    }));
    assert!(plan.resolution.decisions.iter().any(|decision| {
        decision.scope == DecisionScope::AcceptKeywords
            && decision.subject == "~amd64"
            && decision.source == DecisionSource::Metadata
    }));

    cleanup(&root)?;
    Ok(())
}

#[test]
fn defaults_init_and_infers_audio_when_manifest_policy_is_absent(
) -> Result<(), Box<dyn std::error::Error>> {
    let root = test_root("fallback_inference");
    let portage_tree = root.join("repo");
    let cache_dir = root.join("cache");

    write_md5_cache(
        &portage_tree,
        "app-admin/example",
        "1.0.0",
        "IUSE=systemd +openrc pulseaudio +pipewire\nKEYWORDS=amd64\n",
    )?;
    write_md5_cache(
        &portage_tree,
        "sys-apps/systemd",
        "255",
        "IUSE=\nKEYWORDS=amd64\n",
    )?;
    write_md5_cache(
        &portage_tree,
        "media-video/pipewire",
        "1.2.0",
        "IUSE=\nKEYWORDS=amd64\n",
    )?;

    let manifest = SystemManifest {
        packages: vec![
            Package::new("app-admin/example"),
            Package::new("sys-apps/systemd"),
            Package::new("media-video/pipewire"),
        ],
        ..SystemManifest::default()
    };

    let plan = plan_portage(&manifest, &portage_tree, &cache_dir)?;
    let package_use = plan
        .resolution
        .package_use
        .get("app-admin/example")
        .ok_or("missing package.use entry")?;

    // `init_system` is a required manifest field, so an unset manifest resolves
    // to the default (openrc) instead of inferring from selected packages.
    assert!(package_use.contains(&"openrc".to_owned()));
    assert!(package_use.contains(&"-systemd".to_owned()));
    // `audio_stack` is still optional, so it is inferred from the packages.
    assert!(package_use.contains(&"pipewire".to_owned()));
    assert!(package_use.contains(&"-pulseaudio".to_owned()));
    assert!(plan
        .resolution
        .decisions
        .iter()
        .any(|decision| { decision.reason.contains("manifest init system is openrc") }));
    assert!(plan.resolution.decisions.iter().any(|decision| {
        decision.source == DecisionSource::PackageInference
            && decision
                .reason
                .contains("fallback package inference selected pipewire")
    }));

    cleanup(&root)?;
    Ok(())
}

#[test]
fn apply_facing_generation_is_deterministic() -> Result<(), Box<dyn std::error::Error>> {
    let root = test_root("apply_generation");
    let portage_tree = root.join("repo");
    let cache_dir = root.join("cache");
    let config_dir = root.join("etc/portage");

    write_md5_cache(
        &portage_tree,
        "gui-wm/niri",
        "25.11-r1",
        "IUSE=+wayland +X\nKEYWORDS=~amd64\n",
    )?;
    write_md5_cache(
        &portage_tree,
        "gui-apps/waybar",
        "0.10.3",
        "IUSE=+wayland\nKEYWORDS=~amd64\n",
    )?;

    let manifest = SystemManifest {
        packages: vec![Package::new("gui-wm/niri"), Package::new("gui-apps/waybar")],
        ..SystemManifest::default()
    };

    let first = plan_portage(&manifest, &portage_tree, &cache_dir)?;
    let second = plan_portage(&manifest, &portage_tree, &cache_dir)?;

    assert_eq!(first.targets, second.targets);
    assert_eq!(first.resolution.global_use, second.resolution.global_use);
    assert_eq!(
        first.resolution.accept_keywords,
        second.resolution.accept_keywords
    );
    assert_eq!(first.resolution.decisions, second.resolution.decisions);

    write_portage_plan_config(&first, &config_dir)?;

    assert_eq!(
        fs::read_to_string(config_dir.join("package.accept_keywords"))?,
        concat!(
            "# generated by oxys - do not edit manually\n",
            "~amd64 gui-apps/waybar\n",
            "~amd64 gui-wm/niri\n"
        )
    );
    assert_eq!(
        fs::read_to_string(config_dir.join("package.use").join("oxys"))?,
        concat!(
            "# generated by oxys - do not edit manually\n",
            "gui-apps/waybar wayland\n",
            "gui-wm/niri -X wayland\n"
        )
    );
    assert!(first.targets.iter().all(|target| target.starts_with('=')));

    // Bare-default manifest: no explicit `display_stack`, so wayland-vs-X is
    // resolved per package via IUSE-default inference only. That inference
    // must not additionally strip X globally -- doing so would silently move
    // every bare-default install off Gentoo's binhost (which ships desktop
    // packages with X enabled alongside wayland) for no explicit reason.
    assert!(!first.resolution.global_use.contains(&"-X".to_owned()));

    cleanup(&root)?;
    Ok(())
}

#[test]
fn explicit_wayland_display_stack_disables_global_x() -> Result<(), Box<dyn std::error::Error>> {
    let root = test_root("explicit_wayland_disables_global_x");
    let portage_tree = root.join("repo");
    let cache_dir = root.join("cache");

    write_md5_cache(
        &portage_tree,
        "gui-wm/niri",
        "25.11-r1",
        "IUSE=+wayland +X\nKEYWORDS=~amd64\n",
    )?;

    let manifest = SystemManifest {
        display_stack: Some(DisplayStack::Wayland),
        packages: vec![Package::new("gui-wm/niri")],
        ..SystemManifest::default()
    };

    let plan = plan_portage(&manifest, &portage_tree, &cache_dir)?;

    // With an explicit manifest policy, global X is still disabled as before.
    assert!(plan.resolution.global_use.contains(&"-X".to_owned()));

    cleanup(&root)?;
    Ok(())
}

#[test]
fn llvm_slot_fallback_selects_highest_available_slot() -> Result<(), Box<dyn std::error::Error>> {
    let root = test_root("llvm_slot_highest_fallback");
    let portage_tree = root.join("repo");
    let cache_dir = root.join("cache");

    write_md5_cache(
        &portage_tree,
        "gui-wm/niri",
        "25.11-r1",
        "IUSE=llvm_slot_18 llvm_slot_20 llvm_slot_19\nKEYWORDS=amd64\n",
    )?;

    let manifest = SystemManifest {
        packages: vec![Package::new("gui-wm/niri")],
        ..SystemManifest::default()
    };

    let plan = plan_portage(&manifest, &portage_tree, &cache_dir)?;
    let package_use = plan
        .resolution
        .package_use
        .get("gui-wm/niri")
        .ok_or("missing package.use entry")?;

    assert!(package_use.contains(&"llvm_slot_20".to_owned()));
    assert!(package_use.contains(&"-llvm_slot_18".to_owned()));
    assert!(package_use.contains(&"-llvm_slot_19".to_owned()));
    assert!(plan.resolution.decisions.iter().any(|decision| {
        decision.reason == "selected highest available llvm slot llvm_slot_20"
    }));

    cleanup(&root)?;
    Ok(())
}

#[test]
fn resolve_latest_version_uses_gentoo_style_version_ordering(
) -> Result<(), Box<dyn std::error::Error>> {
    let root = test_root("gentoo_version_ordering");
    let portage_tree = root.join("repo");

    write_md5_cache(
        &portage_tree,
        "gui-wm/niri",
        "1.9.0",
        "IUSE=\nKEYWORDS=amd64\n",
    )?;
    write_md5_cache(
        &portage_tree,
        "gui-wm/niri",
        "1.10.0",
        "IUSE=\nKEYWORDS=amd64\n",
    )?;
    write_md5_cache(
        &portage_tree,
        "gui-wm/niri",
        "1.10.0-r1",
        "IUSE=\nKEYWORDS=amd64\n",
    )?;

    let version = resolve_latest_version("gui-wm/niri", &portage_tree)?;

    assert_eq!(version, "1.10.0-r1");

    cleanup(&root)?;
    Ok(())
}

#[test]
fn blank_manifest_version_falls_back_to_latest_resolution() -> Result<(), Box<dyn std::error::Error>>
{
    let root = test_root("blank_manifest_version");
    let portage_tree = root.join("repo");
    let cache_dir = root.join("cache");

    write_md5_cache(
        &portage_tree,
        "gui-wm/niri",
        "25.11-r1",
        "IUSE=+wayland\nKEYWORDS=amd64\n",
    )?;

    let manifest = toml::from_str::<SystemManifest>(
        r#"
            [[packages]]
            package = "gui-wm/niri"
            version = ""
        "#,
    )?;

    let plan = plan_portage(&manifest, &portage_tree, &cache_dir)?;

    assert_eq!(plan.targets, vec!["=gui-wm/niri-25.11-r1".to_owned()]);

    cleanup(&root)?;
    Ok(())
}

#[test]
fn resolve_latest_version_searches_across_repo_root_children(
) -> Result<(), Box<dyn std::error::Error>> {
    let root = test_root("multi_repo_root");
    let repos_root = root.join("repos");
    let gentoo_repo = repos_root.join("gentoo");
    let overlay_repo = repos_root.join("guru");

    write_md5_cache(
        &gentoo_repo,
        "gui-wm/niri",
        "25.10.0",
        "IUSE=\nKEYWORDS=amd64\n",
    )?;
    write_md5_cache(
        &overlay_repo,
        "gui-wm/niri",
        "25.11.0",
        "IUSE=\nKEYWORDS=amd64\n",
    )?;

    let version = resolve_latest_version("gui-wm/niri", &repos_root)?;

    assert_eq!(version, "25.11.0");

    cleanup(&root)?;
    Ok(())
}

#[test]
fn explicit_use_flags_that_fight_manifest_policy_are_reported_not_overridden(
) -> Result<(), Box<dyn std::error::Error>> {
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

    // Each disagreement is surfaced as a hard conflict that names the package,
    // the explicit flag token, and the manifest field to edit.
    let conflicts = &plan.resolution.conflicts;
    let init_conflict = conflicts
        .iter()
        .find(|conflict| conflict.flag == "openrc")
        .ok_or("missing openrc policy conflict")?;
    assert!(init_conflict.reason.contains("app-admin/example"));
    assert!(init_conflict.reason.contains("+openrc"));
    assert!(init_conflict.reason.contains("init_system = systemd"));
    assert!(init_conflict
        .packages
        .contains(&"app-admin/example".to_owned()));

    assert!(conflicts.iter().any(
        |conflict| conflict.flag == "X" && conflict.reason.contains("display_stack = wayland")
    ));
    assert!(conflicts
        .iter()
        .any(|conflict| conflict.flag == "pulseaudio"
            && conflict.reason.contains("audio_stack = pipewire")));

    cleanup(&root)?;
    Ok(())
}

#[test]
fn explicit_init_flag_vs_policy_reports_one_conflict_not_two(
) -> Result<(), Box<dyn std::error::Error>> {
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
fn from_source_with_use_flags_and_global_prefer_binary_resolves_to_source_and_applies_flags(
) -> Result<(), Box<dyn std::error::Error>> {
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
        packages: vec![Package::new("gui-wm/niri")
            .from_source()
            .use_flags(["screencast"])],
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
    );
    assert!(!argv.contains(&"--getbinpkg".to_string()));
    assert!(!argv.contains(&"--usepkg".to_string()));

    cleanup(&root)?;
    Ok(())
}

#[test]
fn use_flags_without_from_source_and_global_prefer_binary_falls_back_to_source_with_warning(
) -> Result<(), Box<dyn std::error::Error>> {
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
fn no_from_source_no_use_flags_global_prefer_binary_resolves_binary_happy_path(
) -> Result<(), Box<dyn std::error::Error>> {
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
    assert!(uses
        .iter()
        .all(|f| f.starts_with("l10n_") || f == "-wayland" /*but wayland stripped*/));
    assert!(plan.use_binpkgs);

    let argv = emerge_command_for_test(
        &plan.targets,
        std::path::Path::new("/"),
        2,
        plan.use_binpkgs,
    );
    assert!(argv.contains(&"--getbinpkg".to_string()));
    assert!(argv.contains(&"--usepkg".to_string()));

    cleanup(&root)?;
    Ok(())
}

#[test]
fn emerge_chroot_command_uses_target_root_and_binpkg_flags(
) -> Result<(), Box<dyn std::error::Error>> {
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
fn from_source_no_use_flags_resolves_to_source_with_defaults_no_error(
) -> Result<(), Box<dyn std::error::Error>> {
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
fn reports_virtual_slot_license_and_property_checks() -> Result<(), Box<dyn std::error::Error>> {
    let root = test_root("resolver_metadata_checks");
    let portage_tree = root.join("repo");
    let cache_dir = root.join("cache");

    write_md5_cache(
        &portage_tree,
        "app-shells/example",
        "1.0.0",
        concat!(
            "DEPEND=virtual/editor app-text/provider:0= dev-libs/libfoo:2\n",
            "PDEPEND=app-misc/post-step\n",
            "LICENSE=all-rights-reserved\n",
            "PROPERTIES=interactive live\n",
            "RESTRICT=mirror test\n",
            "KEYWORDS=**\n"
        ),
    )?;
    write_md5_cache(
        &portage_tree,
        "app-text/provider",
        "1.0.0",
        "PROVIDE=virtual/editor\nSLOT=0/1\nKEYWORDS=amd64\n",
    )?;
    write_md5_cache(
        &portage_tree,
        "dev-libs/libfoo",
        "1.0.0",
        "SLOT=1/4\nKEYWORDS=amd64\n",
    )?;
    write_md5_cache(
        &portage_tree,
        "app-misc/post-step",
        "1.0.0",
        "KEYWORDS=amd64\n",
    )?;

    let manifest = SystemManifest {
        packages: vec![
            Package::new("app-shells/example"),
            Package::new("app-text/provider"),
            Package::new("dev-libs/libfoo"),
        ],
        ..SystemManifest::default()
    };

    let plan = plan_portage(&manifest, &portage_tree, &cache_dir)?;

    assert!(plan.resolution.conflicts.iter().any(|conflict| {
        conflict.flag == "dev-libs/libfoo:2" && conflict.reason.contains("slot mismatch")
    }));
    assert!(plan.resolution.warnings.iter().any(|warning| {
        warning.package == "app-shells/example"
            && warning
                .message
                .contains("virtual dependency virtual/editor is satisfied")
    }));
    assert!(plan.resolution.warnings.iter().any(|warning| {
        warning.package == "app-shells/example"
            && warning.message.contains("require explicit accept_license")
    }));
    assert!(plan.resolution.warnings.iter().any(|warning| {
        warning.package == "app-shells/example" && warning.message.contains("interactive")
    }));
    assert!(plan.resolution.warnings.iter().any(|warning| {
        warning.package == "app-shells/example" && warning.message.contains("live")
    }));
    assert!(plan.resolution.warnings.iter().any(|warning| {
        warning.package == "app-shells/example" && warning.message.contains("RESTRICT=mirror")
    }));
    assert!(plan.resolution.warnings.iter().any(|warning| {
        warning.package == "app-shells/example" && warning.message.contains("RESTRICT=test")
    }));
    assert!(plan.resolution.warnings.iter().any(|warning| {
        warning.package == "app-shells/example" && warning.message.contains("PDEPEND")
    }));
    assert!(plan.resolution.warnings.iter().any(|warning| {
        warning.package == "app-shells/example" && warning.message.contains("keyworded **")
    }));

    cleanup(&root)?;
    Ok(())
}

#[test]
fn manifest_package_overrides_drive_keywords_and_licenses() -> Result<(), Box<dyn std::error::Error>>
{
    let root = test_root("manifest_keyword_license_overrides");
    let portage_tree = root.join("repo");
    let cache_dir = root.join("cache");

    write_md5_cache(
        &portage_tree,
        "gui-wm/niri",
        "25.11-r1",
        "LICENSE=all-rights-reserved\nKEYWORDS=**\n",
    )?;

    let manifest = SystemManifest {
        packages: vec![Package::new("gui-wm/niri")
            .keywords(["~amd64"])
            .accept_licenses(["all-rights-reserved"])],
        ..SystemManifest::default()
    };

    let plan = plan_portage(&manifest, &portage_tree, &cache_dir)?;

    assert!(plan
        .resolution
        .accept_keywords
        .iter()
        .any(|entry| entry == "~amd64 gui-wm/niri"));
    assert!(plan
        .resolution
        .accept_licenses
        .iter()
        .any(|entry| entry == "all-rights-reserved gui-wm/niri"));
    assert!(!plan.resolution.warnings.iter().any(|warning| {
        warning.package == "gui-wm/niri"
            && warning.message.contains("require explicit accept_license")
    }));

    cleanup(&root)?;
    Ok(())
}

#[test]
fn abi_consistency_conflict_when_binary_depends_on_modified_source(
) -> Result<(), Box<dyn std::error::Error>> {
    let root = test_root("abi_consistency_conflict");
    let portage_tree = root.join("repo");
    let cache_dir = root.join("cache");

    write_md5_cache(
        &portage_tree,
        "media-video/pipewire",
        "1.0.0",
        "IUSE=systemd\nKEYWORDS=amd64\n",
    )?;
    write_md5_cache(
        &portage_tree,
        "media-sound/wireplumber",
        "1.0.0",
        "IUSE=+systemd\nRDEPEND=media-video/pipewire\nKEYWORDS=amd64\n",
    )?;

    let manifest = SystemManifest {
        packages: vec![
            Package::new("media-video/pipewire")
                .from_source()
                .use_flags(["-systemd"]),
            Package::new("media-sound/wireplumber").binary(true),
        ],
        ..SystemManifest::default()
    };

    let plan = plan_portage(&manifest, &portage_tree, &cache_dir)?;
    let conflicts = &plan.resolution.conflicts;

    assert!(
        conflicts.iter().any(|c| {
            c.flag == "abi-consistency"
                && c.packages.contains(&"media-video/pipewire".to_owned())
                && c.packages.contains(&"media-sound/wireplumber".to_owned())
                && c.reason.contains("conflicting USE flags for 'systemd'")
                && c.reason
                    .contains("rebuild 'media-sound/wireplumber' from source")
        }),
        "expected abi-consistency conflict, got: {:?}",
        conflicts
    );

    cleanup(&root)?;
    Ok(())
}

#[test]
fn abi_consistency_treats_prefer_binary_dependents_as_binary(
) -> Result<(), Box<dyn std::error::Error>> {
    let root = test_root("abi_consistency_prefer_binary");
    let portage_tree = root.join("repo");
    let cache_dir = root.join("cache");

    write_md5_cache(
        &portage_tree,
        "media-video/pipewire",
        "1.0.0",
        "IUSE=systemd\nKEYWORDS=amd64\n",
    )?;
    write_md5_cache(
        &portage_tree,
        "media-sound/wireplumber",
        "1.0.0",
        "IUSE=+systemd\nRDEPEND=media-video/pipewire\nKEYWORDS=amd64\n",
    )?;

    let manifest = SystemManifest {
        prefer_binary: true,
        packages: vec![
            Package::new("media-video/pipewire")
                .from_source()
                .use_flags(["-systemd"]),
            Package::new("media-sound/wireplumber"),
        ],
        ..SystemManifest::default()
    };

    let plan = plan_portage(&manifest, &portage_tree, &cache_dir)?;
    let conflicts = &plan.resolution.conflicts;

    assert!(
        conflicts.iter().any(|c| {
            c.flag == "abi-consistency"
                && c.packages.contains(&"media-video/pipewire".to_owned())
                && c.packages.contains(&"media-sound/wireplumber".to_owned())
                && c.reason.contains("conflicting USE flags for 'systemd'")
        }),
        "expected abi-consistency conflict, got: {:?}",
        conflicts
    );

    cleanup(&root)?;
    Ok(())
}

#[test]
fn abi_consistency_no_conflict_when_no_binary_dependent() -> Result<(), Box<dyn std::error::Error>>
{
    let root = test_root("abi_consistency_no_binary_dep");
    let portage_tree = root.join("repo");
    let cache_dir = root.join("cache");

    write_md5_cache(
        &portage_tree,
        "media-video/pipewire",
        "1.0.0",
        "IUSE=systemd\nKEYWORDS=amd64\n",
    )?;

    let manifest = SystemManifest {
        packages: vec![Package::new("media-video/pipewire")
            .from_source()
            .use_flags(["-systemd"])],
        ..SystemManifest::default()
    };

    let plan = plan_portage(&manifest, &portage_tree, &cache_dir)?;
    let conflicts = &plan.resolution.conflicts;

    assert!(
        !conflicts.iter().any(|c| c.flag == "abi-consistency"),
        "expected no abi-consistency conflict, got: {:?}",
        conflicts
    );

    cleanup(&root)?;
    Ok(())
}

#[test]
fn abi_consistency_no_conflict_when_flags_not_affected() -> Result<(), Box<dyn std::error::Error>> {
    let root = test_root("abi_consistency_unaffected");
    let portage_tree = root.join("repo");
    let cache_dir = root.join("cache");

    write_md5_cache(
        &portage_tree,
        "media-video/pipewire",
        "1.0.0",
        "IUSE=systemd wayland\nKEYWORDS=amd64\n",
    )?;
    write_md5_cache(
        &portage_tree,
        "media-sound/wireplumber",
        "1.0.0",
        "IUSE=+systemd\nRDEPEND=media-video/pipewire\nKEYWORDS=amd64\n",
    )?;

    // wireplumber has no 'wayland' in IUSE or dependencies.
    // Changing 'wayland' on pipewire shouldn't trigger ABI consistency conflict on wireplumber.
    let manifest = SystemManifest {
        packages: vec![
            Package::new("media-video/pipewire")
                .from_source()
                .use_flags(["-wayland"]),
            Package::new("media-sound/wireplumber").binary(true),
        ],
        ..SystemManifest::default()
    };

    let plan = plan_portage(&manifest, &portage_tree, &cache_dir)?;
    let conflicts = &plan.resolution.conflicts;

    assert!(
        !conflicts.iter().any(|c| c.flag == "abi-consistency"),
        "expected no abi-consistency conflict, got: {:?}",
        conflicts
    );

    cleanup(&root)?;
    Ok(())
}

fn write_md5_cache(
    portage_tree: &Path,
    package: &str,
    version: &str,
    contents: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let (category, package_name) = package.split_once('/').ok_or("invalid package")?;
    let path = portage_tree
        .join("metadata")
        .join("md5-cache")
        .join(category)
        .join(format!("{package_name}-{version}"));

    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }

    fs::write(path, contents)?;
    Ok(())
}

fn test_root(name: &str) -> std::path::PathBuf {
    let unique = format!(
        "oxys_portage_integration_{name}_{}_{}",
        std::process::id(),
        chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default()
    );
    std::env::temp_dir().join(unique)
}

fn cleanup(path: &Path) -> Result<(), Box<dyn std::error::Error>> {
    if path.exists() {
        fs::remove_dir_all(path)?;
    }
    Ok(())
}
