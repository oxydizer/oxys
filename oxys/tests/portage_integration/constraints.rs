use super::*;

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
        packages: vec![
            Package::new("gui-wm/niri")
                .keywords(["~amd64"])
                .accept_licenses(["all-rights-reserved"]),
        ],
        ..SystemManifest::default()
    };

    let plan = plan_portage(&manifest, &portage_tree, &cache_dir)?;

    assert!(
        plan.resolution
            .accept_keywords
            .iter()
            .any(|entry| entry == "~amd64 gui-wm/niri")
    );
    assert!(
        plan.resolution
            .accept_licenses
            .iter()
            .any(|entry| entry == "all-rights-reserved gui-wm/niri")
    );
    assert!(!plan.resolution.warnings.iter().any(|warning| {
        warning.package == "gui-wm/niri"
            && warning.message.contains("require explicit accept_license")
    }));

    cleanup(&root)?;
    Ok(())
}

#[test]
fn abi_consistency_conflict_when_binary_depends_on_modified_source()
-> Result<(), Box<dyn std::error::Error>> {
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
fn abi_consistency_treats_prefer_binary_dependents_as_binary()
-> Result<(), Box<dyn std::error::Error>> {
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
        packages: vec![
            Package::new("media-video/pipewire")
                .from_source()
                .use_flags(["-systemd"]),
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
