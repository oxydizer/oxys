use super::*;

pub(in crate::ui) fn draw_confirm(frame: &mut Frame, area: Rect, app: &App) {
    let chunks = screen_chunks(area, 2, 8);
    frame.render_widget(
        Paragraph::new(section_header("step 5", "confirm")),
        chunks[0],
    );

    let permission_error = provisioning::install_permission_error();
    let warning_text = if app.confirm_view_manifest {
        "Viewing generated manifest.toml. This screen is read-only."
    } else if permission_error.is_some() {
        "Installer is not running as root. Enter will show the privilege error."
    } else {
        "Enter wipes the selected disk and mounts the target root."
    };
    let warning = status_line("!", warning_text.to_string(), WARN, true);
    frame.render_widget(Paragraph::new(warning), chunks[1]);

    if app.confirm_view_manifest {
        draw_manifest_preview(frame, chunks[2], app);
        return;
    }

    let body = vec![
        kv_line("hardware", &app.hardware_full, 12),
        Line::from(""),
        rule_line(chunks[2].width.saturating_sub(6)),
        Line::from(""),
        kv_line("disk", &app.selected_disk(), 12),
        Line::from(""),
        kv_line("filesystem", app.selected_layout_label(), 12),
        Line::from(""),
        kv_line("profile", app.selected_config(), 12),
        Line::from(""),
        kv_line("target", TARGET_MOUNT, 12),
        Line::from(""),
        rule_line(chunks[2].width.saturating_sub(6)),
        Line::from(""),
    ];

    draw_focal_panel(frame, chunks[2], "install plan", ACCENT, body);
}

pub(in crate::ui) fn draw_timezone(frame: &mut Frame, area: Rect, app: &App) {
    let chunks = screen_chunks(area, 2, 8);
    frame.render_widget(
        Paragraph::new(section_header("step 5", "timezone")),
        chunks[0],
    );

    let zones = app.filtered_timezones();
    frame.render_widget(
        Paragraph::new(status_line(
            "•",
            "pick the system timezone".to_string(),
            ACCENT,
            true,
        )),
        chunks[1],
    );

    // A timezone picker only needs a short viewport. Capping the panel keeps
    // it from stretching to the full terminal height on large displays.
    let panel = Rect {
        height: chunks[2].height.min(15),
        ..chunks[2]
    };

    let mut body = vec![
        Line::from(vec![
            Span::styled(format!("{:<10}", "filter"), Style::default().fg(DIM)),
            Span::styled(format!("{}█", app.timezone_filter), Style::default().fg(FG)),
            Span::styled(
                format!("   {} match(es)", zones.len()),
                Style::default().fg(FAINT),
            ),
        ]),
        Line::from(""),
    ];

    // Fixed rows for the filter line, spacing, and the hint below; the rest
    // of the panel is the list viewport, kept scrolled around the cursor.
    let viewport = (panel.height.saturating_sub(4) as usize)
        .saturating_sub(4)
        .max(3);
    let cursor = app.timezone_cursor.min(zones.len().saturating_sub(1));
    let first = cursor
        .saturating_sub(viewport / 2)
        .min(zones.len().saturating_sub(viewport));

    if zones.is_empty() {
        body.push(Line::from(Span::styled(
            "no timezones match this filter",
            Style::default().fg(WARN),
        )));
    }
    for (i, zone) in zones.iter().enumerate().skip(first).take(viewport) {
        if i == cursor {
            body.push(Line::from(vec![
                Span::styled(
                    "› ",
                    Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    (*zone).to_owned(),
                    Style::default().fg(FG).add_modifier(Modifier::BOLD),
                ),
            ]));
        } else {
            body.push(Line::from(vec![
                Span::raw("  "),
                Span::styled((*zone).to_owned(), Style::default().fg(DIM)),
            ]));
        }
    }

    body.push(Line::from(""));
    body.push(Line::from(Span::styled(
        "Type to filter, up/down to move, enter to select.",
        Style::default().fg(FAINT),
    )));

    draw_focal_panel(frame, panel, "timezone", ACCENT, body);
}

