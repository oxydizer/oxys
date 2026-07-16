use super::*;

pub(in crate::ui) fn draw_package_summary(frame: &mut Frame, area: Rect, app: &App) {
    let chunks = screen_chunks(area, 2, 8);
    frame.render_widget(
        Paragraph::new(section_header("step 5", "packages")),
        chunks[0],
    );

    let Some(summary) = &app.package_summary else {
        frame.render_widget(
            Paragraph::new(status_line(
                "!",
                "no package data available for this config".to_string(),
                WARN,
                true,
            )),
            chunks[1],
        );
        draw_focal_panel(
            frame,
            chunks[2],
            "packages",
            ACCENT,
            vec![Line::from(Span::styled(
                "the compiled manifest could not be read".to_string(),
                Style::default().fg(DIM),
            ))],
        );
        return;
    };

    let source_count = summary.source.len();
    let (glyph, headline, color) = if source_count == 0 {
        (
            "•",
            format!(
                "{} package(s), all prebuilt binaries — fast copy from the ISO",
                summary.total()
            ),
            SUCCESS,
        )
    } else {
        (
            "!",
            format!(
                "{} of {} package(s) build from source — expect longer install and network use",
                source_count,
                summary.total()
            ),
            WARN,
        )
    };
    frame.render_widget(
        Paragraph::new(status_line(glyph, headline, color, true)),
        chunks[1],
    );

    // Build a single flat, scrollable body across both groups. The binary group
    // is usually large (the whole ISO base), so scrolling matters.
    let mut lines: Vec<Line<'static>> = Vec::new();

    for notice in &app.compile_notices {
        lines.push(Line::from(Span::styled(
            notice.clone(),
            Style::default().fg(DIM),
        )));
    }
    if !app.compile_notices.is_empty() {
        lines.push(Line::from(""));
    }

    if !app.defaults_report.is_empty() {
        lines.push(Line::from(Span::styled(
            "◆ defaults in effect".to_string(),
            Style::default().fg(FAINT).add_modifier(Modifier::BOLD),
        )));
        for line in &app.defaults_report {
            lines.push(Line::from(Span::styled(
                format!("  {line}"),
                Style::default().fg(DIM),
            )));
        }
        lines.push(Line::from(""));
    }

    lines.push(Line::from(vec![
        Span::styled(
            "◆ from ISO ".to_string(),
            Style::default().fg(SUCCESS).add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            "binary · no download  ".to_string(),
            Style::default().fg(FAINT),
        ),
        Span::styled(
            format!("({})", summary.binary.len()),
            Style::default().fg(DIM),
        ),
    ]));
    if summary.binary.is_empty() {
        lines.push(Line::from(Span::styled(
            "  (none)".to_string(),
            Style::default().fg(FAINT),
        )));
    }
    for entry in &summary.binary {
        lines.push(Line::from(Span::styled(
            format!("  {}", entry.atom),
            Style::default().fg(FG),
        )));
    }

    lines.push(Line::from(""));
    lines.push(Line::from(vec![
        Span::styled(
            "◆ build from source ".to_string(),
            Style::default().fg(WARN).add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            "download + compile  ".to_string(),
            Style::default().fg(FAINT),
        ),
        Span::styled(
            format!("({})", summary.source.len()),
            Style::default().fg(DIM),
        ),
    ]));
    if summary.source.is_empty() {
        lines.push(Line::from(Span::styled(
            "  (none)".to_string(),
            Style::default().fg(FAINT),
        )));
    }
    for entry in &summary.source {
        let mut spans = vec![Span::styled(
            format!("  {}", entry.atom),
            Style::default().fg(FG),
        )];
        if !entry.use_flags.is_empty() {
            spans.push(Span::styled(
                format!("   {}", entry.use_flags.join(" ")),
                Style::default().fg(ACCENT),
            ));
        }
        lines.push(Line::from(spans));
    }

    // Window the body to the panel height, mirroring draw_config_error.
    let visible = chunks[2].height.saturating_sub(4).max(1) as usize;
    let max_scroll = lines.len().saturating_sub(visible);
    let scroll = app.package_scroll.min(max_scroll);
    let mut body: Vec<Line<'static>> = lines.into_iter().skip(scroll).collect();
    if scroll < max_scroll {
        body.push(Line::from(Span::styled(
            "  ↓ more (PgDn)".to_string(),
            Style::default().fg(FAINT),
        )));
    }

    draw_focal_panel(frame, chunks[2], "package sources", ACCENT, body);
}
