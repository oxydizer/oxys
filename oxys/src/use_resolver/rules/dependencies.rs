use super::*;

pub fn apply_required_use_rule(
    state: &PackageState,
    conflicts: &mut Vec<Conflict>,
    _warnings: &mut Vec<Warning>,
    _decisions: &mut Vec<PortageDecision>,
) {
    for expr in &state.metadata.required_use {
        // md5-cache IUSE defaults do not include the active profile's
        // USE_EXPAND selections. Only reject a REQUIRED_USE expression here
        // when the manifest explicitly controls every flag it references;
        // otherwise Portage is the authoritative validator once it combines
        // profile defaults with the generated package.use.
        if !referenced_required_use_flags(expr)
            .iter()
            .all(|flag| state.is_explicit(flag))
        {
            continue;
        }
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

        let blocked = all_packages.iter().find(|candidate| {
            candidate.manifest.package == dep.package && !std::ptr::eq(*candidate, state)
        });

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
    _conflicts: &mut Vec<Conflict>,
    warnings: &mut Vec<Warning>,
) {
    for dep in iter_all_dependencies(&state.metadata)
        .filter(|dep| dep.blocker.is_none() && dep.package.starts_with("virtual/"))
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

        // A manifest lists world roots, not their full transitive closure.
        // Portage selects normal virtual providers from that closure. Retain
        // the useful note when the manifest explicitly includes a provider,
        // but absence here is not an unresolved conflict.
        if !providers.is_empty() {
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
        // Blockers describe packages/slots that must *not* be selected. They
        // are not positive slot dependencies (Firefox rapid, for example,
        // blocks firefox-bin:0 and :esr while remaining a valid rapid slot).
        if dep.blocker.is_some() {
            continue;
        }
        if !dependency_condition_matches(dep, state) {
            continue;
        }

        if dep.slot.is_none() && dep.slot_operator.is_none() {
            continue;
        }

        let selected = all_packages.iter().find(|candidate| {
            candidate.manifest.package == dep.package && !std::ptr::eq(*candidate, state)
        });

        let Some(selected) = selected else {
            continue;
        };

        if let Some(required_slot) = dep.slot.as_deref()
            && selected.metadata.slot.as_deref() != Some(required_slot)
        {
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

        if let Some(required_subslot) = dep.subslot.as_deref()
            && selected.metadata.subslot.as_deref() != Some(required_subslot)
        {
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
