use std::{fs, process::Command};

use oxys::{
    manifest::{Package, SystemManifest},
    manifest_to_toml,
    use_resolver::{parse_pretend_world_update, plan_update_preflight},
};

#[path = "support/mod.rs"]
mod support;

use support::fixture_repo::FixtureRepo;

#[test]
fn update_preflight_catches_toolchain_source_bump_affecting_binary_package(
) -> Result<(), Box<dyn std::error::Error>> {
    let fixture = FixtureRepo::new()
        .with_package_metadata("sys-devel/gcc-15.1.1", "IUSE=systemd\nKEYWORDS=amd64\n")
        .with_package_metadata(
            "app-misc/binary-consumer-1.0.0",
            "IUSE=+systemd\nRDEPEND=sys-devel/gcc\nKEYWORDS=amd64\n",
        );
    let cache_dir = tempfile::TempDir::new()?;
    let current = SystemManifest {
        packages: vec![
            Package::new("sys-devel/gcc")
                .version("14.3.0")
                .from_source()
                .use_flags(["-systemd"]),
            Package::new("app-misc/binary-consumer")
                .version("1.0.0")
                .binary(true),
        ],
        ..SystemManifest::default()
    };
    let pretend = parse_pretend_world_update(
        "These are the packages that would be merged, in order:\n[ebuild U] sys-devel/gcc [15.1.1] [14.3.0]\n",
    )?;

    let plan = plan_update_preflight(&current, &pretend, fixture.root.path(), cache_dir.path())?;

    assert!(
        plan.resolution.conflicts.iter().any(|conflict| {
            conflict.flag == "abi-consistency"
                && conflict.packages.contains(&"sys-devel/gcc".to_owned())
                && conflict
                    .packages
                    .contains(&"app-misc/binary-consumer".to_owned())
                && conflict
                    .reason
                    .contains("rebuild 'app-misc/binary-consumer' from source")
        }),
        "expected toolchain update row to feed existing ABI consistency check, got {:?}",
        plan.resolution.conflicts
    );

    Ok(())
}

#[test]
fn clean_update_preflight_passes() -> Result<(), Box<dyn std::error::Error>> {
    let fixture = FixtureRepo::new()
        .with_package_metadata("app-admin/example-2.0.0", "IUSE=+systemd\nKEYWORDS=amd64\n");
    let cache_dir = tempfile::TempDir::new()?;
    let current = SystemManifest {
        packages: vec![Package::new("app-admin/example").version("1.0.0")],
        ..SystemManifest::default()
    };
    let pretend = parse_pretend_world_update(
        "These are the packages that would be merged, in order:\n[ebuild U] app-admin/example [2.0.0] [1.0.0]\n",
    )?;

    let plan = plan_update_preflight(&current, &pretend, fixture.root.path(), cache_dir.path())?;

    assert!(
        plan.resolution.conflicts.is_empty(),
        "expected clean update preflight, got {:?}",
        plan.resolution.conflicts
    );

    Ok(())
}

