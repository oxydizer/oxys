use super::*;

pub(super) fn require_executable(
    root: &Path,
    candidates: &[&str],
    message: &str,
) -> Result<(), SessionResolveError> {
    let found = candidates.iter().any(|candidate| {
        fs::metadata(root.join(candidate))
            .is_ok_and(|metadata| metadata.is_file() && metadata.permissions().mode() & 0o111 != 0)
    });
    if found { Ok(()) } else { Err(invalid(message)) }
}

pub(super) fn resolve_user(
    manifest: &SystemManifest,
) -> Result<Option<usize>, SessionResolveError> {
    let valid = |index: usize| {
        manifest
            .users
            .get(index)
            .is_some_and(|user| !user.name.as_str().trim().is_empty())
    };
    let resolved = match &manifest.session.user {
        SessionUser::FirstConfigured => manifest
            .users
            .iter()
            .position(|user| !user.name.as_str().trim().is_empty()),
        SessionUser::Index(index) if valid(*index) => Some(*index),
        SessionUser::Index(index) => {
            return Err(invalid(format!(
                "session.user index {index} is missing or empty"
            )));
        }
        SessionUser::Named(name) if name.trim().is_empty() => {
            return Err(invalid("session.user named selector cannot be empty"));
        }
        SessionUser::Named(name) => {
            let matches = manifest
                .users
                .iter()
                .enumerate()
                .filter(|(_, user)| user.name.as_str() == name)
                .map(|(index, _)| index)
                .collect::<Vec<_>>();
            if matches.len() != 1 {
                return Err(invalid(format!(
                    "session.user named selector {name:?} matched {} configured users; expected exactly one",
                    matches.len()
                )));
            }
            Some(matches[0])
        }
    };
    Ok(resolved)
}

pub(super) fn validate_tty(login: LoginFrontend) -> Result<(), SessionResolveError> {
    let tty = match login {
        LoginFrontend::Tty { tty } | LoginFrontend::OxysLogin { tty, .. } => tty,
    };
    if tty != 1 {
        return Err(invalid(format!(
            "session.login tty {tty} is unsupported; the initial implementation supports tty1 only"
        )));
    }
    Ok(())
}

pub(super) fn validate_compatibility(
    init: InitSystem,
    seat: SeatBackend,
    tracker: SessionTracker,
) -> Result<(), SessionResolveError> {
    if init == InitSystem::Openrc && tracker == SessionTracker::Systemd {
        return Err(invalid(
            "session.session_tracker = systemd is incompatible with init_system = openrc",
        ));
    }
    if init == InitSystem::Systemd && tracker == SessionTracker::Elogind {
        return Err(invalid(
            "session.session_tracker = elogind is incompatible with init_system = systemd",
        ));
    }
    match (seat, tracker) {
        (SeatBackend::Seatd, SessionTracker::Elogind)
        | (SeatBackend::Logind, SessionTracker::Elogind)
        | (SeatBackend::Logind, SessionTracker::Systemd)
        | (SeatBackend::Direct, SessionTracker::Pam | SessionTracker::None) => Ok(()),
        (SeatBackend::Seatd, SessionTracker::Systemd) => Err(invalid(
            "session.seat = seatd with session_tracker = systemd is not supported by the initial compatibility matrix",
        )),
        (SeatBackend::Logind, _) => Err(invalid(
            "session.seat = logind requires session_tracker = elogind or systemd",
        )),
        (SeatBackend::Direct, _) => Err(invalid(
            "session.seat = direct requires session_tracker = pam or none",
        )),
        (_, SessionTracker::Auto) | (SeatBackend::Auto, _) => {
            unreachable!("auto choices resolved before validation")
        }
        _ => Err(invalid(
            "the selected session.seat and session.session_tracker combination is unsupported",
        )),
    }
}

pub(super) fn package_matches(package: &Package, atom: &str) -> bool {
    package.package.trim() == atom || package.package.trim().starts_with(&format!("{atom}-"))
}

pub(super) fn has_package(manifest: &SystemManifest, atom: &str) -> bool {
    manifest
        .packages
        .iter()
        .any(|package| package_matches(package, atom))
}

pub(super) fn push_unique(values: &mut Vec<String>, value: &str) {
    if !values.iter().any(|existing| existing == value) {
        values.push(value.to_owned());
    }
}

pub(super) fn source_for_auto(mode: SessionMode) -> DecisionSource {
    if mode == SessionMode::Auto {
        DecisionSource::LegacyInference
    } else {
        DecisionSource::Explicit
    }
}

pub(super) fn decision(
    field: &str,
    value: &str,
    source: DecisionSource,
    reason: impl Into<String>,
    affected: &[String],
) -> SessionDecision {
    SessionDecision {
        field: field.to_owned(),
        value: value.to_owned(),
        source,
        reason: reason.into(),
        affected: affected.to_vec(),
    }
}

pub(super) fn invalid(message: impl Into<String>) -> SessionResolveError {
    SessionResolveError::Invalid(message.into())
}

pub(super) fn seat_name(seat: SeatBackend) -> &'static str {
    match seat {
        SeatBackend::Seatd => "seatd",
        SeatBackend::Logind => "logind",
        SeatBackend::Direct => "direct",
        SeatBackend::Auto => "auto",
    }
}
pub(super) fn tracker_name(tracker: SessionTracker) -> &'static str {
    match tracker {
        SessionTracker::Elogind => "elogind",
        SessionTracker::Systemd => "systemd",
        SessionTracker::Pam => "pam",
        SessionTracker::None => "none",
        SessionTracker::Auto => "auto",
    }
}
