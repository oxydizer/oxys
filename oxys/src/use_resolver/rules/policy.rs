use std::collections::BTreeSet;

use crate::manifest::Libc;

use super::super::resolver::{PackageState, PolicyOrigin, Preference, ResolutionContext};
use super::super::{
    Conflict, DecisionAction, DecisionScope, DecisionSource, PortageDecision, Warning,
};
use super::package_decision;

struct PolicyProvenance<'a> {
    reason: &'a str,
    field: &'a str,
    value: &'a str,
}

pub fn record_explicit_policy_notes(
    context: &ResolutionContext,
    decisions: &mut Vec<PortageDecision>,
) {
    if let Some(libc) = context.libc {
        decisions.push(PortageDecision {
            scope: DecisionScope::PlannerPolicy,
            package: None,
            subject: "libc".to_owned(),
            action: DecisionAction::Note,
            source: DecisionSource::ManifestPolicy,
            reason: match libc {
                Libc::Glibc => "manifest libc policy is glibc".to_owned(),
            },
        });
    }
}

pub fn apply_wayland_x_rule(
    state: &mut PackageState,
    context: &ResolutionContext,
    global_use: &mut BTreeSet<String>,
    warnings: &mut Vec<Warning>,
    decisions: &mut Vec<PortageDecision>,
) {
    if !state.has_flag("wayland") || !state.has_flag("X") {
        return;
    }

    match context.display_preference {
        Preference::PreferFirst => {
            if !state.is_explicit("wayland") {
                state.set_flag("wayland", true);
                decisions.push(package_decision(
                    state,
                    "wayland",
                    DecisionAction::Enable,
                    context.display_origin.decision_source(),
                    "manifest display policy prefers wayland",
                ));
            }
            if !state.is_explicit("X") {
                state.set_flag("X", false);
                warnings.push(Warning {
                    package: state.manifest.package.clone(),
                    message: "resolved X vs wayland in favor of wayland".to_owned(),
                });
                decisions.push(package_decision(
                    state,
                    "X",
                    DecisionAction::Disable,
                    context.display_origin.decision_source(),
                    "manifest display policy prefers wayland",
                ));
            }
        }
        Preference::PreferSecond => {
            if !state.is_explicit("X") {
                state.set_flag("X", true);
                decisions.push(package_decision(
                    state,
                    "X",
                    DecisionAction::Enable,
                    context.display_origin.decision_source(),
                    "manifest display policy prefers X11",
                ));
            }
            if !state.is_explicit("wayland") {
                state.set_flag("wayland", false);
                warnings.push(Warning {
                    package: state.manifest.package.clone(),
                    message: "resolved X vs wayland in favor of X11".to_owned(),
                });
                decisions.push(package_decision(
                    state,
                    "wayland",
                    DecisionAction::Disable,
                    context.display_origin.decision_source(),
                    "manifest display policy prefers X11",
                ));
            }
        }
        Preference::Ambiguous => {}
        Preference::Unspecified => {
            let wayland_enabled = state.is_enabled("wayland");
            let x_enabled = state.is_enabled("X");

            if wayland_enabled
                && x_enabled
                && !state.is_explicit("wayland")
                && !state.is_explicit("X")
            {
                state.set_flag("X", false);
                warnings.push(Warning {
                    package: state.manifest.package.clone(),
                    message: "resolved X vs wayland by preferring wayland".to_owned(),
                });
                decisions.push(package_decision(
                    state,
                    "X",
                    DecisionAction::Disable,
                    DecisionSource::PackageInference,
                    "fallback display inference prefers wayland when no manifest display policy exists",
                ));
            }
        }
    }

    // Only force global `-X` when the manifest *explicitly* chose Wayland
    // (`context.display_origin == PolicyOrigin::Manifest`). Bare-default
    // configs resolve wayland/X per package via IUSE-default inference above
    // and must not additionally strip X globally — Gentoo's official binhost
    // ships desktop packages with X enabled alongside wayland, so disabling
    // it globally here would silently push every bare-default install off
    // the binhost and onto a from-source build for no explicit reason.
    if context.display_origin == PolicyOrigin::Manifest
        && state.is_enabled("wayland")
        && !state.is_enabled("X")
    {
        global_use.insert("-X".to_owned());
        decisions.push(PortageDecision {
            scope: DecisionScope::GlobalUse,
            package: None,
            subject: "X".to_owned(),
            action: DecisionAction::Disable,
            source: context.display_origin.decision_source(),
            reason: "manifest display policy prefers wayland".to_owned(),
        });
    }
}

