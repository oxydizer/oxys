use std::fs;
use std::process::Command;

use oxys::manifest::{AudioStack, DisplayStack, InitSystem, Package, SystemManifest};
use oxys::manifest_to_toml;

#[path = "support/mod.rs"]
mod support;

use support::fixture_repo::FixtureRepo;

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

#[test]
fn check_exits_nonzero_and_reports_specific_conflict() -> Result<(), Box<dyn std::error::Error>> {
    // Replicates the explicit-vs-policy conflict scenario using the binary.
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
        combined.contains("X") && combined.contains("display_stack = wayland"),
        "missing specific display_stack conflict line in output:\n{}",
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

#[test]
fn check_reports_use_flags_vs_global_prefer_binary_as_warning_not_error_end_to_end(
) -> Result<(), Box<dyn std::error::Error>> {
    // End-to-end via the oxys binary (not just resolver lib): global
    // prefer_binary + use_flags without .from_source() falls back to source
    // for that package and just warns, rather than blocking the whole plan.
    let fixture = FixtureRepo::new().with_package("gui-wm/niri", &["wayland", "screencast"]);

    let workdir = tempfile::TempDir::new()?;
    let manifest = SystemManifest {
        prefer_binary: true,
        packages: vec![Package::new("gui-wm/niri").use_flags(["screencast"])],
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
        "expected success (warning, not hard conflict):\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Plan check passed"));
    assert!(
        stdout.contains("falling back to source"),
        "missing fallback warning in output:\n{}",
        stdout
    );
    assert!(!stdout.contains("hard conflicts detected"));

    Ok(())
}

#[test]
fn check_from_source_allows_use_flags_even_with_global_prefer_binary(
) -> Result<(), Box<dyn std::error::Error>> {
    let fixture = FixtureRepo::new().with_package("gui-wm/niri", &["wayland", "screencast"]);

    let workdir = tempfile::TempDir::new()?;
    let manifest = SystemManifest {
        prefer_binary: true,
        packages: vec![Package::new("gui-wm/niri")
            .from_source()
            .use_flags(["screencast"])],
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
        "expected success when .from_source() used with prefer_binary:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Plan check passed"));
    // should not have the binary error
    assert!(!stdout.contains("will install from binary"));

    Ok(())
}
