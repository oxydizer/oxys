use std::{fs, io, path::Path, time::Duration};

use crossterm::{
    cursor::{Hide, Show},
    event::{self, Event, KeyCode, KeyEventKind},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ratatui::{
    Frame, Terminal, TerminalOptions, Viewport,
    backend::CrosstermBackend,
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span, Text},
    widgets::{Block, BorderType, Borders, Paragraph, Wrap},
};

use crate::Result;

const BG: Color = Color::Rgb(5, 4, 5);
const SURFACE: Color = Color::Rgb(12, 10, 12);
const ACCENT: Color = Color::Rgb(255, 82, 34);
const SUCCESS: Color = Color::Rgb(122, 158, 112);
const FG: Color = Color::Rgb(233, 234, 234);
const DIM: Color = Color::Rgb(143, 143, 144);
const FAINT: Color = Color::Rgb(58, 56, 57);

const OVERVIEW_URL: &str = "github.com/oxydizer/oxys/blob/main/oxys/OVERVIEW.md";
const PROJECT_URL: &str = "github.com/oxydizer/oxys";
const PORTAGE_VDB: &str = "/var/db/pkg";

struct SystemInfo {
    kernel: String,
    packages: Option<usize>,
}

impl SystemInfo {
    fn detect() -> Self {
        let kernel = fs::read_to_string("/proc/sys/kernel/osrelease")
            .map(|release| release.trim().to_owned())
            .ok()
            .filter(|release| !release.is_empty())
            .unwrap_or_else(|| "unavailable".into());

        Self {
            kernel,
            packages: count_installed_packages(Path::new(PORTAGE_VDB)),
        }
    }

    fn line(&self) -> String {
        let packages = self
            .packages
            .map(|count| count.to_string())
            .unwrap_or_else(|| "unavailable".into());
        format!(
            "System: OxysOS {}  |  Kernel: {}  |  Packages: {}",
            env!("CARGO_PKG_VERSION"),
            self.kernel,
            packages
        )
    }
}

pub(crate) fn run() -> Result<()> {
    let info = SystemInfo::detect();
    let _screen = ScreenGuard::enter()?;
    let backend = CrosstermBackend::new(io::stdout());
    let mut terminal = Terminal::with_options(
        backend,
        TerminalOptions {
            viewport: Viewport::Fixed(terminal_area()),
        },
    )?;

    loop {
        terminal.draw(|frame| render(frame, &info))?;

        if event::poll(Duration::from_millis(250))? {
            match event::read()? {
                Event::Key(key)
                    if matches!(key.kind, KeyEventKind::Press | KeyEventKind::Repeat)
                        && matches!(key.code, KeyCode::Char('q' | 'Q')) =>
                {
                    break;
                }
                Event::Resize(width, height) => {
                    terminal.resize(Rect::new(0, 0, width.max(1), height.max(1)))?;
                }
                _ => {}
            }
        }
    }

    Ok(())
}

fn terminal_area() -> Rect {
    crossterm::terminal::window_size()
        .ok()
        .filter(|size| size.columns > 0 && size.rows > 0)
        .map(|size| Rect::new(0, 0, size.columns, size.rows))
        .unwrap_or_else(|| Rect::new(0, 0, 80, 24))
}

struct ScreenGuard;

impl ScreenGuard {
    fn enter() -> io::Result<Self> {
        enable_raw_mode()?;
        if let Err(error) = execute!(io::stdout(), EnterAlternateScreen, Hide) {
            let _ = disable_raw_mode();
            return Err(error);
        }
        Ok(Self)
    }
}

impl Drop for ScreenGuard {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
        let _ = execute!(io::stdout(), Show, LeaveAlternateScreen);
    }
}

fn render(frame: &mut Frame<'_>, info: &SystemInfo) {
    let area = frame.area();
    frame.render_widget(Block::default().style(Style::default().bg(BG)), area);

    if area.width < 54 || area.height < 18 {
        render_small(frame, area, info);
        return;
    }

    let logo =
        Paragraph::new("OXYS").style(Style::default().fg(ACCENT).add_modifier(Modifier::BOLD));
    frame.render_widget(logo, Rect::new(2, 1, 8, 1));

    let status = Paragraph::new(info.line())
        .style(Style::default().fg(SUCCESS))
        .alignment(Alignment::Right)
        .wrap(Wrap { trim: true });
    frame.render_widget(status, Rect::new(10, 1, area.width.saturating_sub(12), 2));

    let popup = centered_rect(area, 92, 29);
    let block = Block::default()
        .title(Line::from(vec![
            Span::styled(
                " OXYS WELCOME ",
                Style::default().fg(FG).add_modifier(Modifier::BOLD),
            ),
            Span::styled("•", Style::default().fg(ACCENT)),
        ]))
        .title_alignment(Alignment::Center)
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(FAINT).add_modifier(Modifier::BOLD))
        .style(Style::default().bg(SURFACE).fg(FG));
    let inner = block.inner(popup);
    frame.render_widget(block, popup);

    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(2),
            Constraint::Length(2),
            Constraint::Length(1),
            Constraint::Length(5),
            Constraint::Length(1),
            Constraint::Length(4),
            Constraint::Min(0),
            Constraint::Length(1),
        ])
        .margin(1)
        .split(inner);

    let title = Paragraph::new(format!("Welcome to Oxys v{}", env!("CARGO_PKG_VERSION")))
        .style(Style::default().fg(FG).add_modifier(Modifier::BOLD))
        .alignment(Alignment::Center);
    frame.render_widget(title, rows[0]);

    let intro =
        Paragraph::new("Your declarative, Rust-first home for building and managing OxysOS.")
            .style(Style::default().fg(DIM))
            .alignment(Alignment::Center);
    frame.render_widget(intro, rows[1]);

    frame.render_widget(section_title("QUICK START"), rows[2]);
    frame.render_widget(quick_commands(), rows[3]);
    frame.render_widget(section_title("DOCUMENTATION"), rows[4]);
    frame.render_widget(documentation(), rows[5]);

    let footer = Paragraph::new(Line::from(vec![
        Span::styled(
            "Q",
            Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
        ),
        Span::styled("  quit", Style::default().fg(DIM)),
    ]))
    .alignment(Alignment::Center);
    frame.render_widget(footer, rows[7]);
}

