use super::*;

pub(in crate::ui) fn draw_welcome(frame: &mut Frame, area: Rect, app: &App) {
    let art_height = (OXYS_SPLASH.len() as u16).min(area.height.saturating_sub(13));
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage(18),
            Constraint::Length(art_height),
            Constraint::Length(4),
            Constraint::Length(8),
            Constraint::Min(1),
        ])
        .split(area);

    let visible_lines = app.splash_lines_visible(art_height);
    let mut art = OXYS_SPLASH
        .iter()
        .take(visible_lines)
        .map(|(left, os)| {
            Line::from(vec![
                Span::styled(*left, Style::default().fg(DIM)),
                Span::styled(*os, Style::default().fg(ACCENT)),
            ])
        })
        .collect::<Vec<_>>();
    while art.len() < art_height as usize {
        art.push(Line::from(""));
    }
    frame.render_widget(Paragraph::new(art).alignment(Alignment::Center), rows[1]);

    frame.render_widget(
        Paragraph::new(vec![
            Line::from(""),
            Line::from(""),
            Line::from(Span::styled("www.oxysos.org", Style::default().fg(DIM))),
        ])
        .alignment(Alignment::Center),
        rows[2],
    );

    // Toggle the caret in software: Linux VT/fbcon (live ISO on real hardware)
    // ignores Modifier::SLOW_BLINK, so ANSI blink never fires there.
    let caret = if app.blink_on() { "› " } else { "  " };
    let body = vec![
        Line::from(""),
        Line::from(""),
        Line::from(""),
        Line::from(""),
        Line::from(""),
        Line::from(""),
        Line::from(vec![
            Span::styled(
                caret,
                Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                "Press ",
                Style::default().fg(FG).add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                "enter",
                Style::default().fg(FG).add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                " to begin",
                Style::default().fg(FG).add_modifier(Modifier::BOLD),
            ),
        ]),
    ];
    frame.render_widget(Paragraph::new(body).alignment(Alignment::Center), rows[3]);
}

const OXYS_SPLASH: &[(&str, &str)] = &[
    (
        "  ******    **      **  **      **    ********          ",
        "******      ********  ",
    ),
    (
        "  ******    **      **  **      **    ********          ",
        "******      ********  ",
    ),
    (
        "**      **    **  **      **  **    **                ",
        "**      **  **          ",
    ),
    (
        "**      **    **  **      **  **    **                ",
        "**      **  **          ",
    ),
    (
        "**      **      **          **        ******          ",
        "**      **    ******    ",
    ),
    (
        "**      **      **          **        ******          ",
        "**      **    ******    ",
    ),
    (
        "**      **    **  **        **              **        ",
        "**      **          **  ",
    ),
    (
        "**      **    **  **        **              **        ",
        "**      **          **  ",
    ),
    (
        "  ******    **      **      **      ********            ",
        "******    ********    ",
    ),
    (
        "  ******    **      **      **      ********            ",
        "******    ********    ",
    ),
];
