use std::{fmt, fs, os::unix::fs::PermissionsExt, path::Path};

use thiserror::Error;

use crate::manifest::{
    AudioStack, Compositor, DesktopShell, DisplayStack, InitSystem, LoginFrontend, Package,
    SeatBackend, SessionMode, SessionTracker, SessionUser, SystemManifest,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResolvedSessionMode {
    Text,
    Graphical,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DecisionSource {
    Explicit,
    Default,
    LegacyInference,
    Dependency,
}

impl fmt::Display for DecisionSource {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Explicit => "explicit",
            Self::Default => "default",
            Self::LegacyInference => "legacy inference",
            Self::Dependency => "dependency",
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionDecision {
    pub field: String,
    pub value: String,
    pub source: DecisionSource,
    pub reason: String,
    pub affected: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SessionRequirements {
    pub packages: Vec<String>,
    pub services: Vec<String>,
    pub user_groups: Vec<String>,
    pub environment: Vec<(String, String)>,
    pub startup: Vec<String>,
    pub pam: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionPolicy {
    pub mode: ResolvedSessionMode,
    pub user_index: Option<usize>,
    pub user_name: Option<String>,
    pub login: LoginFrontend,
    pub compositor: Option<Compositor>,
    pub desktop_shell: Option<DesktopShell>,
    pub seat: SeatBackend,
    pub session_tracker: SessionTracker,
    pub display_stack: Option<DisplayStack>,
    pub audio_stack: Option<AudioStack>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedSession {
    pub policy: SessionPolicy,
    pub requirements: SessionRequirements,
    pub decisions: Vec<SessionDecision>,
    pub warnings: Vec<String>,
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum SessionResolveError {
    #[error("invalid session configuration: {0}")]
    Invalid(String),
}

impl SystemManifest {
    pub fn resolved_session(&self) -> Result<ResolvedSession, SessionResolveError> {
        resolve_session(self)
    }
}

pub fn resolve_session(manifest: &SystemManifest) -> Result<ResolvedSession, SessionResolveError> {
    let legacy_graphical = has_package(manifest, "gui-wm/niri");
    let (mode, mode_source, mode_reason) = match manifest.session.mode {
        SessionMode::Auto if legacy_graphical => (
            ResolvedSessionMode::Graphical,
            DecisionSource::LegacyInference,
            "gui-wm/niri is present in manifest packages".to_owned(),
        ),
        SessionMode::Auto => (
            ResolvedSessionMode::Text,
            DecisionSource::LegacyInference,
            "no gui-wm/niri package is present".to_owned(),
        ),
        SessionMode::Text => (
            ResolvedSessionMode::Text,
            DecisionSource::Explicit,
            "session.mode explicitly selects text".to_owned(),
        ),
        SessionMode::Graphical => (
            ResolvedSessionMode::Graphical,
            DecisionSource::Explicit,
            "session.mode explicitly selects graphical".to_owned(),
        ),
    };

    let mut decisions = vec![decision(
        "session.mode",
        match mode {
            ResolvedSessionMode::Text => "text",
            ResolvedSessionMode::Graphical => "graphical",
        },
        mode_source,
        mode_reason,
        &[],
    )];
    let mut warnings = Vec::new();
    if manifest.session.mode == SessionMode::Auto {
        warnings.push(
            "session.mode = auto uses deprecated package-presence inference; migrate to explicit text or graphical mode"
                .to_owned(),
        );
    }

    let user_index = if mode == ResolvedSessionMode::Graphical {
        resolve_user(manifest)?
    } else {
        None
    };

    // Auto historically fell back to a text login when Niri was declared but
    // no usable account existed. Preserve that migration behavior exactly.
    let mode = if mode == ResolvedSessionMode::Graphical && user_index.is_none() {
        if manifest.session.mode == SessionMode::Auto {
            warnings.push(
                "gui-wm/niri requested a graphical session, but no configured user exists; using text login"
                    .to_owned(),
            );
            decisions.push(decision(
                "session.mode",
                "text",
                DecisionSource::LegacyInference,
                "legacy graphical login requires a configured user; preserving text fallback",
                &[],
            ));
            ResolvedSessionMode::Text
        } else {
            return Err(invalid(
                "session.mode = graphical requires session.user to resolve to exactly one non-empty configured user",
            ));
        }
    } else {
        mode
    };

    if mode == ResolvedSessionMode::Text {
        validate_tty(manifest.session.login)?;
        let login = match manifest.session.login {
            LoginFrontend::Tty { tty } => LoginFrontend::Tty { tty },
            LoginFrontend::OxysLogin { .. } => LoginFrontend::Tty { tty: 1 },
        };
        return Ok(ResolvedSession {
            policy: SessionPolicy {
                mode,
                user_index: None,
                user_name: None,
                login,
                compositor: None,
                desktop_shell: None,
                seat: SeatBackend::Direct,
                session_tracker: SessionTracker::None,
                display_stack: None,
                audio_stack: manifest.audio_stack,
            },
            requirements: SessionRequirements::default(),
            decisions,
            warnings,
        });
    }

    let user_index = user_index.expect("graphical user checked above");
    let user_name = manifest.users[user_index].name.as_str().to_owned();
    decisions.push(decision(
        "session.user",
        &user_name,
        if manifest.session.mode == SessionMode::Auto {
            DecisionSource::LegacyInference
        } else {
            DecisionSource::Explicit
        },
        "resolved the configured account that owns the graphical session",
        &[format!("user[{user_index}]")],
    ));

    let login = manifest.session.login;
    validate_tty(login)?;
    if !matches!(login, LoginFrontend::OxysLogin { .. }) {
        return Err(invalid(
            "session.mode = graphical currently requires LoginFrontend::OxysLogin",
        ));
    }

    let display_stack = manifest.display_stack.unwrap_or(DisplayStack::Wayland);
    if display_stack != DisplayStack::Wayland {
        return Err(invalid(
            "session.compositor = niri requires display_stack = wayland",
        ));
    }

    let desktop_shell = manifest
        .session
        .desktop_shell
        .or_else(|| has_package(manifest, "gui-shells/noctalia").then_some(DesktopShell::Noctalia));
    let audio_stack = manifest.audio_stack.or_else(|| {
        (has_package(manifest, "media-video/pipewire") || desktop_shell.is_some())
            .then_some(AudioStack::Pipewire)
    });
    if desktop_shell == Some(DesktopShell::Noctalia) && audio_stack != Some(AudioStack::Pipewire) {
        return Err(invalid(
            "session.desktop_shell = noctalia requires audio_stack = pipewire",
        ));
    }

    let seat = match manifest.session.seat {
        SeatBackend::Auto => match manifest.init_system {
            InitSystem::Openrc => SeatBackend::Seatd,
            InitSystem::Systemd => SeatBackend::Logind,
        },
        seat => seat,
    };
    let session_tracker = match manifest.session.session_tracker {
        SessionTracker::Auto => match manifest.init_system {
            InitSystem::Openrc => SessionTracker::Elogind,
            InitSystem::Systemd => SessionTracker::Systemd,
        },
        tracker => tracker,
    };
    validate_compatibility(manifest.init_system, seat, session_tracker)?;
    manifest
        .resolved_graphics()
        .map_err(|error| invalid(error.to_string()))?;

    decisions.extend([
        decision(
            "session.login",
            "oxys-login on tty1",
            source_for_auto(manifest.session.mode),
            "graphical session authentication and launch frontend",
            &["/etc/inittab".to_owned(), "PAM".to_owned()],
        ),
        decision(
            "session.seat",
            seat_name(seat),
            if manifest.session.seat == SeatBackend::Auto {
                DecisionSource::Default
            } else {
                DecisionSource::Explicit
            },
            "resolved a seat backend compatible with the init system",
            &["device access".to_owned()],
        ),
        decision(
            "session.session_tracker",
            tracker_name(session_tracker),
            if manifest.session.session_tracker == SessionTracker::Auto {
                DecisionSource::Default
            } else {
                DecisionSource::Explicit
            },
            "resolved session tracking and runtime-directory ownership",
            &["PAM".to_owned(), "XDG_RUNTIME_DIR".to_owned()],
        ),
    ]);

    let mut requirements = SessionRequirements::default();
    push_unique(&mut requirements.packages, "gui-wm/niri");
    push_unique(&mut requirements.packages, "sys-apps/dbus");
    push_unique(&mut requirements.services, "dbus");
    if seat == SeatBackend::Seatd {
        push_unique(&mut requirements.packages, "sys-auth/seatd");
        push_unique(&mut requirements.services, "seatd");
        for group in ["video", "input"] {
            push_unique(&mut requirements.user_groups, group);
        }
        requirements
            .environment
            .push(("LIBSEAT_BACKEND".to_owned(), "seatd".to_owned()));
    }
    if session_tracker == SessionTracker::Elogind {
        push_unique(&mut requirements.packages, "sys-auth/elogind");
        push_unique(&mut requirements.services, "elogind");
        requirements
            .pam
            .push("pam_elogind session tracking".to_owned());
    }
    if desktop_shell == Some(DesktopShell::Noctalia) {
        push_unique(&mut requirements.packages, "gui-shells/noctalia");
        push_unique(&mut requirements.packages, "sys-apps/xdg-desktop-portal");
        push_unique(&mut requirements.packages, "sys-auth/polkit");
    }
    if audio_stack == Some(AudioStack::Pipewire) {
        push_unique(&mut requirements.packages, "media-video/pipewire");
        push_unique(&mut requirements.packages, "media-video/wireplumber");
        push_unique(&mut requirements.user_groups, "audio");
    }
    requirements.environment.extend([
        ("XDG_SESSION_TYPE".to_owned(), "wayland".to_owned()),
        ("XDG_SESSION_CLASS".to_owned(), "user".to_owned()),
        ("XDG_CURRENT_DESKTOP".to_owned(), "niri".to_owned()),
    ]);
    requirements
        .startup
        .push("PAM session -> D-Bus session -> seat backend -> Niri".to_owned());
    if audio_stack == Some(AudioStack::Pipewire) {
        requirements
            .startup
            .push("Niri -> PipeWire -> WirePlumber".to_owned());
    }
    if desktop_shell == Some(DesktopShell::Noctalia) {
        requirements
            .startup
            .push("PipeWire ready -> Noctalia".to_owned());
    }

    for required in &requirements.services {
        if manifest.init_system == InitSystem::Openrc
            && manifest
                .services
                .openrc
                .runlevels()
                .any(|(_, services)| !services.is_empty())
            && !manifest.services.openrc.contains(required)
        {
            return Err(invalid(format!(
                "session requires OpenRC service {required:?}, but it is absent from the authoritative services.openrc runlevels"
            )));
        }
        if manifest
            .services
            .disabled
            .iter()
            .any(|disabled| disabled == required)
        {
            return Err(invalid(format!(
                "session requires service {required:?}, but services.disabled explicitly disables it"
            )));
        }
    }

    for package in &requirements.packages {
        decisions.push(decision(
            "session.requirement.package",
            package,
            DecisionSource::Dependency,
            "required by the resolved graphical session",
            &[package.clone()],
        ));
    }

    Ok(ResolvedSession {
        policy: SessionPolicy {
            mode,
            user_index: Some(user_index),
            user_name: Some(user_name),
            login,
            compositor: Some(manifest.session.compositor),
            desktop_shell,
            seat,
            session_tracker,
            display_stack: Some(display_stack),
            audio_stack,
        },
        requirements,
        decisions,
        warnings,
    })
}

mod helpers;
mod resolved;

use helpers::*;

#[cfg(test)]
mod tests;
