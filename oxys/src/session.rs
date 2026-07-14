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

impl ResolvedSession {
    /// Validate immutable source-image pieces that cannot be satisfied by the
    /// later package-emerge step. This runs while the install plan is built,
    /// before rsync or any target mutation is returned to the executor.
    pub fn validate_source(&self, source_root: &Path) -> Result<(), SessionResolveError> {
        if self.policy.mode != ResolvedSessionMode::Graphical {
            return Ok(());
        }

        require_executable(
            source_root,
            &["usr/local/bin/oxys-login"],
            "session.login = oxys_login requires /usr/local/bin/oxys-login in the source image",
        )?;
        require_executable(
            source_root,
            &["sbin/agetty", "usr/sbin/agetty", "usr/bin/agetty"],
            "graphical tty login requires an executable agetty in the source image",
        )?;
        if matches!(
            self.policy.login,
            LoginFrontend::OxysLogin {
                fallback_tty_login: true,
                ..
            }
        ) {
            require_executable(
                source_root,
                &["bin/login", "usr/bin/login"],
                "session.login fallback_tty_login requires executable /bin/login in the source image",
            )?;
        }
        if !["etc/pam.d/login", "etc/pam.d/system-auth"]
            .iter()
            .any(|path| source_root.join(path).is_file())
        {
            return Err(invalid(
                "oxys-login requires source-image PAM service configuration at /etc/pam.d/login or /etc/pam.d/system-auth",
            ));
        }
        Ok(())
    }

    pub fn materialize_manifest(&self, manifest: &SystemManifest) -> SystemManifest {
        let mut result = manifest.clone();
        for atom in &self.requirements.packages {
            if !has_package(&result, atom) {
                let mut package = Package::new(atom);
                if atom == "media-video/pipewire" {
                    package = package
                        .from_source()
                        .use_flags(["sound-server", "pipewire-alsa"]);
                }
                result.packages.push(package);
            } else if atom == "media-video/pipewire" {
                if let Some(package) = result
                    .packages
                    .iter_mut()
                    .find(|p| package_matches(p, atom))
                {
                    for flag in ["sound-server", "pipewire-alsa"] {
                        if !package.use_flags.iter().any(|existing| existing == flag) {
                            package.use_flags.push(flag.to_owned());
                            package.from_source = true;
                        }
                    }
                }
            }
        }
        for service in &self.requirements.services {
            if !result.services.enabled.contains(service) {
                result.services.enabled.push(service.clone());
            }
            result
                .services
                .disabled
                .retain(|disabled| disabled != service);
        }
        if let Some(index) = self.policy.user_index {
            for group in &self.requirements.user_groups {
                if !result.users[index].groups.contains(group) {
                    result.users[index].groups.push(group.clone());
                }
            }
        }
        if self.policy.display_stack.is_some() {
            result.display_stack = self.policy.display_stack;
        }
        if self.policy.audio_stack.is_some() {
            result.audio_stack = self.policy.audio_stack;
        }
        result
    }

    pub fn render(&self) -> String {
        let mut lines = vec![format!(
            "session policy: {}",
            match self.policy.mode {
                ResolvedSessionMode::Text => "text",
                ResolvedSessionMode::Graphical => "graphical",
            }
        )];
        for decision in &self.decisions {
            lines.push(format!(
                "{} = {} [{}]: {}",
                decision.field, decision.value, decision.source, decision.reason
            ));
        }
        for warning in &self.warnings {
            lines.push(format!("warning: {warning}"));
        }
        if !self.requirements.packages.is_empty() {
            lines.push(format!(
                "packages: {}",
                self.requirements.packages.join(", ")
            ));
        }
        if !self.requirements.services.is_empty() {
            lines.push(format!(
                "services: {}",
                self.requirements.services.join(", ")
            ));
        }
        if !self.requirements.user_groups.is_empty() {
            lines.push(format!(
                "user groups: {}",
                self.requirements.user_groups.join(", ")
            ));
        }
        for (name, value) in &self.requirements.environment {
            lines.push(format!("environment: {name}={value}"));
        }
        for ordering in &self.requirements.startup {
            lines.push(format!("startup: {ordering}"));
        }
        lines.join("\n")
    }
}

