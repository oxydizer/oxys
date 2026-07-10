use std::collections::BTreeSet;

use super::{
    BlockerKind, ConditionalDep, Conflict, DecisionAction, DecisionScope, DecisionSource,
    PackageMetadata, PortageDecision, RequiredUseExpr, SlotOperator, Warning,
};

use super::resolver::{is_l10n_flag, parse_manifest_flag, PackageState};

mod policy;

pub use policy::{
    apply_audio_rule, apply_global_policy_rules, apply_init_system_rule, apply_libc_rule,
    apply_llvm_slot_rule, apply_wayland_x_rule, record_explicit_policy_notes,
};

pub fn collect_keyword_acceptance(
    state: &PackageState,
    accept_keywords: &mut BTreeSet<String>,
    warnings: &mut Vec<Warning>,
    decisions: &mut Vec<PortageDecision>,
) {
    let package = &state.manifest.package;

    for keyword in &state.manifest.keywords {
        accept_keywords.insert(format!("{keyword} {package}"));
        decisions.push(PortageDecision {
            scope: DecisionScope::AcceptKeywords,
            package: Some(package.clone()),
            subject: keyword.clone(),
            action: DecisionAction::Add,
            source: DecisionSource::ManifestPolicy,
            reason: "manifest package keyword override".to_owned(),
        });
    }

    if state
        .manifest
        .keywords
        .iter()
        .any(|keyword| keyword == "~amd64")
    {
        return;
    }

    if state
        .metadata
        .keywords
        .iter()
        .any(|keyword| keyword == "~amd64")
    {
        accept_keywords.insert(format!(
            "={}-{} ~amd64",
            state.metadata.package, state.metadata.version
        ));
        warnings.push(Warning {
            package: package.clone(),
            message: "package is keyworded ~amd64".to_owned(),
        });
        decisions.push(PortageDecision {
            scope: DecisionScope::AcceptKeywords,
            package: Some(package.clone()),
            subject: "~amd64".to_owned(),
            action: DecisionAction::Add,
            source: DecisionSource::Metadata,
            reason: "metadata contains ~amd64".to_owned(),
        });
    } else if state
        .metadata
        .keywords
        .iter()
        .any(|keyword| keyword == "**")
        && !state
            .manifest
            .keywords
            .iter()
            .any(|keyword| keyword == "**")
    {
        warnings.push(Warning {
            package: package.clone(),
            message: "package is keyworded ** and may require explicit keyword acceptance"
                .to_owned(),
        });
    }
}

pub fn collect_license_acceptance(
    state: &PackageState,
    accept_licenses: &mut BTreeSet<String>,
    warnings: &mut Vec<Warning>,
    decisions: &mut Vec<PortageDecision>,
) {
    let package = &state.manifest.package;

    for license in &state.manifest.accept_licenses {
        accept_licenses.insert(format!("{license} {package}"));
        decisions.push(PortageDecision {
            scope: DecisionScope::PlannerPolicy,
            package: Some(package.clone()),
            subject: license.clone(),
            action: DecisionAction::Add,
            source: DecisionSource::ManifestPolicy,
            reason: "manifest package license override".to_owned(),
        });
    }

    let required = state
        .metadata
        .licenses
        .iter()
        .filter(|token| is_license_token(token))
        .collect::<Vec<_>>();

    if required.is_empty() {
        return;
    }

    let accepted = state
        .manifest
        .accept_licenses
        .iter()
        .map(String::as_str)
        .collect::<BTreeSet<_>>();

    let missing = required
        .iter()
        .filter(|license| !accepted.contains(license.as_str()))
        .map(|license| (*license).clone())
        .collect::<Vec<_>>();

    if !missing.is_empty() {
        warnings.push(Warning {
            package: package.clone(),
            message: format!(
                "package license(s) {} require explicit accept_license configuration",
                missing.join(", ")
            ),
        });
    }
}

pub fn collect_metadata_warnings(state: &PackageState, warnings: &mut Vec<Warning>) {
    if !state.metadata.pdepend.is_empty() {
        warnings.push(Warning {
            package: state.manifest.package.clone(),
            message: "package declares PDEPEND entries that must be installed post-merge"
                .to_owned(),
        });
    }

    for property in &state.metadata.properties {
        match property.as_str() {
            "interactive" => warnings.push(Warning {
                package: state.manifest.package.clone(),
                message: "package is interactive and may require user input during build"
                    .to_owned(),
            }),
            "live" => warnings.push(Warning {
                package: state.manifest.package.clone(),
                message: "package is marked live and may be non-reproducible".to_owned(),
            }),
            _ => {}
        }
    }

    for restrict in &state.metadata.restrict {
        match restrict.as_str() {
            "mirror" => warnings.push(Warning {
                package: state.manifest.package.clone(),
                message:
                    "RESTRICT=mirror set; distfiles may need to be fetched from the primary source"
                        .to_owned(),
            }),
            "test" => warnings.push(Warning {
                package: state.manifest.package.clone(),
                message: "RESTRICT=test set; package tests are disabled".to_owned(),
            }),
            _ => {}
        }
    }
}

