//! Enumerate IANA zone names from a zoneinfo directory.
//!
//! Used by the installer to offer a timezone picker for configs declaring
//! [`crate::manifest::Timezone::Prompt`], and by install planning/execution to
//! validate a literal zone before it is written to the target.

use std::path::{Component, Path};

/// The zoneinfo directory shipped by sys-libs/timezone-data.
pub const ZONEINFO_PATH: &str = "/usr/share/zoneinfo";

/// Subtrees and entries under zoneinfo that are not selectable zones:
/// `right/` and `posix/` are alternate leap-second views (and `posix` is a
/// self-referencing symlink on some systems), `Factory` is the tzdata
/// placeholder zone, and `SECURITY` is documentation.
const SKIP_NAMES: &[&str] = &["right", "posix", "Factory", "SECURITY"];

/// All zone names under `zoneinfo` (e.g. `UTC`, `Europe/London`), sorted.
/// Returns an empty list when the directory is missing or unreadable.
pub fn list_timezones(zoneinfo: &Path) -> Vec<String> {
    let mut zones = Vec::new();
    collect_zones(zoneinfo, "", 0, &mut zones);
    zones.sort();
    zones
}

fn collect_zones(dir: &Path, prefix: &str, depth: usize, zones: &mut Vec<String>) {
    // Zone paths are at most a few levels deep (America/Argentina/Ushuaia);
    // the depth cap guards against symlink cycles inside odd zoneinfo trees.
    if depth > 3 {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        // Real zone components start with an uppercase letter; everything
        // else (leapseconds, tzdata.zi, zone.tab, posixrules, ...) is data.
        if !name.starts_with(|c: char| c.is_ascii_uppercase()) || SKIP_NAMES.contains(&name) {
            continue;
        }
        let zone = if prefix.is_empty() {
            name.to_owned()
        } else {
            format!("{prefix}/{name}")
        };
        let path = entry.path();
        if path.is_dir() {
            collect_zones(&path, &zone, depth + 1, zones);
        } else if path.is_file() {
            zones.push(zone);
        }
    }
}

/// Whether `name` is a safe relative zone path with an entry under
/// `zoneinfo`. Rejects anything that could escape the tree (absolute paths,
/// `..`) before touching the filesystem.
pub fn timezone_exists(zoneinfo: &Path, name: &str) -> bool {
    if name.is_empty() || name.len() > 64 {
        return false;
    }
    let relative = Path::new(name);
    let plain_components = relative
        .components()
        .all(|component| matches!(component, Component::Normal(_)));
    if !plain_components {
        return false;
    }
    zoneinfo.join(relative).is_file()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture_zoneinfo() -> tempfile::TempDir {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::write(root.join("UTC"), b"TZif").unwrap();
        std::fs::create_dir_all(root.join("Europe")).unwrap();
        std::fs::write(root.join("Europe/London"), b"TZif").unwrap();
        std::fs::create_dir_all(root.join("America/Argentina")).unwrap();
        std::fs::write(root.join("America/Argentina/Ushuaia"), b"TZif").unwrap();
        // Non-zone data files and alternate views that must not be listed.
        std::fs::write(root.join("zone.tab"), b"").unwrap();
        std::fs::write(root.join("leapseconds"), b"").unwrap();
        std::fs::write(root.join("Factory"), b"TZif").unwrap();
        std::fs::write(root.join("SECURITY"), b"").unwrap();
        std::fs::create_dir_all(root.join("right/Europe")).unwrap();
        std::fs::write(root.join("right/Europe/London"), b"TZif").unwrap();
        tmp
    }

    #[test]
    fn lists_only_real_zone_files() {
        let tmp = fixture_zoneinfo();
        let zones = list_timezones(tmp.path());
        assert_eq!(
            zones,
            vec!["America/Argentina/Ushuaia", "Europe/London", "UTC"]
        );
    }

    #[test]
    fn missing_directory_lists_nothing() {
        assert!(list_timezones(Path::new("/nonexistent/zoneinfo")).is_empty());
    }

    #[test]
    fn exists_accepts_zones_and_rejects_escapes() {
        let tmp = fixture_zoneinfo();
        assert!(timezone_exists(tmp.path(), "Europe/London"));
        assert!(timezone_exists(tmp.path(), "UTC"));
        assert!(!timezone_exists(tmp.path(), "Europe/Lodnon"));
        assert!(!timezone_exists(tmp.path(), "Europe"));
        assert!(!timezone_exists(tmp.path(), ""));
        assert!(!timezone_exists(tmp.path(), "../zoneinfo/UTC"));
        assert!(!timezone_exists(tmp.path(), "/etc/passwd"));
    }
}
