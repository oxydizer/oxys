use super::*;

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
