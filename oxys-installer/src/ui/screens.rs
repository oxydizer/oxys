use ratatui::{
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    prelude::*,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::Paragraph,
};

use crate::{
    app::App,
    provisioning::{self, TARGET_MOUNT},
};

use super::{
    primitives::{
        draw_action_row, draw_focal_panel, draw_simple_scrollbar, highlight_toml_line, kv_line,
        rule_line, section_header, status_line, style_log_line, wrap_text,
    },
    theme::{ACCENT, DIM, FAINT, FG, FILL, SPINNER, SUCCESS, WARN},
};

mod config;
mod credentials;
mod hardware;
mod progress;
mod storage;
mod summary;
mod welcome;

pub(super) use config::{draw_config, draw_config_error, draw_config_validate, draw_custom_source};
pub(super) use credentials::{draw_confirm, draw_passwords, draw_timezone, draw_usernames};
pub(super) use hardware::draw_hardware_detection;
pub(super) use progress::{draw_done, draw_install};
pub(super) use storage::draw_disk_select;
pub(super) use summary::draw_package_summary;
pub(super) use welcome::draw_welcome;

fn screen_chunks(area: Rect, action_height: u16, min_panel: u16) -> std::rc::Rc<[Rect]> {
    Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(2),
            Constraint::Length(action_height),
            Constraint::Min(min_panel),
        ])
        .split(area)
}

fn hardware_rows(app: &App, rule_width: u16) -> Vec<Line<'static>> {
    if app.hardware_rows.is_empty() {
        return vec![
            Line::from(""),
            Line::from(vec![
                Span::styled("component     ", Style::default().fg(DIM)),
                Span::styled("waiting for detection...", Style::default().fg(FAINT)),
            ]),
        ];
    }

    let mut rows = Vec::new();
    for (i, (key, value)) in app.hardware_rows.iter().enumerate() {
        if i > 0 {
            rows.push(Line::from(""));
            rows.push(rule_line(rule_width));
            rows.push(Line::from(""));
        }
        if key == "Disks" {
            if value == "no installable disks detected" || value.trim().is_empty() {
                rows.push(plain_kv_line(key, value, 18));
            } else {
                let disk_items: Vec<&str> = value.split(" · ").collect();
                rows.push(Line::from(vec![
                    Span::styled(format!("{:<18}", key), Style::default().fg(DIM)),
                    Span::styled(disk_items[0].to_string(), Style::default().fg(FG)),
                ]));
                let indent = " ".repeat(18);
                for &item in &disk_items[1..] {
                    rows.push(Line::from(vec![
                        Span::raw(indent.clone()),
                        Span::styled(item.to_string(), Style::default().fg(FG)),
                    ]));
                }
            }
        } else {
            rows.push(plain_kv_line(key, value, 18));
        }
    }
    rows
}

fn plain_kv_line(key: &str, value: &str, key_width: usize) -> Line<'static> {
    Line::from(vec![
        Span::styled(
            format!("{:<width$}", key, width = key_width),
            Style::default().fg(DIM),
        ),
        Span::styled(value.to_string(), Style::default().fg(FG)),
    ])
}

fn log_body(lines: &[String], empty: &str) -> Vec<Line<'static>> {
    if lines.is_empty() {
        vec![Line::from(Span::styled(
            empty.to_string(),
            Style::default().fg(DIM),
        ))]
    } else {
        lines.iter().map(|line| style_log_line(line)).collect()
    }
}

fn progress_line(percent: u16, width: u16) -> Line<'static> {
    let width = width.max(10) as usize;
    let filled_exact = (percent as f32 / 100.0) * width as f32;
    let filled_full = filled_exact.floor() as usize;
    let frac = filled_exact - filled_full as f32;
    let partial_idx = ((frac * (FILL.len() as f32 - 1.0)).round() as usize).min(FILL.len() - 1);

    let mut bar = String::new();
    for i in 0..width {
        if i < filled_full {
            bar.push('█');
        } else if i == filled_full && partial_idx > 0 {
            bar.push_str(FILL[partial_idx]);
        } else {
            bar.push('·');
        }
    }

    Line::from(vec![
        Span::styled(bar, Style::default().fg(ACCENT)),
        Span::raw("  "),
        Span::styled(
            format!("{:>3}%", percent),
            Style::default().fg(FG).add_modifier(Modifier::BOLD),
        ),
    ])
}