pub fn collect_local_conflicts(state: &PackageState, conflicts: &mut Vec<Conflict>) {
    if state.has_flag("wayland")
        && state.has_flag("X")
        && state.is_enabled("wayland")
        && state.is_enabled("X")
        && state.is_explicit("wayland")
        && state.is_explicit("X")
        && !specific_flag_conflict_recorded(conflicts, &state.manifest.package, "wayland", "X")
    {
        conflicts.push(Conflict {
            packages: vec![state.manifest.package.clone()],
            flag: "X/wayland".to_owned(),
            reason: "both X and wayland were explicitly enabled".to_owned(),
        });
    }

    if state.has_flag("systemd")
        && state.has_flag("openrc")
        && state.is_enabled("systemd")
        && state.is_enabled("openrc")
        && !specific_flag_conflict_recorded(conflicts, &state.manifest.package, "systemd", "openrc")
    {
        conflicts.push(Conflict {
            packages: vec![state.manifest.package.clone()],
            flag: "systemd/openrc".to_owned(),
            reason: "both systemd and openrc remain enabled".to_owned(),
        });
    }

    if state.has_flag("pulseaudio")
        && state.has_flag("pipewire")
        && state.is_enabled("pulseaudio")
        && state.is_enabled("pipewire")
        && !specific_flag_conflict_recorded(
            conflicts,
            &state.manifest.package,
            "pulseaudio",
            "pipewire",
        )
    {
        conflicts.push(Conflict {
            packages: vec![state.manifest.package.clone()],
            flag: "pulseaudio/pipewire".to_owned(),
            reason: "both pulseaudio and pipewire remain enabled".to_owned(),
        });
    }
}

fn specific_flag_conflict_recorded(
    conflicts: &[Conflict],
    package: &str,
    flag_a: &str,
    flag_b: &str,
) -> bool {
    conflicts.iter().any(|conflict| {
        (conflict.flag == flag_a || conflict.flag == flag_b)
            && conflict.packages.iter().any(|name| name == package)
    })
}

pub fn apply_required_use_rule(
    state: &PackageState,
    conflicts: &mut Vec<Conflict>,
    _warnings: &mut Vec<Warning>,
    _decisions: &mut Vec<PortageDecision>,
) {
    for expr in &state.metadata.required_use {
        if let Some(reason) = explain_required_use_violation(expr, &state.enabled_flags) {
            conflicts.push(Conflict {
                packages: vec![state.manifest.package.clone()],
                flag: render_required_use_expr(expr),
                reason: format!("REQUIRED_USE: {reason}"),
            });
        }
    }
}

pub fn apply_blocker_rule(
    state: &PackageState,
    all_packages: &[PackageState],
    conflicts: &mut Vec<Conflict>,
    warnings: &mut Vec<Warning>,
) {
    for dep in iter_all_dependencies(&state.metadata) {
        if !dependency_condition_matches(dep, state) {
            continue;
        }

        let blocked = all_packages
            .iter()
            .find(|candidate| candidate.manifest.package == dep.package);

        let Some(blocked) = blocked else {
            continue;
        };

        match dep.blocker {
            Some(BlockerKind::Hard) => conflicts.push(Conflict {
                packages: vec![
                    state.manifest.package.clone(),
                    blocked.manifest.package.clone(),
                ],
                flag: format!("!!{}", dep.package),
                reason: format!(
                    "hard blocker: !!{} cannot be installed alongside {}",
                    dep.package, state.manifest.package
                ),
            }),
            Some(BlockerKind::Soft) => warnings.push(Warning {
                package: state.manifest.package.clone(),
                message: format!(
                    "soft blocker: !{} conflicts with selected package {}",
                    dep.package, blocked.manifest.package
                ),
            }),
            None => {}
        }
    }
}