fn render_small(frame: &mut Frame<'_>, area: Rect, info: &SystemInfo) {
    let text = Text::from(vec![
        Line::styled(
            format!("Welcome to Oxys v{}", env!("CARGO_PKG_VERSION")),
            Style::default().fg(FG).add_modifier(Modifier::BOLD),
        ),
        Line::raw(""),
        Line::styled(info.line(), Style::default().fg(SUCCESS)),
        Line::raw(""),
        Line::from(vec![
            Span::styled(
                "Q",
                Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
            ),
            Span::styled("  quit", Style::default().fg(DIM)),
        ]),
    ]);
    let widget = Paragraph::new(text)
        .alignment(Alignment::Center)
        .wrap(Wrap { trim: true })
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_type(BorderType::Rounded)
                .border_style(Style::default().fg(FAINT))
                .style(Style::default().bg(SURFACE)),
        );
    frame.render_widget(widget, centered_rect(area, area.width.saturating_sub(2), 9));
}

fn section_title(title: &'static str) -> Paragraph<'static> {
    Paragraph::new(title).style(Style::default().fg(ACCENT).add_modifier(Modifier::BOLD))
}

fn quick_commands() -> Paragraph<'static> {
    Paragraph::new(Text::from(vec![
        command("oxys help", "Open the complete command reference."),
        command(
            "oxys compile [config.fe2o3]",
            "Compile your system configuration.",
        ),
        command("oxys check", "Preview package changes safely."),
        command("oxys diff", "Compare config with the running system."),
        command("oxys update --dry-run", "Inspect a guarded system update."),
    ]))
}

fn command(command: &'static str, description: &'static str) -> Line<'static> {
    Line::from(vec![
        Span::styled(
            format!("  {command:<31}"),
            Style::default().fg(SUCCESS).add_modifier(Modifier::BOLD),
        ),
        Span::styled(description, Style::default().fg(DIM)),
    ])
}

fn documentation() -> Paragraph<'static> {
    Paragraph::new(Text::from(vec![
        Line::styled("  System and configuration guide", Style::default().fg(FG)),
        Line::styled(
            format!("    {OVERVIEW_URL}"),
            Style::default()
                .fg(ACCENT)
                .add_modifier(Modifier::UNDERLINED),
        ),
        Line::styled("  Project home and issue tracker", Style::default().fg(FG)),
        Line::styled(
            format!("    {PROJECT_URL}"),
            Style::default()
                .fg(ACCENT)
                .add_modifier(Modifier::UNDERLINED),
        ),
    ]))
}

fn centered_rect(area: Rect, max_width: u16, max_height: u16) -> Rect {
    let width = max_width.min(area.width.saturating_sub(4)).max(1);
    let height = max_height.min(area.height.saturating_sub(4)).max(1);
    Rect::new(
        area.x + area.width.saturating_sub(width) / 2,
        area.y + area.height.saturating_sub(height) / 2,
        width,
        height,
    )
}

fn count_installed_packages(vdb: &Path) -> Option<usize> {
    let categories = fs::read_dir(vdb).ok()?;
    let mut count = 0;

    for category in categories.flatten() {
        if !category.file_type().is_ok_and(|kind| kind.is_dir()) {
            continue;
        }
        let Ok(packages) = fs::read_dir(category.path()) else {
            continue;
        };
        count += packages
            .flatten()
            .filter(|package| package.file_type().is_ok_and(|kind| kind.is_dir()))
            .count();
    }

    Some(count)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn system_line_contains_real_fields() {
        let info = SystemInfo {
            kernel: "6.12.1-oxys".into(),
            packages: Some(847),
        };

        let line = info.line();
        assert!(line.contains(concat!("System: OxysOS ", env!("CARGO_PKG_VERSION"))));
        assert!(line.contains("Kernel: 6.12.1-oxys"));
        assert!(line.contains("Packages: 847"));
    }

    #[test]
    fn package_count_reads_portage_vdb_layout() {
        let temp = tempfile::tempdir().expect("tempdir");
        fs::create_dir_all(temp.path().join("sys-apps/coreutils-9.7")).expect("first package");
        fs::create_dir_all(temp.path().join("gui-wm/niri-25.11")).expect("second package");
        fs::write(temp.path().join("README"), "not a package").expect("unrelated file");

        assert_eq!(count_installed_packages(temp.path()), Some(2));
    }

    #[test]
    fn missing_portage_vdb_is_unavailable() {
        assert_eq!(
            count_installed_packages(Path::new("/definitely/missing/oxys-vdb")),
            None
        );
    }
}
