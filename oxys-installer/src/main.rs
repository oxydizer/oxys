use std::{io, time::Duration};

use crossterm::{
    event::{self, Event, KeyEventKind},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{prelude::*, Terminal};

mod app;
mod hardware;
mod provisioning;
mod ui;

use app::App;
use ui::draw_ui;

#[tokio::main]
async fn main() -> io::Result<()> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let (result, reboot_requested) = run_app(&mut terminal).await;

    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;

    if reboot_requested {
        // Boot into the freshly installed system rather than dropping back to
        // the live medium. `reboot` needs privileges; the installer already
        // runs as root on the live ISO. If it somehow fails, fall through and
        // return the run result so the user lands in a shell instead of hanging.
        println!("Rebooting...");
        let _ = std::process::Command::new("reboot").status();
    }

    result
}

async fn run_app(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
) -> (io::Result<()>, bool) {
    let mut app = App::new();

    let result = loop {
        app.poll_streams();
        if let Err(err) = terminal.draw(|f| draw_ui(f, &app)) {
            break Err(err);
        }

        if app.last_tick.elapsed() >= Duration::from_millis(30) {
            app.on_tick();
            app.last_tick = std::time::Instant::now();
        }

        match event::poll(Duration::from_millis(10)) {
            Ok(true) => match event::read() {
                Ok(Event::Key(key)) => {
                    if key.kind == KeyEventKind::Press && app.on_key(key) {
                        break Ok(());
                    }
                }
                Ok(_) => {}
                Err(err) => break Err(err),
            },
            Ok(false) => {}
            Err(err) => break Err(err),
        }

        if let Some(file) = app.take_pending_edit() {
            // Suspend the TUI for the external editor, then restore it. Any
            // terminal error here ends the loop with that error.
            let editor = (|| -> io::Result<()> {
                disable_raw_mode()?;
                execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
                let _ = std::process::Command::new("nano").arg(&file).status();
                enable_raw_mode()?;
                execute!(terminal.backend_mut(), EnterAlternateScreen)?;
                // Force a full clear because nano overwrote the terminal; without
                // it ratatui's diff-based drawing can leave stale content and only
                // repaint the changed parts (e.g. the "(edited)" marker).
                terminal.clear()?;
                // Immediately redraw the current state (including any markers).
                terminal.draw(|f| draw_ui(f, &app))?;
                Ok(())
            })();
            if let Err(err) = editor {
                break Err(err);
            }
        }
    };

    (result, app.reboot_requested)
}