pub fn apply_init_system_rule(
    state: &mut PackageState,
    context: &ResolutionContext,
    conflicts: &mut Vec<Conflict>,
    warnings: &mut Vec<Warning>,
    decisions: &mut Vec<PortageDecision>,
) {
    if !state.has_flag("systemd") || !state.has_flag("openrc") {
        return;
    }

    match context.init_preference {
        Preference::PreferFirst => {
            if !state.is_explicit("systemd") {
                state.set_flag("systemd", true);
                decisions.push(package_decision(
                    state,
                    "systemd",
                    DecisionAction::Enable,
                    context.init_origin.decision_source(),
                    init_reason(context.init_origin, "systemd"),
                ));
            }
            if !state.is_explicit("openrc") {
                state.set_flag("openrc", false);
                decisions.push(package_decision(
                    state,
                    "openrc",
                    DecisionAction::Disable,
                    context.init_origin.decision_source(),
                    init_reason(context.init_origin, "systemd"),
                ));
            }
            warnings.push(Warning {
                package: state.manifest.package.clone(),
                message: "resolved init flags in favor of systemd".to_owned(),
            });
        }
        Preference::PreferSecond => {
            if !state.is_explicit("openrc") {
                state.set_flag("openrc", true);
                decisions.push(package_decision(
                    state,
                    "openrc",
                    DecisionAction::Enable,
                    context.init_origin.decision_source(),
                    init_reason(context.init_origin, "openrc"),
                ));
            }
            if !state.is_explicit("systemd") {
                state.set_flag("systemd", false);
                decisions.push(package_decision(
                    state,
                    "systemd",
                    DecisionAction::Disable,
                    context.init_origin.decision_source(),
                    init_reason(context.init_origin, "openrc"),
                ));
            }
            warnings.push(Warning {
                package: state.manifest.package.clone(),
                message: "resolved init flags in favor of openrc".to_owned(),
            });
        }
        Preference::Ambiguous => conflicts.push(Conflict {
            packages: vec![state.manifest.package.clone()],
            flag: "systemd/openrc".to_owned(),
            reason: "manifest init system is ambiguous".to_owned(),
        }),
        Preference::Unspecified => {}
    }
}

fn init_reason(origin: PolicyOrigin, selected: &str) -> String {
    match origin {
        PolicyOrigin::Manifest => format!("manifest init system is {selected}"),
        PolicyOrigin::Inferred => {
            format!(
                "fallback package inference selected {selected} because the manifest does not declare an init system"
            )
        }
        PolicyOrigin::Unspecified => format!("init system remains unspecified; using {selected}"),
    }
}

pub fn apply_libc_rule(
    state: &mut PackageState,
    context: &ResolutionContext,
    warnings: &mut Vec<Warning>,
    decisions: &mut Vec<PortageDecision>,
) {
    let Some(libc) = context.libc else {
        return;
    };

    if !state.has_flag("elibc_glibc") {
        return;
    }

    match libc {
        Libc::Glibc => {
            if !state.is_explicit("elibc_glibc") {
                state.set_flag("elibc_glibc", true);
                decisions.push(package_decision(
                    state,
                    "elibc_glibc",
                    DecisionAction::Enable,
                    DecisionSource::ManifestPolicy,
                    "manifest libc policy is glibc",
                ));
            }
            warnings.push(Warning {
                package: state.manifest.package.clone(),
                message: "resolved libc flags in favor of glibc".to_owned(),
            });
        }
    }
}

