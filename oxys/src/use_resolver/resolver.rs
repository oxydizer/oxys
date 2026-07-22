use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    fs,
    path::{Path, PathBuf},
};

use crate::manifest::{
    AudioStack, DisplayStack, InitSystem, Libc, ManifestPackage, PlannerManifest, SystemManifest,
};

use super::{
    Conflict, DecisionSource, EmergeStream, PackageMetadata, PortagePlan, UseResolution,
    UseResolverError, load_or_parse_metadata, run_emerge, write_portage_plan_config,
};

use super::repo::{discover_repo_roots, find_metadata_path, list_package_versions};
use super::rules::{
    apply_abi_consistency_rule, apply_audio_rule, apply_blocker_rule, apply_global_policy_rules,
    apply_init_system_rule, apply_libc_rule, apply_llvm_slot_rule, apply_required_use_rule,
    apply_slot_dependency_rule, apply_virtual_rule, apply_wayland_x_rule,
    collect_keyword_acceptance, collect_license_acceptance, collect_local_conflicts,
    collect_metadata_warnings, collect_use_flags_binary_conflicts, record_explicit_policy_notes,
};
use super::version::{compare_gentoo_versions, is_live_version, normalize_version};

/// Resolves package and global USE state from package input alone.
///
/// This compatibility entry point preserves the staged API. It falls back to package inference
/// for system policy because no explicit manifest-level policy is available.
pub fn resolve(
    packages: &[ManifestPackage],
    portage_tree: &Path,
    cache_dir: &Path,
) -> Result<UseResolution, UseResolverError> {
    let manifest = PlannerManifest {
        packages: packages.to_vec(),
        ..PlannerManifest::default()
    };
    plan_portage_internal(&manifest, portage_tree, cache_dir).map(|plan| plan.resolution)
}

/// Creates the Portage plan used by `oxys apply` from the declarative system manifest.
pub fn plan_portage(
    manifest: &SystemManifest,
    portage_tree: &Path,
    cache_dir: &Path,
) -> Result<PortagePlan, UseResolverError> {
    let resolved_manifest = resolve_manifest_versions(manifest, portage_tree)?;
    let manifest = PlannerManifest::from(resolved_manifest.clone());
    let mut plan = plan_portage_internal(&manifest, portage_tree, cache_dir)?;
    plan.manifest = resolved_manifest;
    Ok(plan)
}

