//! Compile-time package-name validation against the Portage tree.
//!
//! After a config compiles to a manifest we check every declared atom for
//! existence in the configured repositories and, for unknown atoms, offer
//! "did you mean" suggestions from the enumerated category/name index. On a
//! machine with no Portage tree at all the check is skipped (the caller gets
//! [`PackageCheckOutcome::NoPortageTree`] and reports a notice), so configs
//! remain compilable on development hosts.

use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
};

use crate::manifest::SystemManifest;
use crate::use_resolver::repo;
use crate::use_resolver::util::strip_version_suffix;

/// Default location of the Portage tree, honouring `OXYS_PORTAGE_TREE`.
pub fn portage_tree_path() -> PathBuf {
    std::env::var_os("OXYS_PORTAGE_TREE")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/var/db/repos"))
}

/// An atom from the manifest that exists in no configured repository.
#[derive(Debug)]
pub struct UnknownPackage {
    pub atom: String,
    pub suggestions: Vec<String>,
}

/// Result of checking a manifest's package list.
#[derive(Debug)]
pub enum PackageCheckOutcome {
    /// The tree was enumerated; the vec holds every unknown atom (empty = all good).
    Checked(Vec<UnknownPackage>),
    /// No repository with `metadata/md5-cache` exists — check skipped.
    NoPortageTree { tree: PathBuf },
}

/// Every `category -> package names` pair known to the configured repositories.
pub struct PackageIndex {
    atoms: BTreeMap<String, BTreeSet<String>>,
}

impl PackageIndex {
    /// Enumerate all repositories under `portage_tree`. Returns `None` when no
    /// valid repo root (a directory with `metadata/md5-cache`) is found.
    pub fn load(portage_tree: &Path) -> Option<Self> {
        let roots: Vec<PathBuf> = repo::discover_repo_roots(portage_tree)
            .into_iter()
            .filter(|root| repo::is_repo_root(root))
            .collect();
        if roots.is_empty() {
            return None;
        }

        let mut atoms: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
        for root in roots {
            for category in repo::list_categories(&root) {
                let names = atoms.entry(category.clone()).or_default();
                for entry in repo::list_category_entries(&root, &category) {
                    names.insert(strip_version_suffix(&entry).to_owned());
                }
            }
        }
        Some(Self { atoms })
    }

    pub fn contains(&self, category: &str, name: &str) -> bool {
        self.atoms
            .get(category)
            .is_some_and(|names| names.contains(name))
    }

    /// Closest known atoms to `category/name`, best first, at most three.
    ///
    /// Ranking: exact name in another category first (a mis-remembered
    /// category is the most common slip), then close names within the same
    /// category, then close full atoms anywhere as a last resort.
    pub fn suggest(&self, category: &str, name: &str) -> Vec<String> {
        const LIMIT: usize = 3;
        let name_cap = if name.len() > 12 { 3 } else { 2 };

        // (tier, distance, atom) — sorted lexicographically for ranking.
        let mut candidates: Vec<(u8, usize, String)> = Vec::new();
        let full = format!("{category}/{name}");

        for (cand_category, names) in &self.atoms {
            for cand_name in names {
                let atom = format!("{cand_category}/{cand_name}");
                if cand_name == name && cand_category != category {
                    candidates.push((0, levenshtein(category, cand_category), atom));
                } else if cand_category == category {
                    let distance = levenshtein(name, cand_name);
                    if distance > 0 && distance <= name_cap {
                        candidates.push((1, distance, atom));
                    }
                } else if atom.len().abs_diff(full.len()) <= 2 {
                    // Cheap length pre-filter before the full-atom distance.
                    let distance = levenshtein(&full, &atom);
                    if distance > 0 && distance <= 2 {
                        candidates.push((2, distance, atom));
                    }
                }
            }
        }

        candidates.sort();
        candidates.dedup_by(|a, b| a.2 == b.2);
        candidates.truncate(LIMIT);
        candidates.into_iter().map(|(_, _, atom)| atom).collect()
    }
}

/// Check every atom in the manifest's package list against the tree.
pub fn check_packages(manifest: &SystemManifest, portage_tree: &Path) -> PackageCheckOutcome {
    let Some(index) = PackageIndex::load(portage_tree) else {
        return PackageCheckOutcome::NoPortageTree {
            tree: portage_tree.to_path_buf(),
        };
    };

    let mut unknown = Vec::new();
    for package in &manifest.packages {
        // Unparseable atoms are skipped, not failed: syntactic validity is
        // handled elsewhere and a false block is worse than a false pass.
        let Some((category, name)) = normalize_atom(&package.package) else {
            continue;
        };
        if !index.contains(category, name) {
            unknown.push(UnknownPackage {
                atom: package.package.clone(),
                suggestions: index.suggest(category, name),
            });
        }
    }
    PackageCheckOutcome::Checked(unknown)
}

/// A `; did you mean ...?` suffix for `atom`, or an empty string when the atom
/// exists, can't be parsed, or nothing close is known. Used to enrich
/// resolution-time "not found" errors.
pub fn suggestion_note_for(atom: &str, portage_tree: &Path) -> String {
    let Some((category, name)) = normalize_atom(atom) else {
        return String::new();
    };
    let Some(index) = PackageIndex::load(portage_tree) else {
        return String::new();
    };
    if index.contains(category, name) {
        // The package exists; the failure is about versions, not spelling.
        return String::new();
    }
    let suggestions = index.suggest(category, name);
    if suggestions.is_empty() {
        String::new()
    } else {
        format!("; did you mean '{}'?", suggestions.join("', '"))
    }
}

