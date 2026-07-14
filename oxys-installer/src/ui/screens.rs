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

pub(super) fn draw_welcome(frame: &mut Frame, area: Rect, app: &App) {
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

    let body = vec![
        Line::from(""),
        Line::from(""),
        Line::from(""),
        Line::from(""),
        Line::from(""),
        Line::from(""),
        Line::from(vec![
            Span::styled(
                "› ",
                Style::default()
                    .fg(ACCENT)
                    .add_modifier(Modifier::BOLD | Modifier::SLOW_BLINK),
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

pub(super) fn draw_hardware_detection(frame: &mut Frame, area: Rect, app: &App) {
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

pub(super) fn draw_disk_select(frame: &mut Frame, area: Rect, app: &App) {
    let chunks = screen_chunks(area, 1, 9);
    frame.render_widget(
        Paragraph::new(section_header("step 3", "target disk")),
        chunks[0],
    );

    // main panel content: disks list (checkboxes + up/down) then a static
    // filesystem note below. The filesystem is always ext4 whole-disk now, so
    // there is nothing to choose here.
    let list_height = chunks[2].height.saturating_sub(5) as usize;
    // reserve space at the end of the panel so the filesystem note is visible
    let mut body = disk_list_lines(app, list_height.saturating_sub(6), app.target_cursor);

    let fs_width = chunks[2].width.saturating_sub(6);
    body.push(Line::from(""));
    body.push(rule_line(fs_width));
    body.push(Line::from(""));

    body.push(Line::from(Span::styled(
        "filesystem",
        Style::default().fg(DIM).add_modifier(Modifier::BOLD),
    )));
    body.push(Line::from(""));
    body.push(Line::from(vec![
        Span::raw("  "),
        Span::styled("ext4", Style::default().fg(ACCENT)),
        Span::raw(" · whole disk"),
    ]));
    body.push(Line::from(Span::styled(
        "      EFI system partition + ext4 root filling the drive",
        Style::default().fg(DIM),
    )));

    draw_focal_panel(frame, chunks[2], "disks & filesystem", ACCENT, body);
}

fn disk_list_lines(app: &App, max_lines: usize, cursor: usize) -> Vec<Line<'static>> {
    if app.disks.is_empty() {
        return vec![
            Line::from(""),
            Line::from(Span::styled(
                "no installable disks detected",
                Style::default().fg(FAINT),
            )),
        ];
    }

    let n = app.disks.len();
    let focused = if cursor < n {
        cursor
    } else {
        n.saturating_sub(1)
    }; // if cursor on fs, bias to bottom
    let chosen = app.disk_idx.min(n.saturating_sub(1));

    // reserve space for top padding + optional scroll hint
    let usable = max_lines.saturating_sub(2).max(1);
    let visible = if n <= usable { n } else { usable };

    // window follows the focused item (or bottom if fs is focused)
    let bias = 1usize;
    let start = if n <= visible {
        0
    } else {
        focused.saturating_sub(bias).min(n - visible)
    };
    let end = (start + visible).min(n);

    let mut lines = vec![Line::from("")];

    for i in start..end {
        let disk = &app.disks[i];
        let is_focused = i == cursor; // only if cursor is on this disk
        let is_chosen = i == chosen;
        let check = if is_chosen { "[x]" } else { "[ ]" };
        let label = provisioning::format_disk(disk);

        if is_focused {
            lines.push(Line::from(vec![
                Span::styled(
                    "› ",
                    Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    check,
                    Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
                ),
                Span::raw(" "),
                Span::styled(label, Style::default().fg(FG).add_modifier(Modifier::BOLD)),
            ]));
        } else if is_chosen {
            // chosen but not focused: show [x] but dim
            lines.push(Line::from(vec![
                Span::raw("  "),
                Span::styled(check, Style::default().fg(ACCENT)),
                Span::raw(" "),
                Span::styled(label, Style::default().fg(ACCENT)),
            ]));
        } else {
            lines.push(Line::from(vec![
                Span::raw("  "),
                Span::styled(check, Style::default().fg(DIM)),
                Span::raw(" "),
                Span::styled(label, Style::default().fg(DIM)),
            ]));
        }
    }

    // scroll hint if the full list doesn't fit
    if start > 0 || end < n {
        lines.push(Line::from(""));
        let hint = if start > 0 && end < n {
            "↑↓ for more"
        } else if start > 0 {
            "↑ for more"
        } else {
            "↓ for more"
        };
        lines.push(Line::from(Span::styled(
            format!("  {hint}"),
            Style::default().fg(FAINT),
        )));
    }

    lines
}

// pub(super) fn draw_partition(...) { ... }  // Step 4 hidden for now
/*
pub(super) fn draw_partition(frame: &mut Frame, area: Rect, lines: &[String]) {
    let chunks = screen_chunks(area, 2, 6);
    frame.render_widget(
        Paragraph::new(section_header("step 4", "partition")),
        chunks[0],
    );

    let done = lines.iter().any(|line| line.starts_with("[ok   ]"));
    let status = if done {
        status_line("✓", "partition plan complete".to_string(), SUCCESS, true)
    } else {
        status_line("⠋", "building disk plan".to_string(), ACCENT, true)
    };
    frame.render_widget(Paragraph::new(status), chunks[1]);

    let mut body = log_body(lines, "waiting…");
    if done {
        body.push(Line::from(""));
        body.push(status_line(
            "✓",
            "Stage complete, continuing…".to_string(),
            SUCCESS,
            true,
        ));
    }
    draw_focal_panel(frame, chunks[2], "plan", ACCENT, body);
}
*/

pub(super) fn draw_config(frame: &mut Frame, area: Rect, app: &App) {
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

pub(super) fn draw_custom_source(frame: &mut Frame, area: Rect, app: &App) {
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

pub(super) fn draw_config_validate(frame: &mut Frame, area: Rect, app: &App) {
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
        Line::from(Span::styled(
            "Building the config into a checked manifest.toml…",
            Style::default().fg(DIM),
        )),
        Line::from(""),
        Line::from(Span::styled(
            "First run compiles the oxys crate and may take a moment.",
            Style::default().fg(FAINT),
        )),
    ];
    draw_focal_panel(frame, chunks[2], "compile", ACCENT, body);
}

pub(super) fn draw_config_error(frame: &mut Frame, area: Rect, app: &App) {
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

pub(super) fn draw_package_summary(frame: &mut Frame, area: Rect, app: &App) {
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

pub(super) fn draw_confirm(frame: &mut Frame, area: Rect, app: &App) {
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

pub(super) fn draw_usernames(frame: &mut Frame, area: Rect, app: &App) {
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
            Span::styled(
                format!("{}█", app.username_input),
                Style::default().fg(FG),
            ),
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

pub(super) fn draw_passwords(frame: &mut Frame, area: Rect, app: &App) {
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

pub(super) fn draw_install(frame: &mut Frame, area: Rect, lines: &[String], progress: u16) {
    let chunks = screen_chunks(area, 2, 8);
    frame.render_widget(
        Paragraph::new(section_header("step 6", "install")),
        chunks[0],
    );

    let failed = lines.iter().any(|line| line.starts_with("[error]"));
    let status = if failed {
        status_line("✗", "installation blocked".to_string(), WARN, true)
    } else if progress >= 100 {
        status_line("✓", "installation complete".to_string(), SUCCESS, true)
    } else {
        status_line("⠋", "installing system".to_string(), ACCENT, true)
    };
    frame.render_widget(Paragraph::new(status), chunks[1]);

    let mut body = vec![
        progress_line(progress, chunks[2].width.saturating_sub(10)),
        Line::from(""),
    ];
    // The focal panel clips overflow at the bottom and has no scrollback, so
    // render the *tail* of the log -- the newest lines, including the [error]
    // that aborted the run -- rather than the oldest that scroll off unseen.
    // Reserve rows for the panel border, the progress bar + its blank line, and
    // the trailing status footer.
    let inner_h = chunks[2].height.saturating_sub(2) as usize;
    let budget = inner_h.saturating_sub(4).max(1);
    let tail = if lines.len() > budget {
        &lines[lines.len() - budget..]
    } else {
        lines
    };
    body.extend(log_body(tail, "starting…"));
    if failed {
        body.push(Line::from(""));
        body.push(status_line(
            "✗",
            format!(
                "install failed — full log at {} (press q to quit)",
                crate::app::INSTALL_LOG_PATH
            ),
            WARN,
            true,
        ));
    } else if progress >= 100 {
        body.push(Line::from(""));
        body.push(status_line(
            "✓",
            "Finishing installer flow…".to_string(),
            SUCCESS,
            true,
        ));
    }
    draw_focal_panel(frame, chunks[2], "log", ACCENT, body);
}

pub(super) fn draw_done(frame: &mut Frame, area: Rect) {
    let chunks = screen_chunks(area, 2, 6);
    frame.render_widget(Paragraph::new(section_header("step 7", "done")), chunks[0]);
    frame.render_widget(
        Paragraph::new(status_line(
            "✓",
            "Installation complete".to_string(),
            SUCCESS,
            true,
        )),
        chunks[1],
    );

    let body = vec![
        Line::from(""),
        kv_line("state", "complete", 12),
        Line::from(""),
        rule_line(chunks[2].width.saturating_sub(6)),
        Line::from(""),
        kv_line("next", "remove installation media, then reboot", 12),
        Line::from(""),
        kv_line("reboot", "press Enter", 12),
        Line::from(""),
        kv_line("shell", "press q", 12),
    ];
    draw_focal_panel(frame, chunks[2], "summary", SUCCESS, body);
}

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
