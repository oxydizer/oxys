//! Minimal md5-cache fixture builder for binary and integration tests.
//!
//! This module provides a small, reusable helper to construct a fake Portage
//! repository tree containing only the md5-cache entries required for
//! `plan_portage` / `create_plan` to function.
//!
//! # Example
//! ```ignore
//! let repo = FixtureRepo::new()
//!     .with_package("app-admin/example", &["openrc", "systemd"]);
//! ```
//!
//! The resulting `root` can be pointed at via `OXYS_PORTAGE_TREE` when
//! invoking the `oxys` binary under test.
//!
//! NOTE: This is intentionally minimal and not a real portage tree snapshot.
//! It contains only hand-crafted synthetic md5-cache files with the bare
//! structure needed for resolver walks (metadata/md5-cache/<cat>/<pkg>-<ver>).
//! It does not contain ebuilds, Manifest files, full repo metadata, or
//! any data that would validate as a real Gentoo/overlay tree. Do not assume
//! completeness or compatibility with tools that expect a full rsync/webrsync
//! snapshot.

use std::fs;

use tempfile::TempDir;

/// A temporary directory populated with the minimal md5-cache layout
/// required by oxys planning code.
pub struct FixtureRepo {
    pub root: TempDir,
}

impl FixtureRepo {
    /// Create an empty minimal fixture repo root.
    ///
    /// The directory will contain `metadata/md5-cache/` so that
    /// `is_repo_root` and path discovery treat it as a usable tree.
    pub fn new() -> Self {
        let root = TempDir::new().expect("failed to create temp dir for FixtureRepo");
        let md5_cache = root.path().join("metadata").join("md5-cache");
        fs::create_dir_all(&md5_cache).expect("failed to create md5-cache skeleton");
        Self { root }
    }

    /// Add a package entry to the md5-cache.
    ///
    /// `atom` is a "category/package" string. A fixed version "1.0.0" is used
    /// for the cache filename unless the atom already contains a detectable
    /// version suffix (e.g. "cat/foo-2.3" or "cat/bar-1.0.0-r1"), in which
    /// case that suffix is used.
    ///
    /// `iuse` lists the raw flag names to appear in the IUSE= line (without
    /// leading +/− unless you want them recorded as defaults in the metadata).
    /// KEYWORDS=amd64 is always added so the entry is resolvable without
    /// extra accept_keywords in simple cases.
    pub fn with_package(self, atom: &str, iuse: &[&str]) -> Self {
        self.with_package_metadata(atom, &metadata_contents(iuse, ""))
    }

    /// Add a package entry with custom md5-cache metadata.
    pub fn with_package_metadata(self, atom: &str, contents: &str) -> Self {
        let (category, pkg_name, version) = split_atom(atom);
        let dir = self
            .root
            .path()
            .join("metadata")
            .join("md5-cache")
            .join(&category);
        fs::create_dir_all(&dir).expect("failed to create category dir in md5-cache");

        let filename = format!("{}-{}", pkg_name, version);
        let path = dir.join(filename);
        fs::write(&path, contents).expect("failed to write md5-cache file");
        self
    }
}

fn metadata_contents(iuse: &[&str], extra: &str) -> String {
    let iuse_line = if iuse.is_empty() {
        String::new()
    } else {
        format!("IUSE={}\n", iuse.join(" "))
    };
    format!("{iuse_line}{extra}KEYWORDS=amd64\n")
}

fn split_atom(atom: &str) -> (String, String, String) {
    let (category, rest) = atom
        .split_once('/')
        .expect("atom must be in category/package form");

    // Find the rightmost '-' such that the suffix after it starts with an ASCII digit.
    // This mirrors use_resolver::util::version_split_index behavior so that
    // "foo-1.2.0-r3" splits as pkg="foo", ver="1.2.0-r3".
    if let Some((idx, _)) = rest.char_indices().rev().find(|(i, ch)| {
        if *ch != '-' {
            return false;
        }
        rest.get(i + 1..)
            .and_then(|s| s.chars().next())
            .is_some_and(|c| c.is_ascii_digit())
    }) {
        let pkg = &rest[..idx];
        let ver = &rest[idx + 1..];
        if !pkg.is_empty() {
            return (category.to_owned(), pkg.to_owned(), ver.to_owned());
        }
    }

    (category.to_owned(), rest.to_owned(), "1.0.0".to_owned())
}

#[cfg(test)]
mod tests {
    use super::FixtureRepo;

    #[test]
    fn creates_minimal_structure() {
        let f = FixtureRepo::new().with_package("gui-wm/niri", &["wayland"]);
        let root = f.root.path();
        assert!(root.join("metadata").join("md5-cache").is_dir());
        let entry = root
            .join("metadata")
            .join("md5-cache")
            .join("gui-wm")
            .join("niri-1.0.0");
        assert!(entry.is_file());
        let text = std::fs::read_to_string(&entry).unwrap();
        assert!(text.contains("IUSE=wayland"));
        assert!(text.contains("KEYWORDS=amd64"));
    }

    #[test]
    fn supports_versioned_atom() {
        let f = FixtureRepo::new().with_package("app-admin/example-2.1.0-r3", &[]);
        let entry = f
            .root
            .path()
            .join("metadata")
            .join("md5-cache")
            .join("app-admin")
            .join("example-2.1.0-r3");
        assert!(entry.is_file());
    }
}
