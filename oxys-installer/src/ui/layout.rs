use ratatui::{
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    prelude::*,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::Paragraph,
};

use crate::app::{App, Screen};

use super::theme::{ACCENT, ACCENT_DIM, ASCII_SPINNER, BG, DIM, FAINT, FG, SUCCESS};

pub(super) fn centered_container(area: Rect, fixed_width: u16) -> Rect {
    let width = fixed_width.min(area.width.saturating_sub(2).max(40));
    let x = area.x + area.width.saturating_sub(width) / 2;

    // Add top/bottom padding so the whole TUI isn't jammed against the edges.
    let v_margin = 2;
    let height = area.height.saturating_sub(v_margin * 2);
    let y = area.y + v_margin;

    Rect {
        x,
        y,
        width,
        height: height.max(8),
    }
}

/// Inset a rect by `h` columns on each side and `v` rows top/bottom.
pub(super) fn pad(area: Rect, h: u16, v: u16) -> Rect {
    Rect {
        x: area.x + h,
        y: area.y + v,
        width: area.width.saturating_sub(h * 2),
        height: area.height.saturating_sub(v * 2),
    }
}

pub(super) fn draw_header(frame: &mut Frame, area: Rect, app: &App) {
    let chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Length(38), Constraint::Min(10)])
        .split(area);

    let (status_label, status_color) = match app.network_online {
        Some(true) => ("online", SUCCESS),
        Some(false) => ("offline", DIM),
        None => (ASCII_SPINNER[app.network_spinner_idx], DIM),
    };

    let title = Line::from(vec![
        Span::styled("OXYS", Style::default().fg(FG).add_modifier(Modifier::BOLD)),
        Span::raw(" "),
        Span::styled(
            "OS",
            Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
        ),
        Span::styled(" v0.1 installer", Style::default().fg(DIM)),
        Span::styled(" · ", Style::default().fg(FAINT)),
        Span::styled(status_label, Style::default().fg(status_color)),
    ]);

    frame.render_widget(
        Paragraph::new(title).style(Style::default().bg(BG)),
        chunks[0],
    );

    frame.render_widget(
        Paragraph::new(app.hardware_short.as_str())
            .style(Style::default().bg(BG).fg(DIM))
            .alignment(Alignment::Right),
        chunks[1],
    );

    if area.height > 1 {
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                "─".repeat(area.width as usize),
                Style::default().fg(FAINT),
            )))
            .style(Style::default().bg(BG)),
            Rect {
                x: area.x,
                y: area.y + 1,
                width: area.width,
                height: 1,
            },
        );
    }
}

/// The signature element: a slim horizontal rail of step gutters. Filled
/// segments mark completed steps, a bright pulsing segment marks the active
/// step, and faint dots mark what's ahead.
pub(super) fn draw_rail(frame: &mut Frame, area: Rect, current: Screen) {
    let labels = App::step_labels();
    let n = labels.len();
    let idx = current.index();
    let circled = ["①", "②", "③", "④", "⑤", "⑥", "⑦"];

    let mut spans = Vec::new();
    for (i, label) in labels.iter().enumerate() {
        let (circle_color, label_color, label_modifier, line_color) = if i < idx {
            (ACCENT_DIM, ACCENT_DIM, Modifier::empty(), ACCENT_DIM)
        } else if i == idx {
            (ACCENT, ACCENT, Modifier::BOLD, ACCENT)
        } else {
            (FAINT, FAINT, Modifier::empty(), FAINT)
        };
        spans.push(Span::styled(circled[i], Style::default().fg(circle_color)));
        spans.push(Span::raw(" "));
        spans.push(Span::styled(
            *label,
            Style::default()
                .fg(label_color)
                .add_modifier(label_modifier),
        ));
        if i + 1 < n {
            spans.push(Span::raw(" "));
            spans.push(Span::styled("──", Style::default().fg(line_color)));
            spans.push(Span::raw(" "));
        }
    }

    frame.render_widget(
        Paragraph::new(Line::from(spans)).style(Style::default().bg(BG)),
        area,
    );
}

