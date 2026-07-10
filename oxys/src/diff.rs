//! Structural diff between two package sets.
//!
//! Pure data: this module computes what changed between a current and a desired
//! manifest and returns it. Rendering is left to each frontend (CLI, TUI).

use std::collections::BTreeMap;

use crate::manifest::Package;

/// A single package that differs between the current and desired manifests.
///
/// - `current == None` && `desired == Some` → package is being added.
/// - `current == Some` && `desired == None` → package is being removed.
/// - both `Some` (and unequal) → package is being changed.
#[derive(Debug, Clone)]
pub struct PackageChange {
    pub package: String,
    pub current: Option<Package>,
    pub desired: Option<Package>,
}

/// Compute the set of package changes needed to move from `current` to `desired`.
///
/// Packages present and identical in both are omitted. The result is sorted by
/// package atom for stable rendering.
pub fn diff_packages(current: &[Package], desired: &[Package]) -> Vec<PackageChange> {
    let current_map = current
        .iter()
        .map(|p| (p.package.clone(), p.clone()))
        .collect::<BTreeMap<_, _>>();
    let desired_map = desired
        .iter()
        .map(|p| (p.package.clone(), p.clone()))
        .collect::<BTreeMap<_, _>>();

    let mut keys = current_map
        .keys()
        .chain(desired_map.keys())
        .cloned()
        .collect::<Vec<_>>();
    keys.sort();
    keys.dedup();

    keys.into_iter()
        .filter_map(|package| {
            let current_pkg = current_map.get(&package).cloned();
            let desired_pkg = desired_map.get(&package).cloned();
            match (&current_pkg, &desired_pkg) {
                (Some(a), Some(b)) if a == b => None,
                _ => Some(PackageChange {
                    package,
                    current: current_pkg,
                    desired: desired_pkg,
                }),
            }
        })
        .collect()
}
