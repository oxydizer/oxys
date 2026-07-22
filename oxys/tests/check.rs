use std::fs;
use std::process::Command;

use oxys::manifest::{AudioStack, DisplayStack, InitSystem, Package, SystemManifest};
use oxys::manifest_to_toml;

#[path = "support/mod.rs"]
mod support;

use support::fixture_repo::FixtureRepo;

/// CLI smoke: clean plan exits 0 with the success banner.
/// Scenario coverage for policy lives in portage_integration/policy.
#[test]
fn check_exits_zero_on_clean_manifest() -> Result<(), Box<dyn std::error::Error>> {
    let fixture = FixtureRepo::new().with_package("app-admin/example", &["openrc", "systemd"]);

    let workdir = tempfile::TempDir::new()?;
    let manifest = SystemManifest {
        packages: vec![Package::new("app-admin/example")],
        ..SystemManifest::default()
    };
    let toml = manifest_to_toml(&manifest)?;
    fs::write(workdir.path().join("manifest.toml"), toml)?;

    let oxys = env!("CARGO_BIN_EXE_oxys");
    let output = Command::new(oxys)
        .arg("check")
        .current_dir(workdir.path())
        .env("OXYS_PORTAGE_TREE", fixture.root.path())
        .env("OXYS_CACHE_DIR", workdir.path().join("cache"))
        .output()?;

    assert!(
        output.status.success(),
        "expected exit 0, got {:?}\nstdout:\n{}\nstderr:\n{}",
        output.status.code(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("Plan check passed"),
        "expected success message in stdout, got:\n{}",
        stdout
    );

    Ok(())
}

/// CLI smoke: hard conflicts exit non-zero and surface specific policy lines.
/// Detailed prefer_binary / from_source cases are covered by portage_integration.
#[test]
fn check_exits_nonzero_and_reports_specific_conflict() -> Result<(), Box<dyn std::error::Error>> {
    // Package explicitly enables flags that disagree with manifest policy.
    let fixture = FixtureRepo::new().with_package(
        "app-admin/example",
        &[
            "systemd",
            "openrc",
            "wayland",
            "X",
            "pipewire",
            "pulseaudio",
        ],
    );

    let workdir = tempfile::TempDir::new()?;
    let manifest = SystemManifest {
        init_system: InitSystem::Systemd,
        display_stack: Some(DisplayStack::Wayland),
        audio_stack: Some(AudioStack::Pipewire),
        packages: vec![Package::new("app-admin/example").use_flags(["openrc", "X", "pulseaudio"])],
        ..SystemManifest::default()
    };
    let toml = manifest_to_toml(&manifest)?;
    fs::write(workdir.path().join("manifest.toml"), toml)?;

    let oxys = env!("CARGO_BIN_EXE_oxys");
    let output = Command::new(oxys)
        .arg("check")
        .current_dir(workdir.path())
        .env("OXYS_PORTAGE_TREE", fixture.root.path())
        .env("OXYS_CACHE_DIR", workdir.path().join("cache"))
        .output()?;

    assert!(
        !output.status.success(),
        "expected nonzero exit for conflict case, got success\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    // The plan is still printed (conflicts section), then error is emitted.
    // Look for the specific conflict details (not a generic "both X and Y" message).
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let combined = format!("{stdout}{stderr}");

    // Specific per-flag conflicts naming the package and the policy field.
    assert!(
        combined.contains("openrc") && combined.contains("init_system = systemd"),
        "missing specific init_system conflict line in output:\n{}",
        combined
    );
    assert!(
        !combined.contains("display_stack = wayland"),
        "X compatibility must remain valid with a Wayland display stack:\n{}",
        combined
    );
    assert!(
        combined.contains("pulseaudio") && combined.contains("audio_stack = pipewire"),
        "missing specific audio_stack conflict line in output:\n{}",
        combined
    );

    // Also confirm the hard error that aborts check.
    assert!(
        combined.contains("hard conflicts detected"),
        "expected hard conflict error message, got:\n{}",
        combined
    );

    Ok(())
}
