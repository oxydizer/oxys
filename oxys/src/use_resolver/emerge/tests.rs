use std::path::Path;

use super::{
    EmergeLine, ParserState, emerge_command_for_test, emerge_depclean_pretend_command_for_test,
    emerge_deselect_command_for_test, emerge_select_command_for_test, parse_emerge_line,
};

#[test]
fn merge_command_includes_oneshot_when_requested() {
    let argv = emerge_command_for_test(
        &["=app-admin/example-1.0.0".to_owned()],
        Path::new("/"),
        2,
        false,
        true,
    );
    assert!(argv.contains(&"--oneshot".to_owned()));
}

#[test]
fn merge_command_omits_oneshot_when_not_requested() {
    let argv = emerge_command_for_test(
        &["=app-admin/example-1.0.0".to_owned()],
        Path::new("/"),
        2,
        false,
        false,
    );
    assert!(!argv.contains(&"--oneshot".to_owned()));
}

#[test]
fn select_command_uses_noreplace_and_bare_atoms() {
    let argv = emerge_select_command_for_test(&["app-admin/example".to_owned()], Path::new("/"));
    assert_eq!(
        argv,
        vec![
            "emerge".to_owned(),
            "--root".to_owned(),
            "/".to_owned(),
            "--noreplace".to_owned(),
            "--select".to_owned(),
            "app-admin/example".to_owned(),
        ]
    );
}

#[test]
fn deselect_command_targets_bare_atoms() {
    let argv = emerge_deselect_command_for_test(&["app-admin/example".to_owned()], Path::new("/"));
    assert_eq!(
        argv,
        vec![
            "emerge".to_owned(),
            "--root".to_owned(),
            "/".to_owned(),
            "--deselect".to_owned(),
            "app-admin/example".to_owned(),
        ]
    );
}

#[test]
fn depclean_pretend_command_never_touches_atoms() {
    let argv = emerge_depclean_pretend_command_for_test(Path::new("/"));
    assert_eq!(
        argv,
        vec![
            "emerge".to_owned(),
            "--root".to_owned(),
            "/".to_owned(),
            "--depclean".to_owned(),
            "--pretend".to_owned(),
        ]
    );
}

#[test]
fn parses_build_start_line() {
    let mut state = ParserState::default();

    let event = parse_emerge_line(
        ">>> Emerging (1 of 1) gui-wm/niri-25.11-r1::guru",
        &mut state,
    );

    assert_eq!(
        event,
        EmergeLine::BuildStart {
            package: "gui-wm/niri".to_owned()
        }
    );
}

#[test]
fn parses_build_complete_line() {
    let mut state = ParserState::default();

    let event = parse_emerge_line(
        ">>> Completed installing gui-wm/niri-25.11-r1 into /",
        &mut state,
    );

    assert_eq!(
        event,
        EmergeLine::BuildComplete {
            package: "gui-wm/niri".to_owned()
        }
    );
}

#[test]
fn parses_fetch_events_using_current_package() {
    let mut state = ParserState::default();
    let _ = parse_emerge_line(
        ">>> Emerging (1 of 1) gui-wm/niri-25.11-r1::guru",
        &mut state,
    );

    let event = parse_emerge_line(
        ">>> Downloading 'https://example.invalid/src.tar.xz'",
        &mut state,
    );

    assert_eq!(
        event,
        EmergeLine::FetchStart {
            package: "gui-wm/niri".to_owned()
        }
    );
}

#[test]
fn parses_fetch_complete_after_fetch_start() {
    let mut state = ParserState::default();
    let _ = parse_emerge_line(">>> Fetching gui-wm/niri-25.11-r1::guru", &mut state);

    let event = parse_emerge_line(">>> Fetch completed for gui-wm/niri", &mut state);

    assert_eq!(
        event,
        EmergeLine::FetchComplete {
            package: "gui-wm/niri".to_owned()
        }
    );
}

#[test]
fn parses_error_lines_and_tracks_failed_package() {
    let mut state = ParserState::default();
    let _ = parse_emerge_line(
        ">>> Emerging (1 of 1) gui-wm/niri-25.11-r1::guru",
        &mut state,
    );

    let event = parse_emerge_line(
        "!!! gui-wm/niri-25.11-r1 failed (compile phase)",
        &mut state,
    );

    assert_eq!(
        event,
        EmergeLine::Error {
            package: Some("gui-wm/niri".to_owned()),
            message: "!!! gui-wm/niri-25.11-r1 failed (compile phase)".to_owned()
        }
    );
}

#[test]
fn treats_plain_failed_text_as_progress() {
    let mut state = ParserState::default();
    let _ = parse_emerge_line(
        ">>> Emerging (1 of 1) gui-wm/niri-25.11-r1::guru",
        &mut state,
    );

    let event = parse_emerge_line("0 failed, 128 passed", &mut state);

    assert_eq!(
        event,
        EmergeLine::BuildProgress {
            package: Some("gui-wm/niri".to_owned()),
            line: "0 failed, 128 passed".to_owned()
        }
    );
    assert_eq!(state.failed_package, None);
    assert_eq!(state.last_error_message, None);
}

#[test]
fn parses_prefixed_error_line() {
    let mut state = ParserState::default();

    let event = parse_emerge_line(" * ERROR: gui-wm/niri failed", &mut state);

    assert_eq!(
        event,
        EmergeLine::Error {
            package: Some("gui-wm/niri".to_owned()),
            message: " * ERROR: gui-wm/niri failed".to_owned()
        }
    );
}

#[test]
fn falls_back_to_progress_for_unrecognized_lines() {
    let mut state = ParserState::default();

    let event = parse_emerge_line(" * running postinst hooks", &mut state);

    assert_eq!(
        event,
        EmergeLine::BuildProgress {
            package: None,
            line: " * running postinst hooks".to_owned()
        }
    );
}