#[test]
fn update_dry_run_binary_reports_conflict_and_exits_nonzero(
) -> Result<(), Box<dyn std::error::Error>> {
    let fixture = FixtureRepo::new()
        .with_package_metadata("sys-devel/gcc-15.1.1", "IUSE=systemd\nKEYWORDS=amd64\n")
        .with_package_metadata(
            "app-misc/binary-consumer-1.0.0",
            "IUSE=+systemd\nRDEPEND=sys-devel/gcc\nKEYWORDS=amd64\n",
        );
    let workdir = tempfile::TempDir::new()?;
    let current_manifest_path = workdir.path().join("current-manifest.toml");
    let emerge_log = workdir.path().join("emerge.log");
    let emerge = write_fake_emerge(
        workdir.path(),
        &emerge_log,
        "These are the packages that would be merged, in order:\n[ebuild U] sys-devel/gcc [15.1.1] [14.3.0]\n",
    )?;
    let current = SystemManifest {
        packages: vec![
            Package::new("sys-devel/gcc")
                .version("14.3.0")
                .from_source()
                .use_flags(["-systemd"]),
            Package::new("app-misc/binary-consumer")
                .version("1.0.0")
                .binary(true),
        ],
        ..SystemManifest::default()
    };
    fs::write(&current_manifest_path, manifest_to_toml(&current)?)?;

    let output = Command::new(env!("CARGO_BIN_EXE_oxys"))
        .arg("update")
        .arg("--dry-run")
        .current_dir(workdir.path())
        .env("OXYS_EMERGE", &emerge)
        .env("OXYS_SYSTEM_MANIFEST", &current_manifest_path)
        .env("OXYS_PORTAGE_TREE", fixture.root.path())
        .env("OXYS_CACHE_DIR", workdir.path().join("cache"))
        .output()?;

    assert!(
        !output.status.success(),
        "expected dry-run conflict to exit nonzero\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(combined.contains("abi-consistency"), "{combined}");
    assert!(combined.contains("binary-consumer"), "{combined}");
    assert!(combined.contains("hard conflicts detected"), "{combined}");
    assert!(!fs::read_to_string(emerge_log)?.contains("REAL"));

    Ok(())
}

#[test]
fn update_clean_preflight_runs_real_update() -> Result<(), Box<dyn std::error::Error>> {
    let fixture = FixtureRepo::new()
        .with_package_metadata("app-admin/example-2.0.0", "IUSE=\nKEYWORDS=amd64\n");
    let workdir = tempfile::TempDir::new()?;
    let current_manifest_path = workdir.path().join("current-manifest.toml");
    let emerge_log = workdir.path().join("emerge.log");
    let report_dir = workdir.path().join("reports");
    let emerge = write_fake_emerge(
        workdir.path(),
        &emerge_log,
        "These are the packages that would be merged, in order:\n[ebuild U] app-admin/example [2.0.0] [1.0.0]\n",
    )?;
    let current = SystemManifest {
        packages: vec![Package::new("app-admin/example").version("1.0.0")],
        ..SystemManifest::default()
    };
    fs::write(&current_manifest_path, manifest_to_toml(&current)?)?;

    let output = Command::new(env!("CARGO_BIN_EXE_oxys"))
        .arg("update")
        .current_dir(workdir.path())
        .env("OXYS_EMERGE", &emerge)
        .env("OXYS_SYSTEM_MANIFEST", &current_manifest_path)
        .env("OXYS_PORTAGE_TREE", fixture.root.path())
        .env("OXYS_CACHE_DIR", workdir.path().join("cache"))
        .env("OXYS_UPDATE_LOG_DIR", &report_dir)
        .output()?;

    assert!(
        output.status.success(),
        "expected clean update to proceed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let log = fs::read_to_string(emerge_log)?;
    assert!(log.contains("SYNC"), "{log}");
    assert!(log.contains("PRETEND"), "{log}");
    assert!(log.contains("REAL"), "{log}");
    let reports = fs::read_dir(&report_dir)?.collect::<Result<Vec<_>, _>>()?;
    assert_eq!(reports.len(), 1);
    let report = fs::read_to_string(reports[0].path())?;
    assert!(
        report.contains("real_update_status = \"completed\""),
        "{report}"
    );
    assert!(
        report.contains("package = \"app-admin/example\""),
        "{report}"
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("Saved update report:") && stdout.contains("Updated update report:"),
        "expected pre-merge and post-merge report writes\n{stdout}"
    );

    Ok(())
}

#[test]
fn update_no_sync_dry_run_skips_sync() -> Result<(), Box<dyn std::error::Error>> {
    let fixture = FixtureRepo::new()
        .with_package_metadata("app-admin/example-2.0.0", "IUSE=\nKEYWORDS=amd64\n");
    let workdir = tempfile::TempDir::new()?;
    let current_manifest_path = workdir.path().join("current-manifest.toml");
    let emerge_log = workdir.path().join("emerge.log");
    let report_dir = workdir.path().join("reports");
    let emerge = write_fake_emerge(
        workdir.path(),
        &emerge_log,
        "These are the packages that would be merged, in order:\n[ebuild U] app-admin/example [2.0.0] [1.0.0]\n",
    )?;
    let current = SystemManifest {
        packages: vec![Package::new("app-admin/example").version("1.0.0")],
        ..SystemManifest::default()
    };
    fs::write(&current_manifest_path, manifest_to_toml(&current)?)?;

    let output = Command::new(env!("CARGO_BIN_EXE_oxys"))
        .arg("update")
        .arg("--dry-run")
        .arg("--no-sync")
        .current_dir(workdir.path())
        .env("OXYS_EMERGE", &emerge)
        .env("OXYS_SYSTEM_MANIFEST", &current_manifest_path)
        .env("OXYS_PORTAGE_TREE", fixture.root.path())
        .env("OXYS_CACHE_DIR", workdir.path().join("cache"))
        .env("OXYS_UPDATE_LOG_DIR", &report_dir)
        .output()?;

    assert!(
        output.status.success(),
        "expected clean no-sync dry-run to pass\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let log = fs::read_to_string(emerge_log)?;
    assert!(!log.contains("SYNC"), "{log}");
    assert!(log.contains("PRETEND"), "{log}");
    assert!(!log.contains("REAL"), "{log}");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("sync: skipped"), "{stdout}");
    let reports = fs::read_dir(&report_dir)?.collect::<Result<Vec<_>, _>>()?;
    assert_eq!(reports.len(), 1);
    let report = fs::read_to_string(reports[0].path())?;
    assert!(
        report.contains("real_update_status = \"skipped_dry_run\""),
        "{report}"
    );
    assert!(report.contains("sync_ran = false"), "{report}");

    Ok(())
}

#[test]
fn update_pretend_only_does_not_require_manifest() -> Result<(), Box<dyn std::error::Error>> {
    let workdir = tempfile::TempDir::new()?;
    let emerge_log = workdir.path().join("emerge.log");
    let emerge = write_fake_emerge(
        workdir.path(),
        &emerge_log,
        "These are the packages that would be merged, in order:\n[binary U] app-admin/example [2.0.0] [1.0.0]\n",
    )?;

    let output = Command::new(env!("CARGO_BIN_EXE_oxys"))
        .arg("update")
        .arg("--pretend-only")
        .arg("--no-sync")
        .current_dir(workdir.path())
        .env("OXYS_EMERGE", &emerge)
        .env("OXYS_UPDATE_LOG_DIR", workdir.path().join("reports"))
        .output()?;

    assert!(
        output.status.success(),
        "expected pretend-only to pass without current manifest\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let log = fs::read_to_string(emerge_log)?;
    assert!(!log.contains("SYNC"), "{log}");
    assert!(log.contains("PRETEND"), "{log}");
    assert!(!log.contains("REAL"), "{log}");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("World update"), "{stdout}");
    assert!(stdout.contains("app-admin/example"), "{stdout}");
    assert!(stdout.contains("binary packages:"), "{stdout}");

    Ok(())
}

#[test]
fn update_nothing_to_merge_exits_without_manifest() -> Result<(), Box<dyn std::error::Error>> {
    let workdir = tempfile::TempDir::new()?;
    let emerge_log = workdir.path().join("emerge.log");
    let report_dir = workdir.path().join("reports");
    let emerge = write_fake_emerge(
        workdir.path(),
        &emerge_log,
        "Calculating dependencies... done!\nNothing to merge; quitting.\n",
    )?;

    let output = Command::new(env!("CARGO_BIN_EXE_oxys"))
        .arg("update")
        .arg("--no-sync")
        .current_dir(workdir.path())
        .env("OXYS_EMERGE", &emerge)
        .env("OXYS_UPDATE_LOG_DIR", &report_dir)
        .output()?;

    assert!(
        output.status.success(),
        "expected no-update path to pass without current manifest\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let log = fs::read_to_string(emerge_log)?;
    assert!(log.contains("PRETEND"), "{log}");
    assert!(!log.contains("REAL"), "{log}");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("No world updates proposed by Portage"), "{stdout}");
    let reports = fs::read_dir(&report_dir)?.collect::<Result<Vec<_>, _>>()?;
    assert_eq!(reports.len(), 1);
    let report = fs::read_to_string(reports[0].path())?;
    assert!(report.contains("real_update_status = \"no_updates\""), "{report}");

    Ok(())
}

#[test]
fn update_missing_manifest_fails_closed() -> Result<(), Box<dyn std::error::Error>> {
    let workdir = tempfile::TempDir::new()?;
    let emerge_log = workdir.path().join("emerge.log");
    let missing_manifest = workdir.path().join("does-not-exist.toml");
    let emerge = write_fake_emerge(
        workdir.path(),
        &emerge_log,
        "These are the packages that would be merged, in order:\n[ebuild U] app-admin/example [2.0.0] [1.0.0]\n",
    )?;

    let output = Command::new(env!("CARGO_BIN_EXE_oxys"))
        .arg("update")
        .arg("--no-sync")
        .current_dir(workdir.path())
        .env("OXYS_EMERGE", &emerge)
        .env("OXYS_SYSTEM_MANIFEST", &missing_manifest)
        .env("OXYS_CACHE_DIR", workdir.path().join("cache"))
        .output()?;

    assert!(
        !output.status.success(),
        "expected missing manifest to fail closed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(combined.contains("current oxys manifest not found"), "{combined}");
    assert!(combined.contains("--force"), "{combined}");
    let log = fs::read_to_string(emerge_log)?;
    assert!(log.contains("PRETEND"), "{log}");
    assert!(!log.contains("REAL"), "{log}");

    Ok(())
}

#[test]
fn update_force_skips_sync_and_preflight() -> Result<(), Box<dyn std::error::Error>> {
    let workdir = tempfile::TempDir::new()?;
    let emerge_log = workdir.path().join("emerge.log");
    let emerge = write_fake_emerge(
        workdir.path(),
        &emerge_log,
        "malformed pretend output that would fail if preflight ran",
    )?;

    let output = Command::new(env!("CARGO_BIN_EXE_oxys"))
        .arg("update")
        .arg("--force")
        .current_dir(workdir.path())
        .env("OXYS_EMERGE", &emerge)
        .output()?;

    assert!(
        output.status.success(),
        "expected --force to run real update only\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let log = fs::read_to_string(emerge_log)?;
    assert!(log.contains("REAL"), "{log}");
    assert!(!log.contains("SYNC"), "{log}");
    assert!(!log.contains("PRETEND"), "{log}");

    Ok(())
}

#[test]
fn update_force_threads_jobs_and_keep_going() -> Result<(), Box<dyn std::error::Error>> {
    let workdir = tempfile::TempDir::new()?;
    let emerge_log = workdir.path().join("emerge.log");
    let emerge = write_fake_emerge(
        workdir.path(),
        &emerge_log,
        "malformed pretend output that would fail if preflight ran",
    )?;

    let output = Command::new(env!("CARGO_BIN_EXE_oxys"))
        .arg("update")
        .arg("--force")
        .arg("--jobs")
        .arg("4")
        .arg("--keep-going")
        .current_dir(workdir.path())
        .env("OXYS_EMERGE", &emerge)
        .output()?;

    assert!(
        output.status.success(),
        "expected forced update to pass\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let log = fs::read_to_string(emerge_log)?;
    assert!(log.contains("REAL"), "{log}");
    assert!(
        log.contains("ARGS --jobs 4 --keep-going -uDN @world"),
        "{log}"
    );

    Ok(())
}

#[test]
fn malformed_pretend_output_fails_closed_in_binary() -> Result<(), Box<dyn std::error::Error>> {
    let workdir = tempfile::TempDir::new()?;
    let current_manifest_path = workdir.path().join("current-manifest.toml");
    let emerge_log = workdir.path().join("emerge.log");
    let emerge = write_fake_emerge(
        workdir.path(),
        &emerge_log,
        "These are the packages that would be merged, in order:\n[ebuild U] ???\n",
    )?;
    fs::write(
        &current_manifest_path,
        manifest_to_toml(&SystemManifest::default())?,
    )?;

    let output = Command::new(env!("CARGO_BIN_EXE_oxys"))
        .arg("update")
        .current_dir(workdir.path())
        .env("OXYS_EMERGE", &emerge)
        .env("OXYS_SYSTEM_MANIFEST", &current_manifest_path)
        .output()?;

    assert!(
        !output.status.success(),
        "expected malformed pretend output to fail closed"
    );
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        combined.contains("cannot verify emerge pretend output"),
        "{combined}"
    );
    assert!(
        combined.contains("Refusing to run emerge -uDN @world"),
        "{combined}"
    );
    assert!(!fs::read_to_string(emerge_log)?.contains("REAL"));

    Ok(())
}

fn write_fake_emerge(
    dir: &std::path::Path,
    log: &std::path::Path,
    pretend_output: &str,
) -> Result<std::path::PathBuf, Box<dyn std::error::Error>> {
    let path = dir.join("fake-emerge");
    let script = format!(
        r#"#!/bin/sh
echo "ARGS $*" >> "{log}"
case " $* " in
  *" --sync "*)
    echo SYNC >> "{log}"
    exit 0
    ;;
  *" -uDNp "*)
    echo PRETEND >> "{log}"
    cat <<'OXYS_PRETEND'
{pretend_output}
OXYS_PRETEND
    exit 0
    ;;
  *" -uDN @world "*)
    echo REAL >> "{log}"
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