/// Render one report line per unknown atom.
pub fn render_unknown_packages(unknown: &[UnknownPackage]) -> String {
    let mut out = String::new();
    for pkg in unknown {
        if pkg.suggestions.is_empty() {
            out.push_str(&format!(
                "unknown package '{}' (not found in any configured Portage repository)\n",
                pkg.atom
            ));
        } else {
            out.push_str(&format!(
                "unknown package '{}' — did you mean '{}'?\n",
                pkg.atom,
                pkg.suggestions.join("', '")
            ));
        }
    }
    out
}

/// Reduce an atom to its `(category, name)` pair, tolerating version
/// operators (`=cat/foo-1.2.3`), slots (`cat/foo:4`) and USE decorations
/// (`cat/foo[flag]`). Returns `None` when the atom has no `category/name`
/// shape.
fn normalize_atom(atom: &str) -> Option<(&str, &str)> {
    let had_operator = atom.starts_with(['<', '>', '=', '~', '!']);
    let atom = atom.trim_start_matches(['<', '>', '=', '~', '!']);
    let atom = atom.split(['[', ':']).next().unwrap_or(atom);
    let (category, mut name) = atom.split_once('/')?;
    if had_operator {
        name = strip_version_suffix(name);
    }
    if category.is_empty() || name.is_empty() {
        return None;
    }
    Some((category, name))
}

/// Plain Levenshtein edit distance, two-row dynamic programming over chars.
fn levenshtein(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    if a.is_empty() {
        return b.len();
    }
    if b.is_empty() {
        return a.len();
    }

    let mut prev: Vec<usize> = (0..=b.len()).collect();
    let mut current = vec![0; b.len() + 1];
    for (i, ca) in a.iter().enumerate() {
        current[0] = i + 1;
        for (j, cb) in b.iter().enumerate() {
            let substitution = prev[j] + usize::from(ca != cb);
            current[j + 1] = substitution.min(prev[j + 1] + 1).min(current[j] + 1);
        }
        std::mem::swap(&mut prev, &mut current);
    }
    prev[b.len()]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn levenshtein_basics() {
        assert_eq!(levenshtein("", ""), 0);
        assert_eq!(levenshtein("abc", "abc"), 0);
        assert_eq!(levenshtein("abc", ""), 3);
        assert_eq!(levenshtein("", "abc"), 3);
        assert_eq!(levenshtein("kitten", "sitting"), 3);
        // Transposition counts as two edits (no Damerau extension).
        assert_eq!(levenshtein("clipboard", "clipbaord"), 2);
        assert_eq!(levenshtein("naïve", "naive"), 1);
    }

    #[test]
    fn normalize_atom_handles_decorations() {
        assert_eq!(normalize_atom("net-misc/curl"), Some(("net-misc", "curl")));
        assert_eq!(normalize_atom("=cat/foo-1.2.3"), Some(("cat", "foo")));
        assert_eq!(normalize_atom(">=cat/foo-1.2.3-r1"), Some(("cat", "foo")));
        assert_eq!(normalize_atom("cat/foo:4"), Some(("cat", "foo")));
        assert_eq!(normalize_atom("cat/foo[flag,-other]"), Some(("cat", "foo")));
        // Hyphenated names survive when no operator implies a version suffix.
        assert_eq!(
            normalize_atom("gui-apps/wl-clipboard"),
            Some(("gui-apps", "wl-clipboard"))
        );
        assert_eq!(normalize_atom("no-slash"), None);
        assert_eq!(normalize_atom("cat/"), None);
        assert_eq!(normalize_atom("/name"), None);
    }

    fn index(entries: &[(&str, &[&str])]) -> PackageIndex {
        let mut atoms: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
        for (category, names) in entries {
            atoms.insert(
                (*category).to_owned(),
                names.iter().map(|name| (*name).to_owned()).collect(),
            );
        }
        PackageIndex { atoms }
    }

    #[test]
    fn suggests_close_name_in_same_category() {
        let idx = index(&[("gui-apps", &["wl-clipboard", "foot", "wlsunset"])]);
        assert_eq!(
            idx.suggest("gui-apps", "wl-clipbord"),
            vec!["gui-apps/wl-clipboard"]
        );
    }

    #[test]
    fn exact_name_in_other_category_ranks_first() {
        let idx = index(&[("app-misc", &["curl"]), ("net-misc", &["curl", "curli"])]);
        let suggestions = idx.suggest("net-mist", "curl");
        // Both categories carry an exact-name match (tier 0), ordered by
        // category distance: net-misc (2 edits) before app-misc.
        assert_eq!(suggestions[0], "net-misc/curl");
        assert!(suggestions.contains(&"app-misc/curl".to_string()));
    }

    #[test]
    fn distance_cap_excludes_junk() {
        let idx = index(&[("net-misc", &["curl", "rsync", "dhcpcd"])]);
        assert!(idx.suggest("net-misc", "firefox").is_empty());
    }

    #[test]
    fn contains_checks_exact_pair() {
        let idx = index(&[("net-misc", &["curl"])]);
        assert!(idx.contains("net-misc", "curl"));
        assert!(!idx.contains("net-misc", "wget"));
        assert!(!idx.contains("app-misc", "curl"));
    }
}
