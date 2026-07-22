//! Pre-install package classification.
//!
//! Groups a manifest's packages into those that arrive as prebuilt binaries and
//! those configured to build from source, so the installer can show the user
//! what to expect before committing. This mirrors the binary-vs-source rule the
//! Portage resolver applies at emerge time (see [`crate::use_resolver`]) so the
//! summary never disagrees with what an install would actually do.
//!
//! Classification is pure and cheap: it reads only the manifest, with no network
//! access and no Portage tree.

use serde::{Deserialize, Serialize};

use crate::manifest::{Package, SystemManifest};

/// Where a package comes from at install time.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PackageSource {
    /// Prebuilt binary package (no download, no compile).
    Binary,
    /// Built from source (downloads sources, then compiles).
    Source,
}

/// A single classified package for display.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PackageSummaryEntry {
    /// The package atom, e.g. `gui-wm/niri`.
    pub atom: String,
    /// Custom USE flags requested for this package (only meaningful for source
    /// builds, which are the only way custom flags take effect).
    pub use_flags: Vec<String>,
    /// How this package is provisioned.
    pub source: PackageSource,
}

/// Packages grouped by provisioning source, sorted by atom within each group.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PackageSummary {
    /// Packages provisioned as prebuilt binaries.
    pub binary: Vec<PackageSummaryEntry>,
    /// Packages built from source.
    pub source: Vec<PackageSummaryEntry>,
}

impl PackageSummary {
    /// Total number of packages across both groups.
    pub fn total(&self) -> usize {
        self.binary.len() + self.source.len()
    }
}

/// Classify a single package, mirroring the resolver's decision at
/// `use_resolver::resolver` (`resolved_binary = !from_source && (prefer_binary ||
/// binary)`), plus Portage's `-bin` atom convention which the manifest treats as
/// implicitly binary.
pub fn classify(package: &Package, prefer_binary: bool) -> PackageSource {
    if package.from_source {
        return PackageSource::Source;
    }
    let is_bin_atom = package
        .package
        .rsplit('/')
        .next()
        .is_some_and(|name| name.ends_with("-bin"));
    if package.binary || prefer_binary || is_bin_atom {
        PackageSource::Binary
    } else {
        PackageSource::Source
    }
}

/// Build a display-ready summary from a compiled manifest.
pub fn summarize(manifest: &SystemManifest) -> PackageSummary {
    let mut summary = PackageSummary::default();
    for package in &manifest.packages {
        let entry = PackageSummaryEntry {
            atom: package.package.clone(),
            use_flags: package.use_flags.clone(),
            source: classify(package, manifest.prefer_binary),
        };
        match entry.source {
            PackageSource::Binary => summary.binary.push(entry),
            PackageSource::Source => summary.source.push(entry),
        }
    }
    summary.binary.sort_by(|a, b| a.atom.cmp(&b.atom));
    summary.source.sort_by(|a, b| a.atom.cmp(&b.atom));
    summary
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manifest_with(packages: Vec<Package>, prefer_binary: bool) -> SystemManifest {
        SystemManifest {
            packages,
            prefer_binary,
            ..SystemManifest::default()
        }
    }

    #[test]
    fn classify_respects_binary_source_rules() {
        // (package, prefer_binary, expected source)
        let cases = [
            (
                Package::new("media-video/ffmpeg").from_source(),
                true,
                PackageSource::Source,
            ),
            (
                Package::new("gui-wm/niri-bin"),
                false,
                PackageSource::Binary,
            ),
            (
                Package::new("app-editors/vim"),
                false,
                PackageSource::Source,
            ),
            (
                Package::new("app-editors/vim"),
                true,
                PackageSource::Binary,
            ),
            (
                Package::new("www-client/firefox").binary(true),
                false,
                PackageSource::Binary,
            ),
        ];
        for (pkg, prefer_binary, expected) in cases {
            assert_eq!(
                classify(&pkg, prefer_binary),
                expected,
                "classify({:?}, prefer_binary={prefer_binary})",
                pkg.package
            );
        }
    }

    #[test]
    fn summarize_groups_and_sorts() {
        let manifest = manifest_with(
            vec![
                Package::new("sys-apps/portage"),
                Package::new("gui-wm/niri-bin"),
                Package::new("media-video/ffmpeg").from_source(),
                Package::new("app-editors/vim").binary(true),
            ],
            false,
        );
        let summary = summarize(&manifest);
        assert_eq!(summary.total(), 4);
        let binary: Vec<&str> = summary.binary.iter().map(|e| e.atom.as_str()).collect();
        let source: Vec<&str> = summary.source.iter().map(|e| e.atom.as_str()).collect();
        // -bin atom and explicit binary flag land in the binary group, sorted.
        assert_eq!(binary, vec!["app-editors/vim", "gui-wm/niri-bin"]);
        // from_source and plain (no prefer_binary) land in source, sorted.
        assert_eq!(source, vec!["media-video/ffmpeg", "sys-apps/portage"]);
    }
}
