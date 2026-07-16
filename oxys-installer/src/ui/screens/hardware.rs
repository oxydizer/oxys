use super::*;

pub(in crate::ui) fn draw_hardware_detection(frame: &mut Frame, area: Rect, app: &App) {
    let chunks = screen_chunks(area, 6, 8);
    frame.render_widget(
        Paragraph::new(section_header("step 2", "hardware detection")),
        chunks[0],
    );

    let spinner = SPINNER[app.hardware_spinner_idx];
    let status = if app.hardware_detecting {
        status_line(spinner, "detecting hardware".to_string(), ACCENT, true)
    } else if app.hardware_detect_done {
        status_line(
            "✓",
            "Hardware detection complete".to_string(),
            SUCCESS,
            true,
        )
    } else {
        status_line(
            "○",
            "hardware detection not started".to_string(),
            DIM,
            false,
        )
    };
    frame.render_widget(
        Paragraph::new(status),
        Rect {
            height: 1,
            ..chunks[1]
        },
    );

    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Length(1)])
        .split(Rect {
            y: chunks[1].y + 2,
            height: 2,
            ..chunks[1]
        });
    draw_action_row(
        frame,
        rows[0],
        "Detect hardware",
        app.hardware_action_idx == 0,
        !app.hardware_detecting,
    );
    draw_action_row(
        frame,
        rows[1],
        "Continue",
        app.hardware_action_idx == 1,
        app.hardware_detect_done,
    );

    let body = hardware_rows(app, chunks[2].width.saturating_sub(6));
    draw_focal_panel(frame, chunks[2], "detected hardware", ACCENT, body);
}
