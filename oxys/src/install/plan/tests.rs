use std::{
    fs,
    path::{Path, PathBuf},
    sync::mpsc,
    time::{SystemTime, UNIX_EPOCH},
};

use crate::manifest::{DiskPartitions, EfiPartition, Ext4Options, GB, MB, Package, Password};

use super::*;

mod boot;
mod installation;
mod users;

fn setup_login_for_test(
    manifest: &SystemManifest,
    target: &Path,
    sender: &std::sync::mpsc::Sender<SystemInstallEvent>,
) {
    let resolved = manifest.resolved_session().unwrap();
    let materialized = resolved.materialize_manifest(manifest);
    let resolved_graphics = materialized.resolved_graphics().unwrap();
    login::setup_login(&materialized, &resolved, &resolved_graphics, target, sender).unwrap();
}

fn write_graphical_source_requirements(source: &Path) {
    use std::os::unix::fs::PermissionsExt;

    for relative in [
        "usr/local/bin/oxys-login",
        "usr/bin/agetty",
        "usr/bin/login",
    ] {
        let path = source.join(relative);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, "fixture").unwrap();
        fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
    }
    fs::create_dir_all(source.join("etc/pam.d")).unwrap();
    fs::write(source.join("etc/pam.d/login"), "auth include system-auth\n").unwrap();
}

struct TempTree {
    path: PathBuf,
}

impl TempTree {
    fn new(name: &str) -> Self {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!("oxys-install-test-{name}-{nanos}"));
        fs::create_dir_all(&path).unwrap();
        Self { path }
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TempTree {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}
