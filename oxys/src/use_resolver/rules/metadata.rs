use super::*;

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

fn is_license_token(token: &str) -> bool {
    !token.is_empty() && token != "(" && token != ")" && token != "||"
}
