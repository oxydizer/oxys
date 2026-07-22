use super::*;
use super::{
    session::authenticate,
    ui::{SPINNER_FRAMES, render},
};

pub(super) fn setup_terminal()
-> Result<Terminal<CrosstermBackend<Stdout>>, Box<dyn std::error::Error>> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;
    terminal.show_cursor()?;
    Ok(terminal)
}

pub(super) fn restore_terminal(
    terminal: &mut Terminal<CrosstermBackend<Stdout>>,
) -> Result<(), Box<dyn std::error::Error>> {
    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;
    Ok(())
}

pub(super) fn run_app(
    terminal: &mut Terminal<CrosstermBackend<Stdout>>,
) -> Result<AppExit, Box<dyn std::error::Error>> {
    let mut app = App::new();
    let (tick_tx, tick_rx) = mpsc::channel::<()>();
    let mut auth_rx: Option<Receiver<bool>> = None;

    thread::spawn(move || {
        loop {
            if tick_tx.send(()).is_err() {
                break;
            }
            thread::sleep(Duration::from_millis(100));
        }
    });

    loop {
        let mut saw_tick = false;
        while tick_rx.try_recv().is_ok() {
            saw_tick = true;
            if app.auth_in_progress {
                app.spinner_index = (app.spinner_index + 1) % SPINNER_FRAMES.len();
            }
        }
        if saw_tick {
            app.clock = formatted_clock();
            app.uptime = formatted_uptime();
        }

        if let Some(rx) = &auth_rx {
            match rx.try_recv() {
                Ok(true) => {
                    app.auth_in_progress = false;
                    app.status = Some("Launching niri session...".to_string());
                    terminal.draw(|frame| render(frame, &app))?;
                    return Ok(AppExit::LaunchSession {
                        username: app.username.clone(),
                        password: std::mem::take(&mut app.password),
                    });
                }
                Ok(false) => {
                    app.auth_in_progress = false;
                    auth_rx = None;
                    app.failed_attempts += 1;
                    app.password.clear();
                    app.wrong_password = true;
                    app.status = Some("Authentication failed.".to_string());
                }
                Err(TryRecvError::Empty) => {}
                Err(TryRecvError::Disconnected) => {
                    app.auth_in_progress = false;
                    auth_rx = None;
                    app.wrong_password = true;
                    app.status = Some("Authentication error.".to_string());
                }
            }
        }

        terminal.draw(|frame| render(frame, &app))?;

        if event::poll(Duration::from_millis(100))?
            && let Event::Key(key) = event::read()?
                && key.kind == KeyEventKind::Press {
                    match handle_key_event(&mut app, key) {
                        LoopAction::Continue => {}
                        LoopAction::Quit => return Ok(AppExit::Quit),
                        LoopAction::StartAuth { username, password } => {
                            let (tx, rx) = mpsc::channel();
                            app.auth_in_progress = true;
                            app.wrong_password = false;
                            app.spinner_index = 0;
                            app.status = Some("Authenticating...".to_string());
                            auth_rx = Some(rx);
                            thread::spawn(move || {
                                let _ = tx.send(authenticate(&username, &password).is_ok());
                            });
                        }
                    }
                }
    }
}

pub(super) enum AppExit {
    Quit,
    LaunchSession { username: String, password: String },
}

enum LoopAction {
    Continue,
    Quit,
    StartAuth { username: String, password: String },
}

fn handle_key_event(app: &mut App, key: KeyEvent) -> LoopAction {
    if app.auth_in_progress {
        if app.fallback_tty_login
            && key.modifiers.contains(KeyModifiers::CONTROL)
            && key.code == KeyCode::Char('q')
        {
            return LoopAction::Quit;
        }
        return LoopAction::Continue;
    }

    if app.fallback_tty_login
        && key.modifiers.contains(KeyModifiers::CONTROL)
        && key.code == KeyCode::Char('q')
    {
        return LoopAction::Quit;
    }

    if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
        return LoopAction::Continue;
    }

    match key.code {
        KeyCode::Tab | KeyCode::BackTab | KeyCode::Up | KeyCode::Down => {
            app.focus = match app.focus {
                FocusField::Username => FocusField::Password,
                FocusField::Password => FocusField::Username,
            };
        }
        KeyCode::Backspace => match app.focus {
            FocusField::Username => {
                app.username.pop();
                app.wrong_password = false;
                app.status = None;
            }
            FocusField::Password => {
                app.password.pop();
                app.wrong_password = false;
                app.status = None;
            }
        },
        KeyCode::Enter => {
            if app.username.trim().is_empty() {
                app.status = Some("Username is required.".to_string());
                app.wrong_password = true;
                return LoopAction::Continue;
            }

            return LoopAction::StartAuth {
                username: app.username.clone(),
                password: app.password.clone(),
            };
        }
        KeyCode::Char(ch)
            if (key.modifiers.is_empty() || key.modifiers == KeyModifiers::SHIFT) => {
                match app.focus {
                    FocusField::Username => app.username.push(ch),
                    FocusField::Password => app.password.push(ch),
                }
                app.wrong_password = false;
                app.status = None;
            }
        _ => {}
    }

    LoopAction::Continue
}
