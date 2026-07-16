//! Prebuilt manifests for the stock desktop/base profiles.
//!
//! The live ISO ships already-compiled `manifest.toml` files for the two
//! default configs under `configs/prebuilt/`, stamped with the sha256 of the
//! `.fe2o3` source they were built from. When the user advances without editing
//! those profiles, the installer reuses the stamp instead of invoking cargo.

use std::fs;
use std::path::{Path, PathBuf};

/// Directory (relative to the installer cwd, normally `/root`) holding prebuilt
/// manifests for the stock profiles.
pub(crate) const PREBUILT_DIR: &str = "configs/prebuilt";

/// Profiles that the ISO is expected to ship prebuilt manifests for.
const PREBUILT_PROFILES: &[&str] = &["desktop", "base"];

/// If `config_path` is an unedited stock profile with a matching prebuilt
/// manifest on disk, return that manifest path. Otherwise `None` (caller falls
/// back to a live `oxys::compile`).
pub(crate) fn try_prebuilt_manifest(config_path: &Path) -> Option<PathBuf> {
    let profile = stock_profile_name(config_path)?;
    let prebuilt = prebuilt_manifest_path(profile);
    let stamp = prebuilt_stamp_path(profile);
    if !prebuilt.is_file() || !stamp.is_file() {
        return None;
    }

    let expected = fs::read_to_string(&stamp).ok()?;
    let expected = expected.split_whitespace().next()?.trim();
    if expected.is_empty() {
        return None;
    }

    let actual = file_sha256(config_path)?;
    if actual != expected {
        // Source was edited (or the ISO stamp is stale after a config refresh).
        return None;
    }

    // Cheap integrity gate: refuse a truncated/corrupt prebuilt so the install
    // falls back to a real compile instead of failing later in provisioning.
    oxys::compile::load_manifest(&prebuilt).ok()?;
    Some(prebuilt)
}

fn stock_profile_name(config_path: &Path) -> Option<&'static str> {
    let file_name = config_path.file_name()?.to_str()?;
    PREBUILT_PROFILES
        .iter()
        .copied()
        .find(|profile| file_name == format!("{profile}.fe2o3"))
}

pub(crate) fn prebuilt_manifest_path(profile: &str) -> PathBuf {
    PathBuf::from(PREBUILT_DIR).join(format!("{profile}.manifest.toml"))
}

pub(crate) fn prebuilt_stamp_path(profile: &str) -> PathBuf {
    PathBuf::from(PREBUILT_DIR).join(format!("{profile}.source.sha256"))
}

fn file_sha256(path: &Path) -> Option<String> {
    let bytes = fs::read(path).ok()?;
    Some(oxys::util::sha256_hex(&bytes))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn write_temp(dir: &Path, name: &str, contents: &[u8]) -> PathBuf {
        let path = dir.join(name);
        let mut file = fs::File::create(&path).unwrap();
        file.write_all(contents).unwrap();
        path
    }

    #[test]
    fn stock_profile_name_only_matches_desktop_and_base() {
        assert_eq!(
            stock_profile_name(Path::new("configs/desktop.fe2o3")),
            Some("desktop")
        );
        assert_eq!(
            stock_profile_name(Path::new("configs/base.fe2o3")),
            Some("base")
        );
        assert_eq!(stock_profile_name(Path::new("configs/custom.fe2o3")), None);
        assert_eq!(stock_profile_name(Path::new("/tmp/other.fe2o3")), None);
    }

    #[test]
    fn try_prebuilt_requires_matching_hash() {
        let dir = tempfile_dir();
        let prev = std::env::current_dir().unwrap();
        std::env::set_current_dir(&dir).unwrap();

        let prebuilt_dir = dir.join(PREBUILT_DIR);
        fs::create_dir_all(&prebuilt_dir).unwrap();

        let source = write_temp(&dir.join("configs"), "desktop.fe2o3", b"// desktop config\n");
        // Missing prebuilt → None
        assert!(try_prebuilt_manifest(&source).is_none());

        // A real validated manifest is heavy to synthesise here; just check the
        // hash mismatch short-circuits before load_manifest is reached by using
        // an invalid stamp while a dummy prebuilt file exists.
        write_temp(
            &prebuilt_dir,
            "desktop.manifest.toml",
            b"not a valid generated manifest\n",
        );
        write_temp(&prebuilt_dir, "desktop.source.sha256", b"deadbeef\n");
        assert!(try_prebuilt_manifest(&source).is_none());

        // Matching hash still fails validation on the dummy TOML.
        let hash = file_sha256(&source).unwrap();
        write_temp(
            &prebuilt_dir,
            "desktop.source.sha256",
            format!("{hash}\n").as_bytes(),
        );
        assert!(try_prebuilt_manifest(&source).is_none());

        std::env::set_current_dir(prev).unwrap();
        let _ = fs::remove_dir_all(&dir);
    }

    fn tempfile_dir() -> PathBuf {
        let mut dir = std::env::temp_dir();
        dir.push(format!(
            "oxys-prebuilt-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(dir.join("configs")).unwrap();
        dir
    }
}
