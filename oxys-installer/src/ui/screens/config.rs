use super::*;

pub(in crate::ui) fn draw_config(frame: &mut Frame, area: Rect, app: &App) {
    let chunks = screen_chunks(area, 1, 10);
    frame.render_widget(
        Paragraph::new(section_header("step 4", "base configuration")),
        chunks[0],
    );

    // No options above the focal panel anymore -- selection is only inside the box.
    let descriptions = [
        ("desktop.fe2o3", "Windowing system and common applications"),
        ("base.fe2o3", "Minimal system, no desktop environment"),
        ("custom", "Point to your own config source"),
    ];

    let selected = app.config_idx;
    let mut body = vec![
        Line::from(Span::styled(
            "Select a base profile:",
            Style::default().fg(DIM),
        )),
        Line::from(""),
    ];
    for (i, (name, desc)) in descriptions.iter().enumerate() {
        let is_sel = selected == i;
        let display = app.config_display_name(name);
        if is_sel {
            body.push(Line::from(vec![
                Span::styled(
                    "› ",
                    Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    display,
                    Style::default().fg(FG).add_modifier(Modifier::BOLD),
                ),
            ]));
            body.push(Line::from(Span::styled(
                format!("   {}", desc),
                Style::default().fg(FG),
            )));
        } else {
            body.push(Line::from(vec![
                Span::raw("  "),
                Span::styled(display, Style::default().fg(DIM)),
            ]));
            body.push(Line::from(Span::styled(
                format!("   {}", desc),
                Style::default().fg(FAINT),
            )));
        }
        if i + 1 < descriptions.len() {
            body.push(Line::from(""));
        }
    }
    body.push(Line::from(""));
    body.push(Line::from(Span::styled(
        "Ctrl+G to edit selected profile with nano",
        Style::default().fg(FAINT),
    )));
    draw_focal_panel(frame, chunks[2], "profiles", ACCENT, body);
}

pub(in crate::ui) fn draw_custom_source(frame: &mut Frame, area: Rect, app: &App) {
    let chunks = screen_chunks(area, 2, 8);
    frame.render_widget(
        Paragraph::new(section_header("step 4", "custom config source")),
        chunks[0],
    );

    let status = if app.custom_fetching {
        let spinner = SPINNER[app.hardware_spinner_idx % SPINNER.len()];
        status_line(spinner, "fetching config".to_string(), ACCENT, true)
    } else {
        status_line(
            "•",
            "point at a local file path or an http(s) URL".to_string(),
            ACCENT,
            true,
        )
    };
    frame.render_widget(Paragraph::new(status), chunks[1]);

    let mut body = vec![
        Line::from(vec![
            Span::styled(format!("{:<10}", "source"), Style::default().fg(DIM)),
            Span::styled(
                format!("{}█", app.custom_source_input),
                Style::default().fg(FG),
            ),
        ]),
        Line::from(""),
    ];

    if let Some(error) = &app.custom_source_error {
        body.push(Line::from(Span::styled(
            error.clone(),
            Style::default().fg(WARN),
        )));
        body.push(Line::from(""));
    }

    body.push(Line::from(Span::styled(
        "Leave blank and press enter to use the built-in custom.fe2o3 template.",
        Style::default().fg(FAINT),
    )));
    body.push(Line::from(Span::styled(
        "Or type a local path (e.g. /root/my-config.fe2o3) or a URL (https://…) and press enter.",
        Style::default().fg(FAINT),
    )));

    draw_focal_panel(frame, chunks[2], "source", ACCENT, body);
}

pub(in crate::ui) fn draw_config_validate(frame: &mut Frame, area: Rect, app: &App) {
    let chunks = screen_chunks(area, 2, 8);
    frame.render_widget(
        Paragraph::new(section_header("step 4", "compiling config")),
        chunks[0],
    );

    let spinner = SPINNER[app.hardware_spinner_idx % SPINNER.len()];
    frame.render_widget(
        Paragraph::new(status_line(
            spinner,
            "compiling selected config".to_string(),
            ACCENT,
            true,
        )),
        chunks[1],
    );

    let body = vec![
        progress_line(app.compile_progress, chunks[2].width.saturating_sub(10)),
        Line::from(Span::styled(
            "estimated — completion is confirmed by Cargo, not the timer",
            Style::default().fg(FAINT),
        )),
        Line::from(""),
        Line::from(Span::styled(
            "Building the config into a checked manifest.toml…",
            Style::default().fg(DIM),
        )),
        Line::from(""),
        Line::from(Span::styled(
            "Unedited stock profiles finish quickly from a prebuilt cache;",
            Style::default().fg(FAINT),
        )),
        Line::from(Span::styled(
            "edited or custom configs compile here (oxys crate is pre-warmed).",
            Style::default().fg(FAINT),
        )),
    ];
    draw_focal_panel(frame, chunks[2], "compile", ACCENT, body);
}

pub(in crate::ui) fn draw_config_error(frame: &mut Frame, area: Rect, app: &App) {
    let chunks = screen_chunks(area, 2, 8);
    frame.render_widget(
        Paragraph::new(section_header("step 4", "config error")),
        chunks[0],
    );

    let (headline, output) = match &app.compile_error {
        Some(err) => (err.to_string(), err.output.clone()),
        None => ("config failed to compile".to_string(), String::new()),
    };
    frame.render_widget(
        Paragraph::new(status_line("✗", headline, WARN, true)),
        chunks[1],
    );

    // Scrollable compiler output: slice from the scroll offset and let the
    // focal panel clip to its height.
    let visible = chunks[2].height.saturating_sub(4).max(1) as usize;
    let lines: Vec<&str> = if output.is_empty() {
        Vec::new()
    } else {
        output.lines().collect()
    };
    let max_scroll = lines.len().saturating_sub(visible);
    let scroll = app.compile_scroll.min(max_scroll);

    let mut body: Vec<Line<'static>> = if lines.is_empty() {
        vec![Line::from(Span::styled(
            "no compiler output captured".to_string(),
            Style::default().fg(DIM),
        ))]
    } else {
        lines
            .iter()
            .skip(scroll)
            .map(|line| Line::from(Span::styled((*line).to_string(), Style::default().fg(FG))))
            .collect()
    };
    if scroll < max_scroll {
        body.push(Line::from(Span::styled(
            "  ↓ more (PgDn)".to_string(),
            Style::default().fg(FAINT),
        )));
    }
    draw_focal_panel(frame, chunks[2], "compiler output", WARN, body);
}
