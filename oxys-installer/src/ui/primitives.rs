use ratatui::{
    layout::Rect,
    prelude::*,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph},
};

use super::theme::{ACCENT, BG, DIM, FAINT, FG, SUCCESS, SURFACE, WARN};

pub(super) fn section_header(step: &str, title: &str) -> Line<'static> {
    Line::from(vec![
        Span::styled(
            step.to_uppercase(),
            Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
        ),
        Span::styled(" · ", Style::default().fg(DIM).add_modifier(Modifier::BOLD)),
        Span::styled(
            title.to_uppercase(),
            Style::default().fg(FG).add_modifier(Modifier::BOLD),
        ),
    ])
}

pub(super) fn status_line(glyph: &str, text: String, color: Color, bold: bool) -> Line<'static> {
    let modifier = if bold {
        Modifier::BOLD
    } else {
        Modifier::empty()
    };
    Line::from(vec![
        Span::styled(
            format!("{glyph} "),
            Style::default().fg(color).add_modifier(modifier),
        ),
        Span::styled(text, Style::default().fg(color).add_modifier(modifier)),
    ])
}

pub(super) fn draw_action_row(
    frame: &mut Frame,
    area: Rect,
    label: &str,
    selected: bool,
    enabled: bool,
) {
    let line = if selected {
        Line::from(vec![
            Span::styled(
                "› ",
                Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                label.to_string(),
                Style::default().fg(FG).add_modifier(Modifier::BOLD),
            ),
        ])
    } else {
        let color = if enabled { DIM } else { FAINT };
        Line::from(vec![
            Span::raw("  "),
            Span::styled(label.to_string(), Style::default().fg(color)),
        ])
    };

    frame.render_widget(Paragraph::new(line).style(Style::default().bg(BG)), area);
}

pub(super) fn kv_line(key: &str, value: &str, key_width: usize) -> Line<'static> {
    Line::from(vec![
        Span::styled("◆ ", Style::default().fg(FAINT)),
        Span::styled(
            format!("{:<width$}", key, width = key_width),
            Style::default().fg(DIM),
        ),
        Span::styled(value.to_string(), Style::default().fg(FG)),
    ])
}

pub(super) fn rule_line(width: u16) -> Line<'static> {
    Line::from(Span::styled(
        "─".repeat(width as usize),
        Style::default().fg(FAINT),
    ))
}

pub(super) fn draw_focal_panel(
    frame: &mut Frame,
    area: Rect,
    title: &str,
    title_color: Color,
    body: Vec<Line<'static>>,
) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(ACCENT))
        .style(Style::default().bg(SURFACE).fg(FG));
    frame.render_widget(block, area);

    if area.width < 4 || area.height < 3 {
        return;
    }

    let inner_width = area.width.saturating_sub(4);
    let label = format!("◆ {title} ");
    let divider_width = inner_width.saturating_sub(label.chars().count() as u16);
    let header = Line::from(vec![
        Span::styled(
            label,
            Style::default()
                .fg(title_color)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            "─".repeat(divider_width as usize),
            Style::default().fg(FAINT),
        ),
    ]);

    frame.render_widget(
        Paragraph::new(header).style(Style::default().bg(SURFACE)),
        Rect {
            x: area.x + 2,
            y: area.y + 1,
            width: inner_width,
            height: 1,
        },
    );

    if area.height < 5 {
        return;
    }

    frame.render_widget(
        Paragraph::new(body).style(Style::default().bg(SURFACE)),
        Rect {
            x: area.x + 2,
            y: area.y + 3,
            width: inner_width,
            height: area.height.saturating_sub(4),
        },
    );
}

/// Compute the starting row (within the track) for the scrollbar thumb.
/// Pure function so it can be easily tested.
fn compute_thumb_position(scroll: usize, max_scroll: usize, track: usize, _thumb: usize) -> usize {
    if max_scroll == 0 || track == 0 {
        return 0;
    }
    ((track as f64 * scroll as f64) / (max_scroll as f64)) as usize
}