pub fn apply_virtual_rule(
    state: &PackageState,
    all_packages: &[PackageState],
    conflicts: &mut Vec<Conflict>,
    warnings: &mut Vec<Warning>,
) {
    for dep in
        iter_all_dependencies(&state.metadata).filter(|dep| dep.package.starts_with("virtual/"))
    {
        if !dependency_condition_matches(dep, state) {
            continue;
        }

        let providers = all_packages
            .iter()
            .filter(|candidate| {
                candidate.manifest.package == dep.package
                    || candidate
                        .metadata
                        .provides
                        .iter()
                        .any(|provided| provided == &dep.package)
            })
            .map(|candidate| candidate.manifest.package.clone())
            .collect::<Vec<_>>();

        if providers.is_empty() {
            conflicts.push(Conflict {
                packages: vec![state.manifest.package.clone()],
                flag: dep.package.clone(),
                reason: format!(
                    "virtual dependency {} has no selected provider",
                    dep.package
                ),
            });
        } else {
            warnings.push(Warning {
                package: state.manifest.package.clone(),
                message: format!(
                    "virtual dependency {} is satisfied by selected provider(s): {}",
                    dep.package,
                    providers.join(", ")
                ),
            });
        }
    }
}

pub fn apply_slot_dependency_rule(
    state: &PackageState,
    all_packages: &[PackageState],
    conflicts: &mut Vec<Conflict>,
    warnings: &mut Vec<Warning>,
) {
    for dep in iter_all_dependencies(&state.metadata) {
        if !dependency_condition_matches(dep, state) {
            continue;
        }

        if dep.slot.is_none() && dep.slot_operator.is_none() {
            continue;
        }

        let selected = all_packages
            .iter()
            .find(|candidate| candidate.manifest.package == dep.package);

        let Some(selected) = selected else {
            continue;
        };

        if let Some(required_slot) = dep.slot.as_deref() {
            if selected.metadata.slot.as_deref() != Some(required_slot) {
                conflicts.push(Conflict {
                    packages: vec![
                        state.manifest.package.clone(),
                        selected.manifest.package.clone(),
                    ],
                    flag: format!("{}:{}", dep.package, required_slot),
                    reason: format!(
                        "slot mismatch: {} requires {}:{} but selected package is in slot {}",
                        state.manifest.package,
                        dep.package,
                        required_slot,
                        selected.metadata.slot.as_deref().unwrap_or("(unset)")
                    ),
                });
                continue;
            }
        }

        if let Some(required_subslot) = dep.subslot.as_deref() {
            if selected.metadata.subslot.as_deref() != Some(required_subslot) {
                conflicts.push(Conflict {
                    packages: vec![
                        state.manifest.package.clone(),
                        selected.manifest.package.clone(),
                    ],
                    flag: format!(
                        "{}:{}/{}",
                        dep.package,
                        dep.slot.as_deref().unwrap_or(""),
                        required_subslot
                    ),
                    reason: format!(
                        "subslot mismatch: {} requires {} subslot {} but selected package has {}",
                        state.manifest.package,
                        dep.package,
                        required_subslot,
                        selected.metadata.subslot.as_deref().unwrap_or("(unset)")
                    ),
                });
                continue;
            }
        }

        match dep.slot_operator {
            Some(SlotOperator::Equal) => warnings.push(Warning {
                package: state.manifest.package.clone(),
                message: format!(
                    "dependency {}:{}= is subslot-sensitive and should trigger rebuilds if {} changes",
                    dep.package,
                    dep.slot
                        .as_deref()
                        .or(selected.metadata.slot.as_deref())
                        .unwrap_or(""),
                    dep.package
                ),
            }),
            Some(SlotOperator::Any) => warnings.push(Warning {
                package: state.manifest.package.clone(),
                message: format!("dependency {}:* accepts any slot and will not track subslot rebuilds", dep.package),
            }),
            None => {}
        }
    }
}