fn require_executable(
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

fn resolve_user(manifest: &SystemManifest) -> Result<Option<usize>, SessionResolveError> {
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

fn validate_tty(login: LoginFrontend) -> Result<(), SessionResolveError> {
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

fn validate_compatibility(
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

fn package_matches(package: &Package, atom: &str) -> bool {
    package.package.trim() == atom || package.package.trim().starts_with(&format!("{atom}-"))
}

fn has_package(manifest: &SystemManifest, atom: &str) -> bool {
    manifest
        .packages
        .iter()
        .any(|package| package_matches(package, atom))
}

fn push_unique(values: &mut Vec<String>, value: &str) {
    if !values.iter().any(|existing| existing == value) {
        values.push(value.to_owned());
    }
}

fn source_for_auto(mode: SessionMode) -> DecisionSource {
    if mode == SessionMode::Auto {
        DecisionSource::LegacyInference
    } else {
        DecisionSource::Explicit
    }
}

fn decision(
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

fn invalid(message: impl Into<String>) -> SessionResolveError {
    SessionResolveError::Invalid(message.into())
}

fn seat_name(seat: SeatBackend) -> &'static str {
    match seat {
        SeatBackend::Seatd => "seatd",
        SeatBackend::Logind => "logind",
        SeatBackend::Direct => "direct",
        SeatBackend::Auto => "auto",
    }
}
fn tracker_name(tracker: SessionTracker) -> &'static str {
    match tracker {
        SessionTracker::Elogind => "elogind",
        SessionTracker::Systemd => "systemd",
        SessionTracker::Pam => "pam",
        SessionTracker::None => "none",
        SessionTracker::Auto => "auto",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manifest::{DrmDriver, DrmDrivers, Password, Session, User, VideoCard, VideoCards};
    use std::time::{SystemTime, UNIX_EPOCH};

    fn graphical_manifest() -> SystemManifest {
        SystemManifest {
            session: Session {
                mode: SessionMode::Graphical,
                desktop_shell: Some(DesktopShell::Noctalia),
                seat: SeatBackend::Seatd,
                session_tracker: SessionTracker::Elogind,
                ..Session::default()
            },
            users: vec![User::new("alex").password(Password::Prompt)],
            ..SystemManifest::default()
        }
    }

    #[test]
    fn explicit_graphical_resolves_without_auto_values() {
        let resolved = graphical_manifest().resolved_session().unwrap();
        assert_eq!(resolved.policy.mode, ResolvedSessionMode::Graphical);
        assert_eq!(resolved.policy.user_name.as_deref(), Some("alex"));
        assert_eq!(resolved.policy.seat, SeatBackend::Seatd);
        assert_eq!(resolved.policy.session_tracker, SessionTracker::Elogind);
        assert!(
            resolved
                .requirements
                .environment
                .contains(&("LIBSEAT_BACKEND".into(), "seatd".into()))
        );
    }

    #[test]
    fn text_mode_overrides_niri_package_inference() {
        let mut manifest = graphical_manifest();
        manifest.session.mode = SessionMode::Text;
        manifest.packages.push(Package::new("gui-wm/niri"));
        assert_eq!(
            manifest.resolved_session().unwrap().policy.mode,
            ResolvedSessionMode::Text
        );
    }

    #[test]
    fn text_mode_rejects_an_unsupported_oxys_login_tty_before_normalizing_frontend() {
        let manifest = SystemManifest {
            session: Session {
                mode: SessionMode::Text,
                login: LoginFrontend::OxysLogin {
                    tty: 2,
                    fallback_tty_login: true,
                },
                ..Session::default()
            },
            ..SystemManifest::default()
        };

        let error = manifest.resolved_session().unwrap_err().to_string();
        assert!(error.contains("tty 2 is unsupported"));
    }

    #[test]
    fn graphical_source_preflight_requires_login_binary_fallback_and_pam() {
        let root = test_root("session-source-preflight");
        fs::create_dir_all(root.join("usr/local/bin")).unwrap();
        fs::create_dir_all(root.join("usr/bin")).unwrap();
        fs::create_dir_all(root.join("etc/pam.d")).unwrap();
        let resolved = graphical_manifest().resolved_session().unwrap();

        let error = resolved.validate_source(&root).unwrap_err().to_string();
        assert!(error.contains("oxys-login"));

        write_executable(&root.join("usr/local/bin/oxys-login"));
        let error = resolved.validate_source(&root).unwrap_err().to_string();
        assert!(error.contains("agetty"));

        write_executable(&root.join("usr/bin/agetty"));
        let error = resolved.validate_source(&root).unwrap_err().to_string();
        assert!(error.contains("/bin/login"));

        write_executable(&root.join("usr/bin/login"));
        let error = resolved.validate_source(&root).unwrap_err().to_string();
        assert!(error.contains("PAM"));

        fs::write(root.join("etc/pam.d/login"), "auth include system-auth\n").unwrap();
        resolved.validate_source(&root).unwrap();
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn auto_niri_without_user_preserves_text_fallback() {
        let manifest = SystemManifest {
            session: Session {
                mode: SessionMode::Auto,
                ..Session::default()
            },
            packages: vec![Package::new("gui-wm/niri")],
            ..SystemManifest::default()
        };
        let resolved = manifest.resolved_session().unwrap();
        assert_eq!(resolved.policy.mode, ResolvedSessionMode::Text);
        assert_eq!(resolved.warnings.len(), 2);
        assert!(resolved.warnings[0].contains("deprecated"));
    }

    #[test]
    fn explicit_auto_is_retained_with_a_deprecation_warning() {
        let manifest = SystemManifest {
            session: Session {
                mode: SessionMode::Auto,
                ..Session::default()
            },
            ..SystemManifest::default()
        };
        let resolved = manifest.resolved_session().unwrap();
        assert_eq!(resolved.policy.mode, ResolvedSessionMode::Text);
        assert!(
            resolved
                .warnings
                .iter()
                .any(|warning| warning.contains("deprecated"))
        );
    }

    #[test]
    fn invalid_tracker_init_pair_is_rejected() {
        let mut manifest = graphical_manifest();
        manifest.session.session_tracker = SessionTracker::Systemd;
        assert!(
            manifest
                .resolved_session()
                .unwrap_err()
                .to_string()
                .contains("incompatible")
        );
    }

    #[test]
    fn explicitly_disabled_required_service_is_rejected() {
        let mut manifest = graphical_manifest();
        manifest.services.disabled.push("seatd".to_owned());
        assert!(
            manifest
                .resolved_session()
                .unwrap_err()
                .to_string()
                .contains("services.disabled")
        );
    }

    #[test]
    fn explicit_graphics_userspace_kernel_mismatch_is_rejected() {
        let mut manifest = graphical_manifest();
        manifest.hardware.graphics.mesa.video_cards = VideoCards::Explicit(vec![VideoCard::Virgl]);
        manifest.hardware.graphics.drm.drivers = DrmDrivers::Explicit(vec![DrmDriver::Intel]);
        assert!(
            manifest
                .resolved_session()
                .unwrap_err()
                .to_string()
                .contains("VirtioGpu")
        );
    }

    #[test]
    fn materialization_merges_requirements_without_duplicates() {
        let manifest = graphical_manifest();
        let resolved = manifest.resolved_session().unwrap();
        let materialized = resolved.materialize_manifest(&manifest);
        assert!(has_package(&materialized, "gui-wm/niri"));
        assert!(materialized.services.enabled.contains(&"seatd".to_owned()));
        assert!(materialized.users[0].groups.contains(&"video".to_owned()));
        assert_eq!(
            materialized
                .packages
                .iter()
                .filter(|p| package_matches(p, "gui-wm/niri"))
                .count(),
            1
        );
    }

    fn test_root(name: &str) -> std::path::PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("oxys_session_{name}_{nanos}"))
    }

    fn write_executable(path: &Path) {
        fs::write(path, "fixture").unwrap();
        fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
    }
}
