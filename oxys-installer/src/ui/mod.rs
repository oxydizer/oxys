use ratatui::{
    layout::{Constraint, Direction, Layout},
    prelude::*,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph},
};

use crate::app::{App, Screen};

mod layout;
mod primitives;
mod screens;
pub(crate) mod theme;

use layout::{centered_container, draw_divider, draw_footer, draw_header, draw_rail, pad};
use screens::{
    draw_config,
    draw_config_error,
    draw_config_validate,
    draw_confirm,
    draw_disk_select,
    draw_done,
    draw_hardware_detection,
    draw_install,
    draw_package_summary,
    draw_passwords,
    draw_usernames,
    draw_welcome,
    // draw_partition,  // step 4 hidden for now
};
use theme::{ACCENT, BG, DIM, FG, SURFACE};

pub(crate) fn draw_ui(frame: &mut Frame, app: &App) {
    frame.render_widget(
        Block::default().style(Style::default().bg(BG)),
        frame.area(),
    );
    let container = centered_container(frame.area(), 108);

    let vertical = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(2),
            Constraint::Length(1),
            Constraint::Min(10),
            Constraint::Length(1),
            Constraint::Length(3),
        ])
        .split(container);

    draw_header(frame, vertical[0], app);
    draw_rail(frame, vertical[1], app.current);

    let body = pad(vertical[2], 1, 1);
    match app.current {
        Screen::Welcome => draw_welcome(frame, body, app),
        Screen::HardwareDetection => draw_hardware_detection(frame, body, app),
        Screen::DiskSelect => draw_disk_select(frame, body, app),
        Screen::Partition => {} // step 4 (partition) hidden for now
        Screen::ConfigSelect => draw_config(frame, body, app),
        Screen::ConfigValidate => draw_config_validate(frame, body, app),
        Screen::ConfigError => draw_config_error(frame, body, app),
        Screen::PackageSummary => draw_package_summary(frame, body, app),
        Screen::Confirm => draw_confirm(frame, body, app),
        Screen::Usernames => draw_usernames(frame, body, app),
        Screen::Passwords => draw_passwords(frame, body, app),
        Screen::Installing => draw_install(frame, body, &app.install_lines, app.install_progress),
        Screen::Done => draw_done(frame, body),
    }

    draw_divider(frame, vertical[3]);
    draw_footer(frame, vertical[4], app);

    if app.confirm_quit {
        draw_quit_dialog(frame, container);
    }
}

fn draw_quit_dialog(frame: &mut Frame, area: Rect) {
    let width = 46.min(area.width.saturating_sub(4));
    let height = 7.min(area.height.saturating_sub(2));
    if width == 0 || height == 0 {
        return;
    }

    let dialog = Rect {
        x: area.x + area.width.saturating_sub(width) / 2,
        y: area.y + area.height.saturating_sub(height) / 2,
        width,
        height,
    };

    frame.render_widget(Clear, dialog);
    frame.render_widget(
        Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(ACCENT))
            .style(Style::default().bg(SURFACE).fg(FG)),
        dialog,
    );

    let lines = vec![
        Line::from(Span::styled(
            "quit installer?",
            Style::default().fg(FG).add_modifier(Modifier::BOLD),
        )),
        Line::from(Span::styled(
            "Unsaved selections and running progress will be lost.",
            Style::default().fg(DIM),
        )),
        Line::from(""),
        Line::from(vec![
            Span::styled(
                "enter",
                Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
            ),
            Span::styled(" quit    ", Style::default().fg(DIM)),
            Span::styled("esc", Style::default().fg(FG).add_modifier(Modifier::BOLD)),
            Span::styled(" cancel", Style::default().fg(DIM)),
        ]),
    ];

    frame.render_widget(
        Paragraph::new(lines)
            .style(Style::default().bg(SURFACE))
            .alignment(Alignment::Center),
        Rect {
            x: dialog.x + 2,
            y: dialog.y + 1,
            width: dialog.width.saturating_sub(4),
            height: dialog.height.saturating_sub(2),
        },
    );
}