pub fn apply_abi_consistency_rule(states: &[PackageState], conflicts: &mut Vec<Conflict>) {
    for state_x in states {
        if state_x.resolved_binary || state_x.manifest.use_flags.is_empty() {
            continue;
        }

        for state_y in states {
            if !state_y.resolved_binary {
                continue;
            }

            let has_direct_dep = state_y
                .metadata
                .rdepend
                .iter()
                .any(|dep| dep.package == state_x.manifest.package)
                || state_y
                    .metadata
                    .depend
                    .iter()
                    .any(|dep| dep.package == state_x.manifest.package);

            if !has_direct_dep {
                continue;
            }

            for use_flag_token in &state_x.manifest.use_flags {
                let Some((flag_name, _)) = parse_manifest_flag(use_flag_token) else {
                    continue;
                };

                let y_cares = state_y.metadata.iuse.iter().any(|f| f.name == flag_name)
                    || state_y.metadata.rdepend.iter().any(|dep| {
                        dep.package == state_x.manifest.package
                            && dep
                                .condition
                                .as_ref()
                                .is_some_and(|c| c.contains(&flag_name))
                    })
                    || state_y.metadata.depend.iter().any(|dep| {
                        dep.package == state_x.manifest.package
                            && dep
                                .condition
                                .as_ref()
                                .is_some_and(|c| c.contains(&flag_name))
                    });

                if !y_cares {
                    continue;
                }

                let y_val = state_y.is_enabled(&flag_name);
                let x_val = state_x.is_enabled(&flag_name);

                if y_val != x_val {
                    let exists = conflicts.iter().any(|c| {
                        c.flag == "abi-consistency"
                            && c.packages.contains(&state_x.manifest.package)
                            && c.packages.contains(&state_y.manifest.package)
                            && c.reason.contains(&format!("'{}'", flag_name))
                    });

                    if !exists {
                        conflicts.push(Conflict {
                            packages: vec![
                                state_x.manifest.package.clone(),
                                state_y.manifest.package.clone(),
                            ],
                            flag: "abi-consistency".to_owned(),
                            reason: format!(
                                "binary package '{}' depends on '{}' but they have conflicting USE flags for '{}' (binary built with '{}'={}, source building with '{}'={}). Remediation: rebuild '{}' from source as well, or drop the conflicting flag from '{}'.",
                                state_y.manifest.package,
                                state_x.manifest.package,
                                flag_name,
                                flag_name,
                                y_val,
                                flag_name,
                                x_val,
                                state_y.manifest.package,
                                state_x.manifest.package
                            ),
                        });
                    }
                }
            }
        }
    }
}

fn iter_all_dependencies(metadata: &PackageMetadata) -> impl Iterator<Item = &ConditionalDep> {
    metadata
        .depend
        .iter()
        .chain(metadata.bdepend.iter())
        .chain(metadata.rdepend.iter())
        .chain(metadata.pdepend.iter())
}

fn is_license_token(token: &str) -> bool {
    !token.is_empty() && token != "(" && token != ")" && token != "||"
}

fn dependency_condition_matches(dep: &ConditionalDep, state: &PackageState) -> bool {
    dep.condition
        .as_deref()
        .map(|condition| {
            condition.split(" && ").all(|term| {
                if let Some(flag) = term.strip_prefix('!') {
                    !state.is_enabled(flag)
                } else {
                    state.is_enabled(term)
                }
            })
        })
        .unwrap_or(true)
}

fn explain_required_use_violation(
    expr: &RequiredUseExpr,
    enabled_flags: &BTreeSet<String>,
) -> Option<String> {
    match expr {
        RequiredUseExpr::Flag(flag) => {
            (!enabled_flags.contains(flag)).then(|| format!("`{flag}` must be enabled"))
        }
        RequiredUseExpr::Not(flag) => enabled_flags
            .contains(flag)
            .then(|| format!("`{flag}` must be disabled")),
        RequiredUseExpr::AnyOf(items) => {
            let enabled = items
                .iter()
                .filter(|item| required_use_expr_matches(item, enabled_flags))
                .count();
            (enabled == 0).then(|| {
                format!(
                    "at least one of {} must be enabled but 0 are",
                    render_required_use_list(items)
                )
            })
        }
        RequiredUseExpr::ExactlyOne(items) => {
            let enabled = items
                .iter()
                .filter(|item| required_use_expr_matches(item, enabled_flags))
                .count();
            (enabled != 1).then(|| {
                format!(
                    "exactly one of {} must be enabled but {} are",
                    render_required_use_list(items),
                    enabled
                )
            })
        }
        RequiredUseExpr::AtMostOne(items) => {
            let enabled = items
                .iter()
                .filter(|item| required_use_expr_matches(item, enabled_flags))
                .count();
            (enabled > 1).then(|| {
                format!(
                    "at most one of {} may be enabled but {} are",
                    render_required_use_list(items),
                    enabled
                )
            })
        }
        RequiredUseExpr::IfThen(flag, items) => {
            if enabled_flags.contains(flag) {
                items
                    .iter()
                    .find_map(|item| explain_required_use_violation(item, enabled_flags))
                    .map(|reason| format!("when `{flag}` is enabled, {reason}"))
            } else {
                None
            }
        }
        RequiredUseExpr::AllOf(items) => items
            .iter()
            .find_map(|item| explain_required_use_violation(item, enabled_flags)),
    }
}