pub fn apply_llvm_slot_rule(
    state: &mut PackageState,
    conflicts: &mut Vec<Conflict>,
    warnings: &mut Vec<Warning>,
    decisions: &mut Vec<PortageDecision>,
) {
    let llvm_flags = state
        .available_flags
        .iter()
        .filter_map(|flag| parse_llvm_slot_flag(flag).map(|slot| (flag.clone(), slot)))
        .collect::<Vec<_>>();

    if llvm_flags.len() < 2 {
        return;
    }

    let explicit_enabled = llvm_flags
        .iter()
        .filter(|(flag, _)| state.explicit_flags.get(flag) == Some(&true))
        .map(|(flag, slot)| (flag.clone(), *slot))
        .collect::<Vec<_>>();

    if explicit_enabled.len() > 1 {
        conflicts.push(Conflict {
            packages: vec![state.manifest.package.clone()],
            flag: "llvm-slot".to_owned(),
            reason: "multiple llvm_slot flags were explicitly enabled".to_owned(),
        });
        return;
    }

    let chosen_flag = if let Some((flag, _)) = explicit_enabled.first() {
        flag.clone()
    } else {
        llvm_flags
            .iter()
            .filter(|(flag, _)| state.is_enabled(flag))
            .max_by_key(|(_, slot)| *slot)
            .map(|(flag, _)| flag.clone())
            .unwrap_or_else(|| {
                llvm_flags
                    .iter()
                    .max_by_key(|(_, slot)| *slot)
                    .map(|(flag, _)| flag.clone())
                    .expect("llvm_flags contains at least two entries")
            })
    };

    for (flag, _) in &llvm_flags {
        if flag == &chosen_flag {
            if !state.is_explicit(flag) {
                state.set_flag(flag, true);
                decisions.push(package_decision(
                    state,
                    flag,
                    DecisionAction::Enable,
                    DecisionSource::PackageInference,
                    format!("selected highest available llvm slot {chosen_flag}"),
                ));
            }
        } else if !state.is_explicit(flag) {
            state.set_flag(flag, false);
            decisions.push(package_decision(
                state,
                flag,
                DecisionAction::Disable,
                DecisionSource::PackageInference,
                format!("selected highest available llvm slot {chosen_flag}"),
            ));
        }
    }

    warnings.push(Warning {
        package: state.manifest.package.clone(),
        message: format!("pruned llvm slot flags to {chosen_flag}"),
    });
}

fn parse_llvm_slot_flag(flag: &str) -> Option<u32> {
    let suffix = flag.strip_prefix("llvm_slot_")?;
    suffix.parse::<u32>().ok()
}

pub fn apply_audio_rule(
    state: &mut PackageState,
    context: &ResolutionContext,
    conflicts: &mut Vec<Conflict>,
    warnings: &mut Vec<Warning>,
    decisions: &mut Vec<PortageDecision>,
) {
    if !state.has_flag("pulseaudio") || !state.has_flag("pipewire") {
        return;
    }

    match context.audio_preference {
        Preference::PreferFirst => {
            if !state.is_explicit("pipewire") {
                state.set_flag("pipewire", true);
                decisions.push(package_decision(
                    state,
                    "pipewire",
                    DecisionAction::Enable,
                    context.audio_origin.decision_source(),
                    audio_reason(context.audio_origin, "pipewire"),
                ));
            }
            if !state.is_explicit("pulseaudio") {
                state.set_flag("pulseaudio", false);
                decisions.push(package_decision(
                    state,
                    "pulseaudio",
                    DecisionAction::Disable,
                    context.audio_origin.decision_source(),
                    audio_reason(context.audio_origin, "pipewire"),
                ));
            }
            warnings.push(Warning {
                package: state.manifest.package.clone(),
                message: "resolved audio flags in favor of pipewire".to_owned(),
            });
        }
        Preference::PreferSecond => {
            if !state.is_explicit("pulseaudio") {
                state.set_flag("pulseaudio", true);
                decisions.push(package_decision(
                    state,
                    "pulseaudio",
                    DecisionAction::Enable,
                    context.audio_origin.decision_source(),
                    audio_reason(context.audio_origin, "pulseaudio"),
                ));
            }
            if !state.is_explicit("pipewire") {
                state.set_flag("pipewire", false);
                decisions.push(package_decision(
                    state,
                    "pipewire",
                    DecisionAction::Disable,
                    context.audio_origin.decision_source(),
                    audio_reason(context.audio_origin, "pulseaudio"),
                ));
            }
            warnings.push(Warning {
                package: state.manifest.package.clone(),
                message: "resolved audio flags in favor of pulseaudio".to_owned(),
            });
        }
        Preference::Ambiguous => conflicts.push(Conflict {
            packages: vec![state.manifest.package.clone()],
            flag: "pulseaudio/pipewire".to_owned(),
            reason: "manifest audio stack is ambiguous".to_owned(),
        }),
        Preference::Unspecified => {}
    }
}