pub fn resolve_latest_version(
    package: &str,
    portage_tree: &Path,
) -> Result<String, UseResolverError> {
    resolve_latest_version_with_policy(package, portage_tree, KeywordPolicy::PreferStable)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum KeywordPolicy {
    StableOnly,
    StableAndTesting,
    Any,
    PreferStable,
}

#[derive(Debug)]
struct VersionCandidate {
    version: String,
    stable_amd64: bool,
    testing_amd64: bool,
}

fn resolve_latest_version_with_policy(
    package: &str,
    portage_tree: &Path,
    keyword_policy: KeywordPolicy,
) -> Result<String, UseResolverError> {
    let (category, package_name) =
        package
            .split_once('/')
            .ok_or_else(|| UseResolverError::InvalidPackageIdentifier {
                package: package.to_owned(),
            })?;

    if category.is_empty() || package_name.is_empty() {
        return Err(UseResolverError::InvalidPackageIdentifier {
            package: package.to_owned(),
        });
    }

    let repo_roots = discover_repo_roots(portage_tree);
    let candidates = repo_roots
        .iter()
        .flat_map(|repo_root| {
            list_package_versions(repo_root, category, package_name)
                .into_iter()
                .map(|version| {
                    let metadata_path = repo_root
                        .join("metadata")
                        .join("md5-cache")
                        .join(category)
                        .join(format!("{package_name}-{version}"));
                    let keywords = fs::read_to_string(metadata_path)
                        .ok()
                        .and_then(|contents| {
                            contents
                                .lines()
                                .find_map(|line| line.strip_prefix("KEYWORDS="))
                                .map(str::to_owned)
                        })
                        .unwrap_or_default();
                    VersionCandidate {
                        version,
                        stable_amd64: keywords.split_whitespace().any(|item| item == "amd64"),
                        testing_amd64: keywords.split_whitespace().any(|item| item == "~amd64"),
                    }
                })
        })
        .collect::<Vec<_>>();

    let newest = |filter: &dyn Fn(&VersionCandidate) -> bool, live: bool| {
        let mut versions = candidates
            .iter()
            .filter(|candidate| is_live_version(&candidate.version) == live)
            .filter(|candidate| filter(candidate))
            .map(|candidate| candidate.version.clone())
            .collect::<Vec<_>>();
        versions.sort_by(|left, right| compare_gentoo_versions(left, right));
        versions.dedup();
        versions.pop()
    };
    let stable = |candidate: &VersionCandidate| candidate.stable_amd64;
    let stable_or_testing =
        |candidate: &VersionCandidate| candidate.stable_amd64 || candidate.testing_amd64;
    let any = |_candidate: &VersionCandidate| true;

    let selected = match keyword_policy {
        KeywordPolicy::StableOnly => newest(&stable, false),
        KeywordPolicy::StableAndTesting => newest(&stable_or_testing, false),
        KeywordPolicy::Any => newest(&any, false).or_else(|| newest(&any, true)),
        KeywordPolicy::PreferStable => newest(&stable, false)
            .or_else(|| newest(&any, false))
            .or_else(|| newest(&any, true)),
    };

    selected.ok_or_else(|| {
        if keyword_policy == KeywordPolicy::StableOnly && !candidates.is_empty() {
            UseResolverError::NoStableVersion {
                package: package.to_owned(),
            }
        } else {
            metadata_not_found(
                package,
                portage_tree
                    .join("metadata")
                    .join("md5-cache")
                    .join(category)
                    .join(format!("{package_name}-")),
                portage_tree,
            )
        }
    })
}

/// Build a `MetadataNotFound`, attaching a "did you mean" hint when the atom
/// itself looks misspelled. The index scan runs only on this error path.
fn metadata_not_found(package: &str, path: PathBuf, portage_tree: &Path) -> UseResolverError {
    UseResolverError::MetadataNotFound {
        package: package.to_owned(),
        path,
        suggestion_note: crate::package_check::suggestion_note_for(package, portage_tree),
    }
}

fn plan_portage_internal(
    manifest: &PlannerManifest,
    portage_tree: &Path,
    cache_dir: &Path,
) -> Result<PortagePlan, UseResolverError> {
    let mut states = load_package_states(
        &manifest.packages,
        manifest.prefer_binary,
        portage_tree,
        cache_dir,
    )?;
    let context = ResolutionContext::from_manifest(manifest);
    let mut global_use = BTreeSet::new();
    let mut accept_keywords = BTreeSet::new();
    let mut accept_licenses = BTreeSet::new();
    let mut conflicts = Vec::new();
    let mut warnings = Vec::new();
    let mut decisions = Vec::new();

    collect_use_flags_binary_conflicts(&states, &mut conflicts, &mut warnings);

    record_explicit_policy_notes(&context, &mut decisions);

    for state in &mut states {
        apply_wayland_x_rule(
            state,
            &context,
            &mut global_use,
            &mut warnings,
            &mut decisions,
        );
        apply_init_system_rule(
            state,
            &context,
            &mut conflicts,
            &mut warnings,
            &mut decisions,
        );
        apply_libc_rule(state, &context, &mut warnings, &mut decisions);
        apply_llvm_slot_rule(state, &mut conflicts, &mut warnings, &mut decisions);
        apply_audio_rule(
            state,
            &context,
            &mut conflicts,
            &mut warnings,
            &mut decisions,
        );
        apply_global_policy_rules(
            state,
            &context,
            &mut global_use,
            &mut conflicts,
            &mut decisions,
        );
        apply_required_use_rule(state, &mut conflicts, &mut warnings, &mut decisions);
        collect_keyword_acceptance(state, &mut accept_keywords, &mut warnings, &mut decisions);
        collect_license_acceptance(state, &mut accept_licenses, &mut warnings, &mut decisions);
        collect_metadata_warnings(state, &mut warnings);
        collect_local_conflicts(state, &mut conflicts);
    }

    for state in &states {
        apply_blocker_rule(state, &states, &mut conflicts, &mut warnings);
        apply_virtual_rule(state, &states, &mut conflicts, &mut warnings);
        apply_slot_dependency_rule(state, &states, &mut conflicts, &mut warnings);
    }

    apply_abi_consistency_rule(&states, &mut conflicts);

    if context.init_preference == Preference::Ambiguous {
        conflicts.push(Conflict {
            packages: vec!["sys-apps/openrc".to_owned(), "sys-apps/systemd".to_owned()],
            flag: "init-system".to_owned(),
            reason: "manifest selects both systemd and openrc".to_owned(),
        });
    }

    if context.audio_preference == Preference::Ambiguous {
        conflicts.push(Conflict {
            packages: vec![
                "media-sound/pulseaudio".to_owned(),
                "media-video/pipewire".to_owned(),
            ],
            flag: "audio-stack".to_owned(),
            reason: "manifest selects both pulseaudio and pipewire".to_owned(),
        });
    }

    let package_use = states
        .iter()
        .map(|state| (state.manifest.package.clone(), state.render_package_use()))
        .collect::<HashMap<_, _>>();

    warnings.sort_by(|left, right| {
        left.package
            .cmp(&right.package)
            .then_with(|| left.message.cmp(&right.message))
    });
    conflicts.sort_by(|left, right| {
        left.flag
            .cmp(&right.flag)
            .then_with(|| left.reason.cmp(&right.reason))
            .then_with(|| left.packages.cmp(&right.packages))
    });
    decisions.sort_by(|left, right| {
        left.scope
            .cmp(&right.scope)
            .then_with(|| left.package.cmp(&right.package))
            .then_with(|| left.subject.cmp(&right.subject))
            .then_with(|| left.action.cmp(&right.action))
            .then_with(|| left.source.cmp(&right.source))
            .then_with(|| left.reason.cmp(&right.reason))
    });

    let use_binpkgs = manifest
        .packages
        .iter()
        .any(|pkg| !pkg.from_source && (context.prefer_binary || pkg.binary));

    Ok(PortagePlan {
        targets: manifest
            .packages
            .iter()
            .zip(states.iter())
            .map(|(package, state)| package.versioned_atom(&state.metadata.version))
            .collect(),
        manifest: SystemManifest::default(),
        resolution: UseResolution {
            package_use,
            global_use: global_use.into_iter().collect(),
            accept_keywords: accept_keywords.into_iter().collect(),
            accept_licenses: accept_licenses.into_iter().collect(),
            conflicts,
            warnings,
            decisions,
        },
        use_binpkgs,
    })
}

fn resolve_manifest_versions(
    manifest: &SystemManifest,
    portage_tree: &Path,
) -> Result<SystemManifest, UseResolverError> {
    let mut resolved = manifest.clone();

    for package in &mut resolved.packages {
        package.version = normalize_version(package.version.take());
        if package.version.is_none() {
            let keyword_policy = if package.keywords.iter().any(|keyword| keyword == "**") {
                KeywordPolicy::Any
            } else if package.keywords.iter().any(|keyword| keyword == "~amd64") {
                KeywordPolicy::StableAndTesting
            } else {
                KeywordPolicy::StableOnly
            };
            package.version = Some(resolve_latest_version_with_policy(
                &package.package,
                portage_tree,
                keyword_policy,
            )?);
        }
    }

    Ok(resolved)
}

/// Writes Portage config and starts `emerge` for the supplied plan.
pub fn apply_portage_plan(
    plan: &PortagePlan,
    portage_config_dir: &Path,
    root: &Path,
    portage_tmpdir: &Path,
    jobs: usize,
) -> Result<EmergeStream, UseResolverError> {
    write_portage_plan_config(plan, portage_config_dir)?;
    run_emerge(
        &plan.targets,
        root,
        portage_tmpdir,
        jobs,
        plan.use_binpkgs,
        true,
    )
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Preference {
    PreferFirst,
    PreferSecond,
    Ambiguous,
    Unspecified,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PolicyOrigin {
    Manifest,
    Inferred,
    Unspecified,
}

impl PolicyOrigin {
    pub(crate) fn decision_source(self) -> DecisionSource {
        match self {
            Self::Manifest => DecisionSource::ManifestPolicy,
            Self::Inferred => DecisionSource::PackageInference,
            Self::Unspecified => DecisionSource::PackageInference,
        }
    }
}

pub(crate) struct ResolutionContext {
    pub(crate) init_preference: Preference,
    pub(crate) init_origin: PolicyOrigin,
    pub(crate) audio_preference: Preference,
    pub(crate) audio_origin: PolicyOrigin,
    pub(crate) display_preference: Preference,
    pub(crate) display_origin: PolicyOrigin,
    pub(crate) libc: Option<Libc>,
    pub(crate) prefer_binary: bool,
}

impl ResolutionContext {
    fn from_manifest(manifest: &PlannerManifest) -> Self {
        let package_names = manifest
            .packages
            .iter()
            .map(|package| package.package.as_str())
            .collect::<BTreeSet<_>>();

        let (init_preference, init_origin) = if let Some(init_system) = manifest.init_system {
            (
                match init_system {
                    InitSystem::Systemd => Preference::PreferFirst,
                    InitSystem::Openrc => Preference::PreferSecond,
                },
                PolicyOrigin::Manifest,
            )
        } else {
            (
                pair_preference(&package_names, "sys-apps/systemd", "sys-apps/openrc"),
                PolicyOrigin::Inferred,
            )
        };

        let (audio_preference, audio_origin) = if let Some(audio_stack) = manifest.audio_stack {
            (
                match audio_stack {
                    AudioStack::Pipewire => Preference::PreferFirst,
                    AudioStack::Pulseaudio => Preference::PreferSecond,
                },
                PolicyOrigin::Manifest,
            )
        } else {
            (
                pair_preference(
                    &package_names,
                    "media-video/pipewire",
                    "media-sound/pulseaudio",
                ),
                PolicyOrigin::Inferred,
            )
        };

        let (display_preference, display_origin) =
            if let Some(display_stack) = manifest.display_stack {
                (
                    match display_stack {
                        DisplayStack::Wayland => Preference::PreferFirst,
                        DisplayStack::X11 => Preference::PreferSecond,
                    },
                    PolicyOrigin::Manifest,
                )
            } else {
                (Preference::Unspecified, PolicyOrigin::Unspecified)
            };

        Self {
            init_preference,
            init_origin,
            audio_preference,
            audio_origin,
            display_preference,
            display_origin,
            libc: manifest.libc,
            prefer_binary: manifest.prefer_binary,
        }
    }
}

fn pair_preference(package_names: &BTreeSet<&str>, first: &str, second: &str) -> Preference {
    let has_first = package_names.contains(first);
    let has_second = package_names.contains(second);

    match (has_first, has_second) {
        (true, false) => Preference::PreferFirst,
        (false, true) => Preference::PreferSecond,
        (true, true) => Preference::Ambiguous,
        (false, false) => Preference::Unspecified,
    }
}

pub(crate) struct PackageState {
    pub(crate) manifest: ManifestPackage,
    pub(crate) metadata: PackageMetadata,
    /// Whether binary was requested at all (by global `prefer_binary` or an
    /// explicit per-package pin), before the use-flags-vs-binary check below.
    pub(crate) binary_requested: bool,
    pub(crate) resolved_binary: bool,
    pub(crate) available_flags: BTreeSet<String>,
    pub(crate) enabled_flags: BTreeSet<String>,
    pub(crate) explicit_flags: BTreeMap<String, bool>,
}

impl PackageState {
    pub(crate) fn new(
        manifest: ManifestPackage,
        metadata: PackageMetadata,
        prefer_binary: bool,
    ) -> Self {
        let binary_requested = !manifest.from_source && (prefer_binary || manifest.binary);
        let has_custom_use = manifest.use_flags.iter().any(|token| {
            let flag = token.trim().trim_start_matches('-');
            !flag.is_empty() && !is_l10n_flag(flag)
        });
        // Custom use_flags can't be honored on a binary package, so a binary
        // resolution driven only by the *global* prefer_binary policy (not an
        // explicit `.binary()`/`-bin` pin) quietly falls back to building
        // this one package from source instead -- exactly what `.from_source()`
        // would have done by hand. An explicit binary pin combined with custom
        // flags is a real contradiction and is left for
        // `collect_use_flags_binary_conflicts` to report as a hard conflict.
        let resolved_binary = binary_requested && (manifest.binary || !has_custom_use);
        let available_flags = metadata
            .iuse
            .iter()
            .filter(|flag| !resolved_binary || is_l10n_flag(&flag.name))
            .map(|flag| flag.name.clone())
            .collect::<BTreeSet<_>>();

        let mut enabled_flags = metadata
            .iuse
            .iter()
            .filter(|flag| flag.default_enabled)
            .map(|flag| flag.name.clone())
            .collect::<BTreeSet<_>>();

        let explicit_flags = manifest
            .use_flags
            .iter()
            .filter_map(|token| parse_manifest_flag(token))
            .filter(|(name, _)| available_flags.contains(name))
            .collect::<BTreeMap<_, _>>();

        for (flag, enabled) in &explicit_flags {
            if *enabled {
                enabled_flags.insert(flag.clone());
            } else {
                enabled_flags.remove(flag);
            }
        }

        Self {
            manifest,
            metadata,
            binary_requested,
            resolved_binary,
            available_flags,
            enabled_flags,
            explicit_flags,
        }
    }

    pub(crate) fn has_flag(&self, flag: &str) -> bool {
        self.available_flags.contains(flag)
    }

    pub(crate) fn is_enabled(&self, flag: &str) -> bool {
        self.enabled_flags.contains(flag)
    }

    pub(crate) fn is_explicit(&self, flag: &str) -> bool {
        self.explicit_flags.contains_key(flag)
    }

    /// The value the user set for `flag` in the manifest, if they set it at all.
    pub(crate) fn explicit_value(&self, flag: &str) -> Option<bool> {
        self.explicit_flags.get(flag).copied()
    }

    pub(crate) fn set_flag(&mut self, flag: &str, enabled: bool) {
        if enabled {
            self.enabled_flags.insert(flag.to_owned());
        } else {
            self.enabled_flags.remove(flag);
        }
        // Keep `explicit_flags` honest: if anything drives this flag to a value
        // that contradicts the user's explicit choice, that flag is no longer
        // the user's respected intent, so drop the explicit marker rather than
        // let `is_explicit`/`enabled_flags` disagree about the same fact.
        if self
            .explicit_flags
            .get(flag)
            .is_some_and(|explicit| *explicit != enabled)
        {
            self.explicit_flags.remove(flag);
        }
    }

    pub(crate) fn render_package_use(&self) -> Vec<String> {
        self.available_flags
            .iter()
            .map(|flag| {
                if self.enabled_flags.contains(flag) {
                    flag.clone()
                } else {
                    format!("-{flag}")
                }
            })
            .collect()
    }
}

pub(crate) fn is_l10n_flag(flag: &str) -> bool {
    flag.starts_with("l10n_")
}

fn load_package_states(
    packages: &[ManifestPackage],
    prefer_binary: bool,
    portage_tree: &Path,
    cache_dir: &Path,
) -> Result<Vec<PackageState>, UseResolverError> {
    packages
        .iter()
        .cloned()
        .map(|manifest| {
            let metadata_path = metadata_path_for_manifest(&manifest, portage_tree)?;

            let metadata = load_or_parse_metadata(&metadata_path, cache_dir)?;
            Ok(PackageState::new(manifest, metadata, prefer_binary))
        })
        .collect()
}

fn metadata_path_for_manifest(
    manifest: &ManifestPackage,
    portage_tree: &Path,
) -> Result<PathBuf, UseResolverError> {
    let (category, package_name) = match manifest.package.split_once('/') {
        Some(parts) => parts,
        None => (manifest.package.as_str(), ""),
    };

    match normalize_version(manifest.version.clone()) {
        Some(version) => find_metadata_path(portage_tree, category, package_name, &version)
            .ok_or_else(|| {
                metadata_not_found(
                    &manifest.package,
                    portage_tree
                        .join("metadata")
                        .join("md5-cache")
                        .join(category)
                        .join(format!("{package_name}-{version}")),
                    portage_tree,
                )
            }),
        None => {
            let version = resolve_latest_version(&manifest.package, portage_tree)?;
            find_metadata_path(portage_tree, category, package_name, &version).ok_or_else(|| {
                metadata_not_found(
                    &manifest.package,
                    portage_tree
                        .join("metadata")
                        .join("md5-cache")
                        .join(category)
                        .join(format!("{package_name}-{version}")),
                    portage_tree,
                )
            })
        }
    }
}

pub(crate) fn parse_manifest_flag(token: &str) -> Option<(String, bool)> {
    let trimmed = token.trim();
    if trimmed.is_empty() {
        return None;
    }

    if let Some(flag) = trimmed.strip_prefix('-') {
        let trimmed_flag = flag.trim();
        if trimmed_flag.is_empty() {
            return None;
        }
        return Some((trimmed_flag.to_owned(), false));
    }

    let trimmed = trimmed.strip_prefix('+').unwrap_or(trimmed).trim();
    if trimmed.is_empty() {
        return None;
    }

    Some((trimmed.to_owned(), true))
}