fn required_use_expr_matches(expr: &RequiredUseExpr, enabled_flags: &BTreeSet<String>) -> bool {
    match expr {
        RequiredUseExpr::Flag(flag) => enabled_flags.contains(flag),
        RequiredUseExpr::Not(flag) => !enabled_flags.contains(flag),
        RequiredUseExpr::AnyOf(items) => items
            .iter()
            .any(|item| required_use_expr_matches(item, enabled_flags)),
        RequiredUseExpr::ExactlyOne(items) => {
            items
                .iter()
                .filter(|item| required_use_expr_matches(item, enabled_flags))
                .count()
                == 1
        }
        RequiredUseExpr::AtMostOne(items) => {
            items
                .iter()
                .filter(|item| required_use_expr_matches(item, enabled_flags))
                .count()
                <= 1
        }
        RequiredUseExpr::IfThen(flag, items) => {
            !enabled_flags.contains(flag)
                || items
                    .iter()
                    .all(|item| required_use_expr_matches(item, enabled_flags))
        }
        RequiredUseExpr::AllOf(items) => items
            .iter()
            .all(|item| required_use_expr_matches(item, enabled_flags)),
    }
}

fn render_required_use_expr(expr: &RequiredUseExpr) -> String {
    match expr {
        RequiredUseExpr::Flag(flag) => flag.clone(),
        RequiredUseExpr::Not(flag) => format!("!{flag}"),
        RequiredUseExpr::AnyOf(items) => format!("|| ( {} )", render_required_use_items(items)),
        RequiredUseExpr::ExactlyOne(items) => {
            format!("^^ ( {} )", render_required_use_items(items))
        }
        RequiredUseExpr::AtMostOne(items) => {
            format!("?? ( {} )", render_required_use_items(items))
        }
        RequiredUseExpr::IfThen(flag, items) => {
            format!("{flag}? ( {} )", render_required_use_items(items))
        }
        RequiredUseExpr::AllOf(items) => format!("( {} )", render_required_use_items(items)),
    }
}

fn render_required_use_items(items: &[RequiredUseExpr]) -> String {
    items
        .iter()
        .map(render_required_use_expr)
        .collect::<Vec<_>>()
        .join(" ")
}

fn render_required_use_list(items: &[RequiredUseExpr]) -> String {
    format!(
        "[{}]",
        items
            .iter()
            .map(render_required_use_expr)
            .collect::<Vec<_>>()
            .join(", ")
    )
}

pub(super) fn package_decision(
    state: &PackageState,
    subject: &str,
    action: DecisionAction,
    source: DecisionSource,
    reason: impl Into<String>,
) -> PortageDecision {
    PortageDecision {
        scope: DecisionScope::PackageUse,
        package: Some(state.manifest.package.clone()),
        subject: subject.to_owned(),
        action,
        source,
        reason: reason.into(),
    }
}

pub fn collect_use_flags_binary_conflicts(
    states: &[PackageState],
    conflicts: &mut Vec<Conflict>,
    warnings: &mut Vec<Warning>,
) {
    for state in states {
        let has_non_l10n_use = state.manifest.use_flags.iter().any(|token| {
            let flag = token.trim().trim_start_matches('-');
            !flag.is_empty() && !is_l10n_flag(flag)
        });
        if !has_non_l10n_use {
            continue;
        }

        if state.resolved_binary {
            // `PackageState::new` only leaves `resolved_binary` true alongside
            // custom use_flags when the package was *explicitly* pinned binary
            // (`.binary(true)` or a `-bin` name) -- a real contradiction the
            // caller must resolve by hand, so this stays a hard conflict.
            conflicts.push(Conflict {
                packages: vec![state.manifest.package.clone()],
                flag: "use-flags-vs-binary".to_owned(),
                reason: format!(
                    "{}: use_flags set but package will install from binary (binary package (or -bin suffix)). Add .from_source() to build with these flags, or remove use_flags().",
                    state.manifest.package
                ),
            });
        } else if state.binary_requested {
            // Binary was only requested by the global prefer_binary policy;
            // the custom use_flags silently won the tie-break in
            // `PackageState::new` (source build), so just surface why.
            warnings.push(Warning {
                package: state.manifest.package.clone(),
                message: format!(
                    "prefer_binary=true requested a binary package, but explicit use_flags require building from source -- falling back to source for {}",
                    state.manifest.package
                ),
            });
        }
    }
}