/// Very simple scrollbar indicator (track + thumb) drawn inside the right
/// edge of a focal panel. Only draws when content actually overflows.
pub(super) fn draw_simple_scrollbar(
    frame: &mut Frame,
    area: Rect,
    scroll: usize,
    total: usize,
    viewport: usize,
) {
    if area.height < 5 || area.width < 5 || total <= viewport {
        return;
    }

    let track_height = area.height.saturating_sub(4) as usize;
    if track_height == 0 {
        return;
    }

    // Position just inside the right border of the panel.
    let track_x = area.x + area.width.saturating_sub(2);
    let track_y = area.y + 3;

    // Proportional thumb size and position.
    let thumb_height = ((track_height as f64 * viewport as f64) / (total as f64))
        .max(1.0)
        .min(track_height as f64) as usize;

    let max_scroll = total.saturating_sub(viewport);
    let thumb_top = compute_thumb_position(scroll, max_scroll, track_height, thumb_height);
    let thumb_top = thumb_top.min(track_height.saturating_sub(thumb_height));

    let mut lines: Vec<Line<'static>> = Vec::with_capacity(track_height);
    for i in 0..track_height {
        let is_thumb = i >= thumb_top && i < (thumb_top + thumb_height);
        let ch = if is_thumb { "█" } else { "│" };
        let color = if is_thumb { DIM } else { FAINT };
        lines.push(Line::from(Span::styled(
            ch.to_string(),
            Style::default().fg(color),
        )));
    }

    frame.render_widget(
        Paragraph::new(lines).style(Style::default().bg(SURFACE)),
        Rect {
            x: track_x,
            y: track_y,
            width: 1,
            height: track_height as u16,
        },
    );
}

/// Color a `[tag  ] message` log line by its tag so run/ok/error/out read at
/// a glance without adding extra boxes.
pub(super) fn style_log_line(line: &str) -> Line<'static> {
    let (tag_color, rest_color) = if line.starts_with("[error]") {
        (WARN, FG)
    } else if line.starts_with("[ok") {
        (SUCCESS, FG)
    } else if line.starts_with("[run") || line.starts_with("[plan") {
        (ACCENT, FG)
    } else {
        (DIM, DIM)
    };

    if let Some(close) = line.find(']') {
        let (tag, rest) = line.split_at(close + 1);
        Line::from(vec![
            Span::styled(tag.to_string(), Style::default().fg(tag_color)),
            Span::styled(rest.to_string(), Style::default().fg(rest_color)),
        ])
    } else {
        Line::from(Span::styled(
            line.to_string(),
            Style::default().fg(rest_color),
        ))
    }
}

/// Very subtle TOML syntax highlighting for the manifest preview.
/// - comments → FAINT
/// - [section] / [[array]] → DIM
/// - keys → DIM
/// - "=" → FAINT
/// - values → FG
pub(super) fn highlight_toml_line(line: &str) -> Line<'static> {
    if line.trim().is_empty() {
        return Line::from(Span::raw(line.to_string()));
    }

    let trimmed_start = line.trim_start();
    let ws_len = line.len() - trimmed_start.len();
    let ws = &line[..ws_len];
    let rest = trimmed_start;

    if rest.starts_with('#') {
        return Line::from(vec![
            Span::raw(ws.to_string()),
            Span::styled(rest.to_string(), Style::default().fg(FAINT)),
        ]);
    }

    if rest.starts_with('[') {
        return Line::from(vec![
            Span::raw(ws.to_string()),
            Span::styled(rest.to_string(), Style::default().fg(DIM)),
        ]);
    }

    if let Some(eq_idx) = rest.find('=') {
        let before = &rest[..eq_idx];
        let after = &rest[eq_idx + 1..];
        let ws_after_idx = after
            .find(|c: char| !c.is_whitespace())
            .unwrap_or(after.len());
        let ws_after_eq = &after[..ws_after_idx];
        let val = &after[ws_after_idx..];

        return Line::from(vec![
            Span::raw(ws.to_string()),
            Span::styled(before.to_string(), Style::default().fg(DIM)),
            Span::styled("=", Style::default().fg(FAINT)),
            Span::raw(ws_after_eq.to_string()),
            Span::styled(val.to_string(), Style::default().fg(FG)),
        ]);
    }

    // fallback (plain content)
    Line::from(vec![
        Span::raw(ws.to_string()),
        Span::styled(rest.to_string(), Style::default().fg(FG)),
    ])
}

/// Simple word-aware wrap for a single logical line.
/// Prefers to break at whitespace when possible; falls back to hard breaks
/// for long tokens (e.g. long arrays, hashes, etc).
/// Each returned piece has at most `width` characters.
pub(super) fn wrap_text(text: &str, width: usize) -> Vec<String> {
    if width == 0 {
        return vec![text.to_string()];
    }
    let total = text.chars().count();
    if total <= width {
        return vec![text.to_string()];
    }

    let mut result = Vec::new();
    let mut current = String::new();

    for ch in text.chars() {
        current.push(ch);

        if current.chars().count() > width {
            // Find a preferred break point (last whitespace within the width)
            let chars: Vec<char> = current.chars().collect();
            let mut break_at = width;

            for i in (0..width).rev() {
                if chars[i].is_whitespace() {
                    break_at = i + 1; // keep the whitespace with the line
                    break;
                }
            }

            let keep: String = chars[..break_at].iter().collect();
            let carry: String = chars[break_at..].iter().collect();

            result.push(keep);
            current = carry;
        }
    }

    if !current.is_empty() {
        result.push(current);
    }

    if result.is_empty() {
        result.push(String::new());
    }
    result
}
