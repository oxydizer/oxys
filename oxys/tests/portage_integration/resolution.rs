use super::*;

#[test]
fn resolves_versions_from_portage_tree_when_package_version_is_omitted()
-> Result<(), Box<dyn std::error::Error>> {
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
    assert_eq!(plan.targets, vec!["=app-admin/example-0.9.0".to_owned()]);

    assert!(plan.resolution.decisions.iter().any(|decision| {
        decision.scope == DecisionScope::PlannerPolicy
            && decision.subject == "libc"
            && decision.action == DecisionAction::Note
            && decision.source == DecisionSource::ManifestPolicy
            && decision.reason.contains("manifest libc policy is glibc")
    }));
    assert!(!plan.resolution.decisions.iter().any(|decision| {
        decision.scope == DecisionScope::AcceptKeywords && decision.subject == "~amd64"
    }));

    cleanup(&root)?;
    Ok(())
}

#[test]
fn explicit_testing_keyword_selects_the_newest_testing_version()
-> Result<(), Box<dyn std::error::Error>> {
    let root = test_root("explicit_testing_version");
    let portage_tree = root.join("repo");
    let cache_dir = root.join("cache");

    write_md5_cache(
        &portage_tree,
        "app-admin/example",
        "0.9.0",
        "IUSE=\nKEYWORDS=amd64\n",
    )?;
    write_md5_cache(
        &portage_tree,
        "app-admin/example",
        "1.0.0",
        "IUSE=\nKEYWORDS=~amd64\n",
    )?;

    let manifest = SystemManifest {
        packages: vec![Package::new("app-admin/example").keywords(["~amd64"])],
        ..SystemManifest::default()
    };
    let plan = plan_portage(&manifest, &portage_tree, &cache_dir)?;

    assert_eq!(plan.targets, vec!["=app-admin/example-1.0.0".to_owned()]);
    assert!(
        plan.resolution
            .accept_keywords
            .iter()
            .any(|entry| entry.contains("~amd64"))
    );

    cleanup(&root)?;
    Ok(())
}

#[test]
fn defaults_init_and_infers_audio_when_manifest_policy_is_absent()
-> Result<(), Box<dyn std::error::Error>> {
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
    assert!(
        plan.resolution
            .decisions
            .iter()
            .any(|decision| { decision.reason.contains("manifest init system is openrc") })
    );
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
        packages: vec![
            Package::new("gui-wm/niri").keywords(["~amd64"]),
            Package::new("gui-apps/waybar").keywords(["~amd64"]),
        ],
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
        fs::read_to_string(config_dir.join("package.accept_keywords/oxys"))?,
        concat!(
            "# generated by oxys - do not edit manually\n",
            "gui-apps/waybar ~amd64\n",
            "gui-wm/niri ~amd64\n"
        )
    );
    assert_eq!(
        fs::read_to_string(config_dir.join("package.use").join("oxys"))?,
        concat!(
            "# generated by oxys - do not edit manually\n",
            "gui-apps/waybar wayland\n",
            "gui-wm/niri X wayland\n"
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
fn explicit_wayland_display_stack_keeps_x_compatibility() -> Result<(), Box<dyn std::error::Error>>
{
    let root = test_root("explicit_wayland_keeps_x_compatibility");
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
        packages: vec![Package::new("gui-wm/niri").keywords(["~amd64"])],
        ..SystemManifest::default()
    };

    let plan = plan_portage(&manifest, &portage_tree, &cache_dir)?;

    // Wayland selects the primary display path. It does not disable X library
    // support needed by Xwayland, GTK, and compatible binpkgs.
    assert!(plan.resolution.global_use.contains(&"wayland".to_owned()));
    assert!(!plan.resolution.global_use.contains(&"-X".to_owned()));

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
fn resolve_latest_version_uses_gentoo_style_version_ordering()
-> Result<(), Box<dyn std::error::Error>> {
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
fn resolve_latest_version_searches_across_repo_root_children()
-> Result<(), Box<dyn std::error::Error>> {
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
