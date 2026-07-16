use super::*;

pub(in crate::ui) fn draw_disk_select(frame: &mut Frame, area: Rect, app: &App) {
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

// pub(in crate::ui) fn draw_partition(...) { ... }  // Step 4 hidden for now
/*
pub(in crate::ui) fn draw_partition(frame: &mut Frame, area: Rect, lines: &[String]) {
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
