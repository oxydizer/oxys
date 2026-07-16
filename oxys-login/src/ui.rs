use super::*;

pub(super) const SPINNER_FRAMES: &[&str] = &["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

pub(super) fn render(frame: &mut Frame<'_>, app: &App) {
    let area = frame.area();
    frame.render_widget(Block::default().style(Style::default().bg(theme::BG)), area);

    render_logo(frame, area, app.logo);
    render_system_info(frame, area, app);
    render_clock(frame, area, &app.clock);
    render_boot_time(frame, area, &app.boot_time);
    render_login(frame, area, app);
}

fn render_logo(frame: &mut Frame<'_>, area: Rect, logo: &str) {
    let logo_width = logo.lines().map(str::len).max().unwrap_or(0) as u16 + 2;
    let logo_height = logo.lines().count() as u16 + 2;
    let logo_area = Rect::new(
        2,
        1,
        logo_width.min(area.width.saturating_sub(2)),
        logo_height.min(area.height.saturating_sub(1)),
    );

    let logo_widget = Paragraph::new(logo)
        .style(Style::default().fg(theme::ACCENT))
        .alignment(Alignment::Left);

    frame.render_widget(logo_widget, logo_area);
}

fn render_system_info(frame: &mut Frame<'_>, area: Rect, app: &App) {
    let text = format!("{} | up {}", app.hostname, app.uptime);
    let info_width = text.len() as u16 + 4;
    let info_area = Rect::new(
        area.width.saturating_sub(info_width + 2),
        1,
        info_width.min(area.width.saturating_sub(2)),
        3,
    );

    let info = Paragraph::new(text)
        .style(Style::default().fg(theme::SUCCESS))
        .alignment(Alignment::Right);

    frame.render_widget(info, info_area);
}

fn render_clock(frame: &mut Frame<'_>, area: Rect, clock: &str) {
    let width = clock.len() as u16 + 4;
    let clock_area = Rect::new(
        area.width.saturating_sub(width + 2),
        area.height.saturating_sub(3),
        width.min(area.width.saturating_sub(2)),
        3,
    );

    let clock_widget = Paragraph::new(clock)
        .style(Style::default().fg(theme::ACCENT))
        .alignment(Alignment::Right);

    frame.render_widget(clock_widget, clock_area);
}

fn render_boot_time(frame: &mut Frame<'_>, area: Rect, boot_time: &str) {
    let text = format!("booted in {}", boot_time);
    let width = text.len() as u16 + 2;
    let boot_area = Rect::new(2, area.height.saturating_sub(3), width, 1);

    let widget = Paragraph::new(text)
        .style(Style::default().fg(theme::DIM).add_modifier(Modifier::DIM))
        .alignment(Alignment::Left);

    frame.render_widget(widget, boot_area);
}

fn render_login(frame: &mut Frame<'_>, area: Rect, app: &App) {
    let popup = centered_rect(46, 34, area);
    frame.render_widget(Clear, popup);

    let border_color = if app.auth_in_progress || app.wrong_password {
        theme::ACCENT
    } else {
        theme::FAINT
    };

    let mut title = vec![Span::styled(
        " OXYS LOGIN ",
        Style::default().fg(theme::FG).add_modifier(Modifier::BOLD),
    )];
    if app.failed_attempts > 0 {
        title.push(Span::styled(" •", Style::default().fg(theme::ACCENT)));
        title.push(Span::styled(
            format!(" {} failed", app.failed_attempts),
            Style::default().fg(theme::DIM),
        ));
    }
    let block = Block::default()
        .title(Line::from(title))
        .title_alignment(Alignment::Center)
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(
            Style::default()
                .fg(border_color)
                .add_modifier(Modifier::BOLD),
        )
        .style(Style::default().bg(theme::SURFACE).fg(theme::FG));

    frame.render_widget(block, popup);

    let inner = Rect::new(
        popup.x + 2,
        popup.y + 2,
        popup.width.saturating_sub(4),
        popup.height.saturating_sub(4),
    );

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(2),
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(2),
            Constraint::Length(2),
        ])
        .split(inner);

    let header = Paragraph::new("Authenticate with PAM and start the niri session")
        .style(Style::default().fg(theme::DIM))
        .alignment(Alignment::Center);
    frame.render_widget(header, chunks[0]);

    let username_marker = if app.focus == FocusField::Username {
        Span::styled(
            "› ",
            Style::default()
                .fg(theme::ACCENT)
                .add_modifier(Modifier::BOLD),
        )
    } else {
        Span::styled("  ", Style::default().fg(theme::FAINT))
    };
    let username_line = Line::from(vec![
        username_marker,
        Span::styled(
            "Username ",
            Style::default().fg(theme::DIM).add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            app.username.as_str(),
            app.selected_style(FocusField::Username),
        ),
    ]);
    let username = Paragraph::new(username_line).style(Style::default().bg(theme::SURFACE));
    frame.render_widget(username, chunks[1]);

    let password_mask = "*".repeat(app.password.chars().count());
    let password_marker = if app.focus == FocusField::Password {
        Span::styled(
            "› ",
            Style::default()
                .fg(theme::ACCENT)
                .add_modifier(Modifier::BOLD),
        )
    } else {
        Span::styled("  ", Style::default().fg(theme::FAINT))
    };
    let password_line = Line::from(vec![
        password_marker,
        Span::styled(
            "Password ",
            Style::default().fg(theme::DIM).add_modifier(Modifier::BOLD),
        ),
        Span::styled(password_mask, app.selected_style(FocusField::Password)),
    ]);
    let password = Paragraph::new(password_line).style(Style::default().bg(theme::SURFACE));
    frame.render_widget(password, chunks[3]);

    let spacer = Paragraph::new("");
    frame.render_widget(spacer, chunks[4]);

    let spinner_text = if app.auth_in_progress {
        SPINNER_FRAMES[app.spinner_index]
    } else {
        ""
    };
    let spinner = Paragraph::new(spinner_text)
        .style(Style::default().fg(theme::ACCENT))
        .alignment(Alignment::Center);
    frame.render_widget(spinner, chunks[5]);

    let help = if app.fallback_tty_login {
        "Tab switches fields. Enter submits. Ctrl+Q opens TTY login. Ctrl+C is ignored."
    } else {
        "Tab switches fields. Enter submits. Ctrl+C is ignored."
    };
    let status_text = app.status.as_deref().unwrap_or(help);
    let status_color = if app.wrong_password {
        theme::WARN
    } else if app.auth_in_progress {
        theme::ACCENT
    } else {
        theme::DIM
    };
    let status = Paragraph::new(status_text)
        .style(Style::default().fg(status_color))
        .alignment(Alignment::Center);
    frame.render_widget(status, chunks[6]);

    // Prefix is focus marker ("› "/ "  ") + label; use display columns, not UTF-8 bytes
    // (`›` is 3 bytes but only 1 terminal cell).
    const MARKER_COLS: u16 = 2;
    match app.focus {
        FocusField::Username => {
            let cursor_x = chunks[1]
                .x
                .saturating_add(MARKER_COLS)
                .saturating_add("Username ".len() as u16)
                .saturating_add(app.username.chars().count() as u16);
            frame.set_cursor_position((cursor_x, chunks[1].y));
        }
        FocusField::Password => {
            let cursor_x = chunks[3]
                .x
                .saturating_add(MARKER_COLS)
                .saturating_add("Password ".len() as u16)
                .saturating_add(app.password.chars().count() as u16);
            frame.set_cursor_position((cursor_x, chunks[3].y));
        }
    }
}

fn centered_rect(percent_x: u16, percent_y: u16, area: Rect) -> Rect {
    let vertical = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage((100 - percent_y) / 2),
            Constraint::Percentage(percent_y),
            Constraint::Percentage((100 - percent_y) / 2),
        ])
        .split(area);

    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - percent_x) / 2),
            Constraint::Percentage(percent_x),
            Constraint::Percentage((100 - percent_x) / 2),
        ])
        .split(vertical[1])[1]
}