pub(in crate::ui) fn draw_usernames(frame: &mut Frame, area: Rect, app: &App) {
    let chunks = screen_chunks(area, 2, 8);
    frame.render_widget(
        Paragraph::new(section_header("step 5", "account name")),
        chunks[0],
    );

    let total = app.prompt_username_indices.len();
    let position = (app.username_idx + 1).min(total.max(1));
    frame.render_widget(
        Paragraph::new(status_line(
            "•",
            format!("choose a login name for account {position} of {total}"),
            ACCENT,
            true,
        )),
        chunks[1],
    );

    let mut body = vec![
        Line::from(vec![
            Span::styled(format!("{:<10}", "username"), Style::default().fg(DIM)),
            Span::styled(format!("{}█", app.username_input), Style::default().fg(FG)),
        ]),
        Line::from(""),
    ];

    if let Some(error) = &app.username_error {
        body.push(Line::from(Span::styled(
            error.clone(),
            Style::default().fg(WARN),
        )));
        body.push(Line::from(""));
    }

    body.push(Line::from(Span::styled(
        "Type a login name (e.g. lowercase letters, digits, - or _) and press enter.",
        Style::default().fg(FAINT),
    )));

    draw_focal_panel(frame, chunks[2], "username", ACCENT, body);
}

pub(in crate::ui) fn draw_passwords(frame: &mut Frame, area: Rect, app: &App) {
    let chunks = screen_chunks(area, 2, 8);
    frame.render_widget(
        Paragraph::new(section_header("step 5", "user passwords")),
        chunks[0],
    );

    let total = app.prompt_users.len();
    let position = (app.password_idx + 1).min(total.max(1));
    let user = app.current_prompt_user().unwrap_or("").to_string();
    frame.render_widget(
        Paragraph::new(status_line(
            "•",
            format!("set a password for {user} ({position} of {total})"),
            ACCENT,
            true,
        )),
        chunks[1],
    );

    let active = if app.password_confirming {
        &app.password_confirm_input
    } else {
        &app.password_input
    };
    // Never render the secret; show one bullet per character plus a cursor.
    let masked = format!("{}█", "•".repeat(active.chars().count()));
    let field_label = if app.password_confirming {
        "confirm"
    } else {
        "password"
    };

    let mut body = vec![
        kv_line("user", &user, 10),
        Line::from(""),
        Line::from(vec![
            Span::styled(format!("{field_label:<10}"), Style::default().fg(DIM)),
            Span::styled(masked, Style::default().fg(FG)),
        ]),
        Line::from(""),
    ];

    if let Some(error) = &app.password_error {
        body.push(Line::from(Span::styled(
            error.clone(),
            Style::default().fg(WARN),
        )));
        body.push(Line::from(""));
    }

    let hint = if app.password_confirming {
        "Re-enter the same password to confirm, then press enter."
    } else {
        "Type the password and press enter; you'll confirm it next."
    };
    body.push(Line::from(Span::styled(hint, Style::default().fg(FAINT))));

    draw_focal_panel(frame, chunks[2], "password", ACCENT, body);
}

fn draw_manifest_preview(frame: &mut Frame, area: Rect, app: &App) {
    let viewport = area.height.saturating_sub(4).max(1) as usize;
    let inner_width = area.width.saturating_sub(4);
    // Give the wrapped text the full inner paragraph width; scrollbar uses its
    // own dedicated column just inside the right border.
    let body_width = inner_width;

    let (mut body, total_lines, scroll_pos) = if let Some(error) = &app.manifest_read_error {
        (
            vec![Line::from(Span::styled(
                error.clone(),
                Style::default().fg(WARN),
            ))],
            1,
            0,
        )
    } else if let Some(text) = &app.manifest_text {
        let logical_lines: Vec<&str> = text.lines().collect();

        // Wrap each logical line into one or more visual lines.
        // First piece of each logical line keeps full TOML highlighting;
        // continuation pieces (for long lines) render in FG.
        let mut all_visual: Vec<Line<'static>> = Vec::new();
        for &raw in &logical_lines {
            let pieces = wrap_text(raw, body_width as usize);
            for (i, piece) in pieces.into_iter().enumerate() {
                let styled_line = if i == 0 {
                    highlight_toml_line(&piece)
                } else {
                    Line::from(Span::styled(piece, Style::default().fg(FG)))
                };
                all_visual.push(styled_line);
            }
        }

        let total = all_visual.len();
        let max_scroll = total.saturating_sub(viewport);
        let scroll = app.manifest_scroll.min(max_scroll);

        let body: Vec<Line<'static>> = all_visual.into_iter().skip(scroll).take(viewport).collect();

        (body, total, scroll)
    } else {
        (
            vec![Line::from(Span::styled(
                "manifest.toml was not captured after compile",
                Style::default().fg(DIM),
            ))],
            1,
            0,
        )
    };

    if body.is_empty() {
        body.push(Line::from(Span::styled(
            "manifest.toml is empty",
            Style::default().fg(DIM),
        )));
    }

    draw_focal_panel(frame, area, "generated manifest.toml", ACCENT, body);

    // Simple visible scroll indicator (only when content overflows).
    draw_simple_scrollbar(frame, area, scroll_pos, total_lines, viewport);
}
