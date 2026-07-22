//! Integration tests for the compile-time package existence check against a
//! fixture Portage tree.
//!
//! Pure suggestion ranking / levenshtein coverage lives in the package_check
//! unit tests. This file covers FS load, version collapsing, and the public
//! check_packages API. FixtureRepo helper shape is also pinned here so those
//! tests run once (not once per integration binary that includes support/).

mod support;

use support::fixture_repo::FixtureRepo;

use oxys::manifest::{Package, SystemManifest};
use oxys::package_check::{PackageCheckOutcome, PackageIndex, check_packages, suggestion_note_for};

fn manifest_with(atoms: &[&str]) -> SystemManifest {
    SystemManifest {
        packages: atoms.iter().map(|atom| Package::new(*atom)).collect(),
        ..Default::default()
    }
}

#[test]
fn fixture_repo_creates_minimal_structure() {
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
fn fixture_repo_supports_versioned_atom() {
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

#[test]
fn check_packages_reports_only_unknown_atoms() {
    let repo = FixtureRepo::new()
        .with_package("gui-apps/wl-clipboard", &[])
        .with_package("net-misc/curl", &[]);
    let manifest = manifest_with(&["net-misc/curl", "gui-apps/wl-clipbord"]);
    match check_packages(&manifest, repo.root.path()) {
        PackageCheckOutcome::Checked(unknown) => {
            assert_eq!(unknown.len(), 1);
            assert_eq!(unknown[0].atom, "gui-apps/wl-clipbord");
            assert_eq!(unknown[0].suggestions, vec!["gui-apps/wl-clipboard"]);
        }
        PackageCheckOutcome::NoPortageTree { .. } => panic!("fixture tree should be found"),
    }
}

#[test]
fn empty_directory_is_no_portage_tree() {
    let empty = tempfile::tempdir().unwrap();
    let manifest = manifest_with(&["net-misc/curl"]);
    assert!(matches!(
        check_packages(&manifest, empty.path()),
        PackageCheckOutcome::NoPortageTree { .. }
    ));
}

#[test]
fn versioned_entries_collapse_to_one_name() {
    let repo = FixtureRepo::new()
        .with_package("dev-lang/rust-1.85.0", &[])
        .with_package("dev-lang/rust-1.86.0-r1", &[]);
    let index = PackageIndex::load(repo.root.path()).expect("fixture should be a valid repo");
    assert!(index.contains("dev-lang", "rust"));
    let manifest = manifest_with(&["dev-lang/rust"]);
    match check_packages(&manifest, repo.root.path()) {
        PackageCheckOutcome::Checked(unknown) => assert!(unknown.is_empty()),
        PackageCheckOutcome::NoPortageTree { .. } => panic!("fixture tree should be found"),
    }
}

#[test]
fn suggestion_note_only_fires_for_misspellings() {
    let repo = FixtureRepo::new().with_package("net-misc/curl", &[]);
    // Existing package (a version problem, not a spelling problem): no note.
    assert_eq!(suggestion_note_for("net-misc/curl", repo.root.path()), "");
    // Misspelled: note carries the suggestion.
    assert_eq!(
        suggestion_note_for("net-misc/crul", repo.root.path()),
        "; did you mean 'net-misc/curl'?"
    );
}
