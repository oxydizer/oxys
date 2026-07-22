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
    assert!(!materialized.services.enabled.contains(&"seatd".to_owned()));
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

#[test]
fn selected_terminal_is_materialized_as_a_session_requirement() {
    let mut manifest = graphical_manifest();
    manifest.session.terminal = Terminal::Kitty;

    let resolved = manifest.resolved_session().unwrap();
    assert_eq!(resolved.policy.terminal, Terminal::Kitty);
    assert!(
        resolved
            .requirements
            .packages
            .contains(&"x11-terms/kitty".to_owned())
    );

    let materialized = resolved.materialize_manifest(&manifest);
    assert!(has_package(&materialized, "x11-terms/kitty"));
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