pub fn apply_global_policy_rules(
    state: &mut PackageState,
    context: &ResolutionContext,
    global_use: &mut BTreeSet<String>,
    conflicts: &mut Vec<Conflict>,
    decisions: &mut Vec<PortageDecision>,
) {
    if context.init_origin == PolicyOrigin::Manifest {
        match context.init_preference {
            Preference::PreferFirst => {
                let policy = PolicyProvenance {
                    reason: "manifest init system is systemd",
                    field: "init_system",
                    value: "systemd",
                };
                force_flag(state, "systemd", true, &policy, conflicts, decisions);
                force_flag(state, "openrc", false, &policy, conflicts, decisions);
            }
            Preference::PreferSecond => {
                let policy = PolicyProvenance {
                    reason: "manifest init system is openrc",
                    field: "init_system",
                    value: "openrc",
                };
                force_flag(state, "openrc", true, &policy, conflicts, decisions);
                force_flag(state, "systemd", false, &policy, conflicts, decisions);
            }
            Preference::Ambiguous | Preference::Unspecified => {}
        }
    }

    if context.display_origin == PolicyOrigin::Manifest {
        match context.display_preference {
            Preference::PreferFirst => {
                let policy = PolicyProvenance {
                    reason: "manifest display stack is wayland",
                    field: "display_stack",
                    value: "wayland",
                };
                force_flag(state, "wayland", true, &policy, conflicts, decisions);
                force_flag(state, "X", false, &policy, conflicts, decisions);
            }
            Preference::PreferSecond | Preference::Ambiguous | Preference::Unspecified => {}
        }
    }

    if context.audio_origin == PolicyOrigin::Manifest {
        match context.audio_preference {
            Preference::PreferFirst => {
                let policy = PolicyProvenance {
                    reason: "manifest audio stack is pipewire",
                    field: "audio_stack",
                    value: "pipewire",
                };
                force_flag(state, "pipewire", true, &policy, conflicts, decisions);
                force_flag(state, "pulseaudio", false, &policy, conflicts, decisions);
            }
            Preference::PreferSecond | Preference::Ambiguous | Preference::Unspecified => {}
        }
    }

    if context.display_origin == PolicyOrigin::Manifest
        && context.display_preference == Preference::PreferFirst
        && state.has_flag("wayland")
        && state.has_flag("X")
        && state.is_enabled("wayland")
        && !state.is_enabled("X")
    {
        global_use.insert("-X".to_owned());
    }
}

fn force_flag(
    state: &mut PackageState,
    flag: &str,
    enabled: bool,
    policy: &PolicyProvenance,
    conflicts: &mut Vec<Conflict>,
    decisions: &mut Vec<PortageDecision>,
) {
    if !state.has_flag(flag) {
        return;
    }

    if state.is_enabled(flag) == enabled {
        return;
    }

    if let Some(user_enabled) = state.explicit_value(flag) {
        conflicts.push(Conflict {
            packages: vec![state.manifest.package.clone()],
            flag: flag.to_owned(),
            reason: format!(
                "{package} requests {user_token} via explicit use_flags, but {field} = {value} \
                 needs {flag} {needed} — resolve by removing {user_token} or changing {field}",
                package = state.manifest.package,
                user_token = flag_token(flag, user_enabled),
                field = policy.field,
                value = policy.value,
                needed = if enabled { "enabled" } else { "disabled" },
            ),
        });
        return;
    }

    if let Some(prior) = decisions.iter().find(|decision| {
        decision.package.as_deref() == Some(state.manifest.package.as_str())
            && decision.subject == flag
            && matches!(
                decision.action,
                DecisionAction::Enable | DecisionAction::Disable
            )
    }) {
        conflicts.push(Conflict {
            packages: vec![state.manifest.package.clone()],
            flag: flag.to_owned(),
            reason: format!(
                "conflicting rules set {flag} on {package}: \"{prior}\" then \"{now}\"",
                package = state.manifest.package,
                prior = prior.reason,
                now = policy.reason,
            ),
        });
        return;
    }

    state.set_flag(flag, enabled);
    decisions.push(package_decision(
        state,
        flag,
        if enabled {
            DecisionAction::Enable
        } else {
            DecisionAction::Disable
        },
        DecisionSource::ManifestPolicy,
        policy.reason,
    ));
}

fn flag_token(flag: &str, enabled: bool) -> String {
    if enabled {
        format!("+{flag}")
    } else {
        format!("-{flag}")
    }
}

fn audio_reason(origin: PolicyOrigin, selected: &str) -> String {
    match origin {
        PolicyOrigin::Manifest => format!("manifest audio stack is {selected}"),
        PolicyOrigin::Inferred => format!(
            "fallback package inference selected {selected} because the manifest does not declare an audio stack"
        ),
        PolicyOrigin::Unspecified => format!("audio stack remains unspecified; using {selected}"),
    }
}
