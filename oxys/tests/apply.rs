use std::{fs, process::Command};

use oxys::{
    manifest::{Package, SystemManifest},
    manifest_to_toml,
};

#[path = "support/mod.rs"]
mod support;

use support::fixture_repo::FixtureRepo;

/// Verifies F16's fix: `oxys apply` must converge world membership to the manifest, not
/// just merge the desired package set. A package dropped from the manifest should be
/// `--deselect`'d, a newly added package should be `--noreplace --select`'d (never
/// touched by the exact-version `--oneshot` merge itself), and `--depclean --pretend`
/// should always run so orphans are surfaced without being removed automatically.
#[test]
fn apply_reconciles_world_with_manifest() -> Result<(), Box<dyn std::error::Error>> {
    let fixture = FixtureRepo::new()
        .with_package("app-admin/keep", &[])
        .with_package("app-admin/added", &[]);

    let workdir = tempfile::TempDir::new()?;
    let system_manifest_path = workdir.path().join("current-manifest.toml");
    let portage_config_dir = workdir.path().join("portage-config");
    let emerge_log = workdir.path().join("emerge.log");
    let emerge = write_fake_emerge(workdir.path(), &emerge_log)?;

    let current = SystemManifest {
        packages: vec![
            Package::new("app-admin/keep").version("1.0.0"),
            Package::new("app-admin/removed").version("1.0.0"),
        ],
        ..SystemManifest::default()
    };
    fs::write(&system_manifest_path, manifest_to_toml(&current)?)?;

    let desired = SystemManifest {
        packages: vec![
            Package::new("app-admin/keep").version("1.0.0"),
            Package::new("app-admin/added").version("1.0.0"),
        ],
        ..SystemManifest::default()
    };
    fs::write(
        workdir.path().join("manifest.toml"),
        manifest_to_toml(&desired)?,
    )?;

    let output = Command::new(env!("CARGO_BIN_EXE_oxys"))
        .arg("apply")
        .current_dir(workdir.path())
        .env("OXYS_EMERGE", &emerge)
        .env("OXYS_SYSTEM_MANIFEST", &system_manifest_path)
        .env("OXYS_PORTAGE_TREE", fixture.root.path())
        .env("OXYS_PORTAGE_CONFIG_DIR", &portage_config_dir)
        .env("OXYS_CACHE_DIR", workdir.path().join("cache"))
        .env("OXYS_ROOT", workdir.path().join("root"))
        .output()?;

    assert!(
        output.status.success(),
        "expected apply to succeed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let log = fs::read_to_string(&emerge_log).unwrap_or_default();
    let calls = log.lines().collect::<Vec<_>>();

    assert!(
        calls
            .iter()
            .any(|line| line.contains("--oneshot") && line.contains("app-admin/keep")),
        "expected the merge invocation to use --oneshot, log:\n{log}"
    );
    assert!(
        calls
            .iter()
            .any(|line| line.contains("--deselect") && line.contains("app-admin/removed")),
        "expected removed package to be deselected, log:\n{log}"
    );
    assert!(
        calls.iter().any(|line| {
            line.contains("--noreplace")
                && line.contains("--select")
                && line.contains("app-admin/added")
        }),
        "expected added package to be select'd without rebuilding, log:\n{log}"
    );
    assert!(
        calls
            .iter()
            .any(|line| line.contains("--depclean") && line.contains("--pretend")),
        "expected depclean --pretend to be surfaced, log:\n{log}"
    );
    assert!(
        !calls
            .iter()
            .any(|line| line.contains("--deselect") && line.contains("app-admin/keep")),
        "did not expect the still-desired package to be deselected, log:\n{log}"
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("Packages to remove:") && stdout.contains("app-admin/removed"),
        "expected the diff to surface the removed package in stdout:\n{stdout}"
    );
    assert!(
        stdout.contains("depclean would remove"),
        "expected the depclean-pretend header in stdout:\n{stdout}"
    );

    Ok(())
}

fn write_fake_emerge(
    dir: &std::path::Path,
    log: &std::path::Path,
) -> Result<std::path::PathBuf, Box<dyn std::error::Error>> {
    let path = dir.join("fake-emerge");
    let script = format!(
        r#"#!/bin/sh
echo "ARGS $*" >> "{log}"
case " $* " in
  *" --depclean "*)
    echo "Calculating dependencies... done!" >&2
    exit 0
    ;;
  *" --deselect "*)
    exit 0
    ;;
  *" --select "*)
    exit 0
    ;;
  *" --oneshot "*)
    exit 0
    ;;
esac
echo "unexpected fake emerge args: $*" >&2
exit 2
"#,
        log = log.display(),
    );
    fs::write(&path, script)?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut permissions = fs::metadata(&path)?.permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&path, permissions)?;
    }

    Ok(path)
}
