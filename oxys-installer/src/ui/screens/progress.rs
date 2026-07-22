use super::*;

pub(in crate::ui) fn draw_install(
    frame: &mut Frame,
    area: Rect,
    lines: &[String],
    progress: u16,
    spinner_idx: usize,
) {
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
        let spin = ASCII_SPINNER[spinner_idx % ASCII_SPINNER.len()];
        status_line(spin, "installing system".to_string(), ACCENT, true)
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
                "install failed — full log at {} (esc back · q quit)",
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

pub(in crate::ui) fn draw_done(
    frame: &mut Frame,
    area: Rect,
    install_elapsed: Option<std::time::Duration>,
) {
    let chunks = screen_chunks(area, 2, 6);
    frame.render_widget(Paragraph::new(section_header("step 7", "done")), chunks[0]);
    let completed_message = install_elapsed
        .map(|elapsed| format!("System installed in {}", format_install_elapsed(elapsed)))
        .unwrap_or_else(|| "Installation complete".to_string());
    frame.render_widget(
        Paragraph::new(status_line("✓", completed_message, SUCCESS, true)),
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