pub(super) fn draw_divider(frame: &mut Frame, area: Rect) {
    let width = area.width as usize;
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            "─".repeat(width),
            Style::default().fg(FAINT),
        )))
        .style(Style::default().bg(BG)),
        area,
    );
}

pub(super) fn draw_footer(frame: &mut Frame, area: Rect, app: &App) {
    let hints: Vec<(&str, &str)> = match app.current {
        Screen::Welcome => vec![("enter", "continue"), ("q", "quit")],
        Screen::HardwareDetection => vec![("↑↓", "select"), ("enter", "activate"), ("esc", "back")],
        Screen::DiskSelect => vec![
            ("↑↓", "move"),
            ("space", "select"),
            ("enter", "plan"),
            ("esc", "back"),
        ],
        Screen::ConfigSelect => vec![
            ("↑↓", "select"),
            ("enter", "compile"),
            ("^G", "edit"),
            ("esc", "back"),
        ],
        Screen::CustomSource if app.custom_fetching => vec![("wait", "fetching")],
        Screen::CustomSource => vec![
            ("type", "path or URL"),
            ("enter", "confirm"),
            ("esc", "back"),
        ],
        Screen::ConfigValidate => vec![("wait", "compiling"), ("esc", "cancel")],
        Screen::ConfigError => vec![
            ("↑↓", "scroll"),
            ("^G", "edit"),
            ("enter", "retry"),
            ("esc", "back"),
        ],
        Screen::PackageSummary => vec![
            ("↑↓", "scroll"),
            ("pg", "scroll"),
            ("enter", "continue"),
            ("esc", "back"),
        ],
        Screen::Confirm if app.confirm_view_manifest => vec![
            ("↑↓", "scroll"),
            ("pg", "scroll"),
            ("m", "install plan"),
            ("esc", "summary"),
        ],
        Screen::Confirm => vec![
            ("enter", "provision disk"),
            ("m", "view manifest"),
            ("esc", "back"),
        ],
        Screen::Usernames => vec![("type", "username"), ("enter", "confirm"), ("esc", "back")],
        Screen::Passwords => vec![("type", "password"), ("enter", "confirm"), ("esc", "back")],
        Screen::Partition | Screen::Installing => vec![("wait", "running")], // Partition hidden for now
        Screen::Done => vec![("q", "exit")],
    };

    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(1),
        ])
        .split(area);

    let mut top = Vec::new();
    let mut mid = Vec::new();
    let mut bot = Vec::new();
    for (i, (key, action)) in hints.iter().enumerate() {
        if i > 0 {
            top.push(Span::raw("    "));
            mid.push(Span::raw("    "));
            bot.push(Span::raw("    "));
        }
        let width = key.chars().count() + 2;
        let label_pad = " ".repeat(action.chars().count() + 1);
        top.push(Span::styled(
            format!("┌{}┐", "─".repeat(width)),
            Style::default().fg(FAINT),
        ));
        top.push(Span::raw(label_pad.clone()));
        mid.push(Span::styled("│ ".to_string(), Style::default().fg(FAINT)));
        mid.push(Span::styled(
            *key,
            Style::default().fg(DIM).add_modifier(Modifier::BOLD),
        ));
        mid.push(Span::styled(
            format!(" │ {action}"),
            Style::default().fg(FAINT),
        ));
        bot.push(Span::styled(
            format!("└{}┘", "─".repeat(width)),
            Style::default().fg(FAINT),
        ));
        bot.push(Span::raw(label_pad));
    }

    frame.render_widget(
        Paragraph::new(Line::from(top)).style(Style::default().bg(BG)),
        rows[0],
    );
    frame.render_widget(
        Paragraph::new(Line::from(mid)).style(Style::default().bg(BG)),
        rows[1],
    );
    frame.render_widget(
        Paragraph::new(Line::from(bot)).style(Style::default().bg(BG)),
        rows[2],
    );
}
