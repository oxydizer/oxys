use std::env;
use std::ffi::{CStr, CString};
use std::fs;
use std::io::{self, Stdout};
use std::os::raw::{c_int, c_void};
use std::os::unix::process::CommandExt;
use std::process::Command;
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use chrono::Local;
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use pam::{ffi as pam_ffi, Client};
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};
use ratatui::{Frame, Terminal};
use uzers::get_user_by_name;
use uzers::os::unix::UserExt;

const LOGOS: &[&str] = &[
    r#"   _  __
  / |/ /__  ____ ___
 /    / _ \/ __ `__ \
/_/|_/\___/_/ /_/ /_/
"#,
    r#" _ __  _ _ __ _
| '_ \| | '_ \ |
| | | | | |_)| |
|_| |_|_| .__/|_|
        |_|
"#,
    r#" .----------------.
 |   O X I S      |
 |  login shell   |
 '----------------'
"#,
];

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum FocusField {
    Username,
    Password,
}

struct App {
    username: String,
    password: String,
    focus: FocusField,
    failed_attempts: u32,
    status: Option<String>,
    wrong_password: bool,
    logo: &'static str,
    clock: String,
    hostname: String,
    uptime: String,
    boot_time: String,
    spinner_index: usize,
    auth_in_progress: bool,
}

impl App {
    fn new() -> Self {
        Self {
            username: current_username(),
            password: String::new(),
            focus: FocusField::Password,
            failed_attempts: 0,
            status: None,
            wrong_password: false,
            logo: random_logo(),
            clock: formatted_clock(),
            hostname: current_hostname(),
            uptime: formatted_uptime(),
            boot_time: formatted_boot_time(),
            spinner_index: 0,
            auth_in_progress: false,
        }
    }

    fn selected_style(&self, field: FocusField) -> Style {
        if self.focus == field {
            Style::default()
                .fg(theme::FG)
                .bg(theme::ACCENT_DIM)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(theme::DIM)
        }
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut terminal = setup_terminal()?;
    let result = run_app(&mut terminal);
    restore_terminal(&mut terminal)?;

    match result {
        Ok(AppExit::Quit) => Ok(()),
        Ok(AppExit::LaunchSession { username, password }) => {
            exec_niri_session(&username, &password)
        }
        Err(err) => Err(err),
    }
}

fn setup_terminal() -> Result<Terminal<CrosstermBackend<Stdout>>, Box<dyn std::error::Error>> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;
    terminal.show_cursor()?;
    Ok(terminal)
}

fn restore_terminal(
    terminal: &mut Terminal<CrosstermBackend<Stdout>>,
) -> Result<(), Box<dyn std::error::Error>> {
    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;
    Ok(())
}

fn run_app(
    terminal: &mut Terminal<CrosstermBackend<Stdout>>,
) -> Result<AppExit, Box<dyn std::error::Error>> {
    let mut app = App::new();
    let (tick_tx, tick_rx) = mpsc::channel::<()>();
    let mut auth_rx: Option<Receiver<bool>> = None;

    thread::spawn(move || loop {
        if tick_tx.send(()).is_err() {
            break;
        }
        thread::sleep(Duration::from_millis(100));
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
                    app.status = Some("Launching niri-session...".to_string());
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

        if event::poll(Duration::from_millis(100))? {
            if let Event::Key(key) = event::read()? {
                if key.kind == KeyEventKind::Press {
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
    }
}

enum AppExit {
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
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('q') {
            return LoopAction::Quit;
        }
        return LoopAction::Continue;
    }

    if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('q') {
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
        KeyCode::Char(ch) => {
            if key.modifiers.is_empty() || key.modifiers == KeyModifiers::SHIFT {
                match app.focus {
                    FocusField::Username => app.username.push(ch),
                    FocusField::Password => app.password.push(ch),
                }
                app.wrong_password = false;
                app.status = None;
            }
        }
        _ => {}
    }

    LoopAction::Continue
}

const SPINNER_FRAMES: &[&str] = &["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

fn render(frame: &mut Frame<'_>, app: &App) {
    let area = frame.area();
    frame.render_widget(Block::default().style(Style::default().bg(theme::BG)), area);

    render_logo(frame, area, app.logo);
    render_system_info(frame, area, app);
    render_clock(frame, area, &app.clock);
    render_boot_time(frame, area, &app.boot_time);
    render_login(frame, area, app);
}

fn render_logo(frame: &mut Frame<'_>, area: Rect, logo: &str) {
    let logo_width = logo.lines().map(str::len).max().unwrap_or(0) as u16 + 2;
    let logo_height = logo.lines().count() as u16 + 2;
    let logo_area = Rect::new(
        2,
        1,
        logo_width.min(area.width.saturating_sub(2)),
        logo_height.min(area.height.saturating_sub(1)),
    );

    let logo_widget = Paragraph::new(logo)
        .style(Style::default().fg(theme::ACCENT))
        .alignment(Alignment::Left);

    frame.render_widget(logo_widget, logo_area);
}

fn render_system_info(frame: &mut Frame<'_>, area: Rect, app: &App) {
    let text = format!("{} | up {}", app.hostname, app.uptime);
    let info_width = text.len() as u16 + 4;
    let info_area = Rect::new(
        area.width.saturating_sub(info_width + 2),
        1,
        info_width.min(area.width.saturating_sub(2)),
        3,
    );

    let info = Paragraph::new(text)
        .style(Style::default().fg(theme::SUCCESS))
        .alignment(Alignment::Right);

    frame.render_widget(info, info_area);
}

fn render_clock(frame: &mut Frame<'_>, area: Rect, clock: &str) {
    let width = clock.len() as u16 + 4;
    let clock_area = Rect::new(
        area.width.saturating_sub(width + 2),
        area.height.saturating_sub(3),
        width.min(area.width.saturating_sub(2)),
        3,
    );

    let clock_widget = Paragraph::new(clock)
        .style(Style::default().fg(theme::ACCENT))
        .alignment(Alignment::Right);

    frame.render_widget(clock_widget, clock_area);
}

fn render_boot_time(frame: &mut Frame<'_>, area: Rect, boot_time: &str) {
    let text = format!("booted in {}", boot_time);
    let width = text.len() as u16 + 2;
    let boot_area = Rect::new(2, area.height.saturating_sub(3), width, 1);

    let widget = Paragraph::new(text)
        .style(Style::default().fg(theme::DIM).add_modifier(Modifier::DIM))
        .alignment(Alignment::Left);

    frame.render_widget(widget, boot_area);
}

fn render_login(frame: &mut Frame<'_>, area: Rect, app: &App) {
    let popup = centered_rect(46, 34, area);
    frame.render_widget(Clear, popup);

    let border_color = if app.auth_in_progress || app.wrong_password {
        theme::ACCENT
    } else {
        theme::FAINT
    };

    let mut title = vec![Span::styled(
        " OXYS LOGIN ",
        Style::default().fg(theme::FG).add_modifier(Modifier::BOLD),
    )];
    if app.failed_attempts > 0 {
        title.push(Span::styled(" •", Style::default().fg(theme::ACCENT)));
        title.push(Span::styled(
            format!(" {} failed", app.failed_attempts),
            Style::default().fg(theme::DIM),
        ));
    }
    let block = Block::default()
        .title(Line::from(title))
        .title_alignment(Alignment::Center)
        .borders(Borders::ALL)
        .border_style(
            Style::default()
                .fg(border_color)
                .add_modifier(Modifier::BOLD),
        )
        .style(Style::default().bg(theme::SURFACE).fg(theme::FG));

    frame.render_widget(block, popup);

    let inner = Rect::new(
        popup.x + 2,
        popup.y + 2,
        popup.width.saturating_sub(4),
        popup.height.saturating_sub(4),
    );

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(2),
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(2),
            Constraint::Length(2),
        ])
        .split(inner);

    let header = Paragraph::new("Authenticate with PAM and start niri-session")
        .style(Style::default().fg(theme::DIM))
        .alignment(Alignment::Center);
    frame.render_widget(header, chunks[0]);

    let username_marker = if app.focus == FocusField::Username {
        Span::styled(
            "› ",
            Style::default()
                .fg(theme::ACCENT)
                .add_modifier(Modifier::BOLD),
        )
    } else {
        Span::styled("  ", Style::default().fg(theme::FAINT))
    };
    let username_line = Line::from(vec![
        username_marker,
        Span::styled(
            "Username ",
            Style::default().fg(theme::DIM).add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            app.username.as_str(),
            app.selected_style(FocusField::Username),
        ),
    ]);
    let username = Paragraph::new(username_line).style(Style::default().bg(theme::SURFACE));
    frame.render_widget(username, chunks[1]);

    let password_mask = "*".repeat(app.password.chars().count());
    let password_marker = if app.focus == FocusField::Password {
        Span::styled(
            "› ",
            Style::default()
                .fg(theme::ACCENT)
                .add_modifier(Modifier::BOLD),
        )
    } else {
        Span::styled("  ", Style::default().fg(theme::FAINT))
    };
    let password_line = Line::from(vec![
        password_marker,
        Span::styled(
            "Password ",
            Style::default().fg(theme::DIM).add_modifier(Modifier::BOLD),
        ),
        Span::styled(password_mask, app.selected_style(FocusField::Password)),
    ]);
    let password = Paragraph::new(password_line).style(Style::default().bg(theme::SURFACE));
    frame.render_widget(password, chunks[3]);

    let spacer = Paragraph::new("");
    frame.render_widget(spacer, chunks[4]);

    let spinner_text = if app.auth_in_progress {
        SPINNER_FRAMES[app.spinner_index]
    } else {
        ""
    };
    let spinner = Paragraph::new(spinner_text)
        .style(Style::default().fg(theme::ACCENT))
        .alignment(Alignment::Center);
    frame.render_widget(spinner, chunks[5]);

    let status_text = app
        .status
        .as_deref()
        .unwrap_or("Tab switches fields. Enter submits. Ctrl+Q exits. Ctrl+C is ignored.");
    let status_color = if app.wrong_password {
        theme::WARN
    } else if app.auth_in_progress {
        theme::ACCENT
    } else {
        theme::DIM
    };
    let status = Paragraph::new(status_text)
        .style(Style::default().fg(status_color))
        .alignment(Alignment::Center);
    frame.render_widget(status, chunks[6]);

    match app.focus {
        FocusField::Username => {
            let cursor_x = chunks[1]
                .x
                .saturating_add("› Username ".len() as u16)
                .saturating_add(app.username.chars().count() as u16);
            frame.set_cursor_position((cursor_x, chunks[1].y));
        }
        FocusField::Password => {
            let cursor_x = chunks[3]
                .x
                .saturating_add("› Password ".len() as u16)
                .saturating_add(app.password.chars().count() as u16);
            frame.set_cursor_position((cursor_x, chunks[3].y));
        }
    }
}

fn centered_rect(percent_x: u16, percent_y: u16, area: Rect) -> Rect {
    let vertical = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage((100 - percent_y) / 2),
            Constraint::Percentage(percent_y),
            Constraint::Percentage((100 - percent_y) / 2),
        ])
        .split(area);

    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - percent_x) / 2),
            Constraint::Percentage(percent_x),
            Constraint::Percentage((100 - percent_x) / 2),
        ])
        .split(vertical[1])[1]
}

fn authenticate(username: &str, password: &str) -> Result<(), String> {
    const SERVICES: &[&str] = &["login", "system-auth"];

    let mut last_error = "failed to initialize PAM".to_string();
    for service in SERVICES {
        let mut client = match Client::with_password(service) {
            Ok(client) => client,
            Err(error) => {
                last_error = format!("{error:?}");
                continue;
            }
        };

        client
            .conversation_mut()
            .set_credentials(username.to_string(), password.to_string());

        match client.authenticate() {
            Ok(()) => return Ok(()),
            Err(error) => {
                last_error = format!("{error:?}");
            }
        }
    }

    Err(last_error)
}

fn exec_niri_session(username: &str, password: &str) -> Result<(), Box<dyn std::error::Error>> {
    let user = get_user_by_name(username)
        .ok_or_else(|| format!("unable to resolve user record for {username}"))?;

    become_session_leader()?;

    let shell = user.shell().to_string_lossy().to_string();
    let home = user.home_dir().to_string_lossy().to_string();
    let uid = user.uid();
    let gid = user.primary_group_id();
    let mut pam_session = PamSession::open(username, password)?;
    let environment = session_environment(&mut pam_session, username, uid, &home, &shell);
    let initgroups_user = CString::new(username)?;

    let _ = unsafe {
        Command::new("niri-session")
            .env_clear()
            .envs(environment)
            .current_dir(&home)
            .pre_exec(move || {
                if libc::initgroups(initgroups_user.as_ptr(), gid) != 0 {
                    return Err(io::Error::last_os_error());
                }
                if libc::setgid(gid) != 0 {
                    return Err(io::Error::last_os_error());
                }
                if libc::setuid(uid) != 0 {
                    return Err(io::Error::last_os_error());
                }
                Ok(())
            })
            .exec()
    };

    drop(pam_session);
    Err("failed to exec niri-session".into())
}

struct PamCredentials {
    username: CString,
    password: CString,
}

struct PamSession {
    handle: *mut pam_ffi::pam_handle_t,
    _credentials: Box<PamCredentials>,
    credentials_established: bool,
    session_open: bool,
}

impl PamSession {
    fn open(username: &str, password: &str) -> Result<Self, String> {
        const SERVICES: &[&str] = &["login", "system-auth"];

        let mut last_error = "failed to initialize PAM".to_string();
        for service in SERVICES {
            match Self::open_with_service(service, username, password) {
                Ok(session) => return Ok(session),
                Err(error) => last_error = error,
            }
        }

        Err(last_error)
    }

    fn open_with_service(service: &str, username: &str, password: &str) -> Result<Self, String> {
        let service = CString::new(service).map_err(|_| "PAM service contains NUL".to_string())?;
        let mut credentials = Box::new(PamCredentials {
            username: CString::new(username).map_err(|_| "username contains NUL".to_string())?,
            password: CString::new(password).map_err(|_| "password contains NUL".to_string())?,
        });
        let conversation = pam_ffi::pam_conv {
            conv: Some(pam_conversation),
            appdata_ptr: credentials.as_mut() as *mut PamCredentials as *mut c_void,
        };

        let mut handle = std::ptr::null_mut();
        let start_code = unsafe {
            pam_ffi::pam_start(
                service.as_ptr(),
                credentials.username.as_ptr(),
                &conversation,
                &mut handle,
            )
        };
        if start_code != pam_ffi::PAM_SUCCESS {
            return Err(format!("pam_start failed: {start_code}"));
        }

        let mut session = Self {
            handle,
            _credentials: credentials,
            credentials_established: false,
            session_open: false,
        };

        session.set_tty();
        session.check(
            unsafe { pam_ffi::pam_authenticate(session.handle, 0) },
            "authenticate",
        )?;
        session.check(
            unsafe { pam_ffi::pam_acct_mgmt(session.handle, 0) },
            "account",
        )?;
        session.check(
            unsafe { pam_ffi::pam_setcred(session.handle, pam_ffi::PAM_ESTABLISH_CRED) },
            "set credentials",
        )?;
        session.credentials_established = true;
        session.check(
            unsafe { pam_ffi::pam_open_session(session.handle, 0) },
            "open session",
        )?;
        session.session_open = true;
        session.check(
            unsafe { pam_ffi::pam_setcred(session.handle, pam_ffi::PAM_REINITIALIZE_CRED) },
            "refresh credentials",
        )?;

        Ok(session)
    }

    fn environment(&mut self) -> Vec<(String, String)> {
        let env = unsafe { pam_ffi::pam_getenvlist(self.handle) };
        if env.is_null() {
            return Vec::new();
        }

        let mut result = Vec::new();
        unsafe {
            let mut current = env;
            while !(*current).is_null() {
                let raw = CStr::from_ptr(*current).to_string_lossy();
                if let Some((key, value)) = raw.split_once('=') {
                    if !key.is_empty() {
                        result.push((key.to_string(), value.to_string()));
                    }
                }
                current = current.add(1);
            }
            pam_ffi::pam_misc_drop_env(env);
        }
        result
    }

    fn check(&mut self, code: c_int, action: &str) -> Result<(), String> {
        if code == pam_ffi::PAM_SUCCESS {
            return Ok(());
        }

        let message = unsafe {
            let ptr = pam_ffi::pam_strerror(self.handle, code);
            if ptr.is_null() {
                format!("PAM error {code}")
            } else {
                CStr::from_ptr(ptr).to_string_lossy().into_owned()
            }
        };
        Err(format!("PAM {action} failed: {message}"))
    }

    fn set_tty(&mut self) {
        let Some(tty) = current_tty() else {
            return;
        };
        let Ok(tty) = CString::new(tty) else {
            return;
        };
        unsafe {
            pam_ffi::pam_set_item(self.handle, pam_ffi::PAM_TTY, tty.as_ptr() as *const c_void);
        }
    }
}

impl Drop for PamSession {
    fn drop(&mut self) {
        let mut status = pam_ffi::PAM_SUCCESS;
        if self.session_open {
            status = unsafe { pam_ffi::pam_close_session(self.handle, 0) };
        }
        if self.credentials_established {
            status = unsafe { pam_ffi::pam_setcred(self.handle, pam_ffi::PAM_DELETE_CRED) };
        }
        unsafe {
            pam_ffi::pam_end(self.handle, status);
        }
    }
}

unsafe extern "C" fn pam_conversation(
    num_msg: c_int,
    msg: *mut *const pam_ffi::pam_message,
    out_resp: *mut *mut pam_ffi::pam_response,
    appdata_ptr: *mut c_void,
) -> c_int {
    unsafe {
        if num_msg <= 0 || num_msg > pam_ffi::PAM_MAX_NUM_MSG || msg.is_null() || out_resp.is_null() {
            return pam_ffi::PAM_CONV_ERR;
        }
        let credentials = &*(appdata_ptr as *const PamCredentials);
        let responses = libc::calloc(
            num_msg as usize,
            std::mem::size_of::<pam_ffi::pam_response>(),
        ) as *mut pam_ffi::pam_response;
        if responses.is_null() {
            return pam_ffi::PAM_BUF_ERR;
        }

        for index in 0..num_msg as isize {
            let message = *msg.offset(index);
            if message.is_null() {
                free_pam_responses(responses, index);
                return pam_ffi::PAM_CONV_ERR;
            }

            let response = &mut *responses.offset(index);
            let answer = match (*message).msg_style {
                pam_ffi::PAM_PROMPT_ECHO_ON => Some(credentials.username.as_ptr()),
                pam_ffi::PAM_PROMPT_ECHO_OFF => Some(credentials.password.as_ptr()),
                pam_ffi::PAM_TEXT_INFO | pam_ffi::PAM_ERROR_MSG => None,
                _ => {
                    free_pam_responses(responses, index);
                    return pam_ffi::PAM_CONV_ERR;
                }
            };

            if let Some(answer) = answer {
                response.resp = libc::strdup(answer);
                if response.resp.is_null() {
                    free_pam_responses(responses, index);
                    return pam_ffi::PAM_BUF_ERR;
                }
            }
        }

        *out_resp = responses;
        pam_ffi::PAM_SUCCESS
    }
}

unsafe fn free_pam_responses(responses: *mut pam_ffi::pam_response, initialized: isize) {
    unsafe {
        for index in 0..initialized {
            let response = &mut *responses.offset(index);
            if !response.resp.is_null() {
                libc::free(response.resp as *mut c_void);
            }
        }
        libc::free(responses as *mut c_void);
    }
}

fn session_environment(
    pam_session: &mut PamSession,
    username: &str,
    uid: u32,
    home: &str,
    shell: &str,
) -> Vec<(String, String)> {
    let mut environment = pam_session.environment();
    set_env_default(&mut environment, "USER", username);
    set_env_default(&mut environment, "LOGNAME", username);
    set_env_default(&mut environment, "HOME", home);
    set_env_default(&mut environment, "PWD", home);
    set_env_default(&mut environment, "SHELL", shell);
    set_env_default(
        &mut environment,
        "XDG_RUNTIME_DIR",
        &format!("/run/user/{uid}"),
    );
    set_env_default(&mut environment, "XDG_SESSION_TYPE", "wayland");
    set_env_default(&mut environment, "XDG_SESSION_CLASS", "user");
    set_env_default(
        &mut environment,
        "PATH",
        "/usr/local/sbin:/usr/local/bin:/usr/bin:/bin",
    );
    environment
}

fn set_env_default(environment: &mut Vec<(String, String)>, key: &str, value: &str) {
    if let Some((_, existing)) = environment
        .iter_mut()
        .find(|(existing_key, _)| existing_key == key)
    {
        if existing.trim().is_empty() {
            *existing = value.to_string();
        }
        return;
    }

    environment.push((key.to_string(), value.to_string()));
}

fn become_session_leader() -> io::Result<()> {
    if unsafe { libc::setsid() } == -1 {
        if unsafe { libc::getsid(0) } == unsafe { libc::getpid() } {
            return Ok(());
        }
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

fn current_tty() -> Option<String> {
    fs::read_link("/proc/self/fd/0")
        .ok()
        .map(|path| path.display().to_string())
        .filter(|path| path.starts_with("/dev/"))
}

fn current_username() -> String {
    env::var("USER")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .or_else(username_from_passwd)
        .unwrap_or_else(|| "user".to_string())
}

fn username_from_passwd() -> Option<String> {
    let uid = unsafe { libc::geteuid() };
    let passwd = fs::read_to_string("/etc/passwd").ok()?;

    passwd.lines().find_map(|line| {
        let mut parts = line.split(':');
        let name = parts.next()?;
        let _password = parts.next()?;
        let uid_field = parts.next()?;
        if uid_field.parse::<u32>().ok()? == uid {
            Some(name.to_string())
        } else {
            None
        }
    })
}

fn current_hostname() -> String {
    env::var("HOSTNAME")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .or_else(|| {
            fs::read_to_string("/etc/hostname")
                .ok()
                .map(|value| value.trim().to_string())
                .filter(|value| !value.is_empty())
        })
        .unwrap_or_else(|| "unknown-host".to_string())
}

fn formatted_uptime() -> String {
    let raw = fs::read_to_string("/proc/uptime").unwrap_or_default();
    let seconds = raw
        .split_whitespace()
        .next()
        .and_then(|value| value.split('.').next())
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(0);

    let days = seconds / 86_400;
    let hours = (seconds % 86_400) / 3_600;
    let minutes = (seconds % 3_600) / 60;

    if days > 0 {
        format!("{days}d {hours}h {minutes}m")
    } else if hours > 0 {
        format!("{hours}h {minutes}m")
    } else {
        format!("{minutes}m")
    }
}

fn formatted_clock() -> String {
    Local::now().format("%Y-%m-%d %H:%M:%S").to_string()
}

fn formatted_boot_time() -> String {
    let output = Command::new("systemd-analyze").output();
    let stdout = match output {
        Ok(output) if output.status.success() => {
            String::from_utf8_lossy(&output.stdout).to_string()
        }
        _ => return "unavailable".to_string(),
    };

    stdout
        .lines()
        .find_map(parse_systemd_analyze_total)
        .unwrap_or_else(|| "unavailable".to_string())
}

fn parse_systemd_analyze_total(line: &str) -> Option<String> {
    let total = line.split('=').nth(1)?.trim();
    let value = total.split_whitespace().next()?.trim();
    if value.is_empty() {
        None
    } else if let Some(seconds) = value.strip_suffix('s') {
        let parsed = seconds.parse::<f64>().ok()?;
        Some(format!("{parsed:.2}s"))
    } else {
        Some(value.to_string())
    }
}

fn random_logo() -> &'static str {
    let seed = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos() as usize)
        .unwrap_or(0);
    LOGOS[seed % LOGOS.len()]
}

mod theme {
    use ratatui::style::Color;

    pub const BG: Color = Color::Rgb(5, 4, 5);
    pub const SURFACE: Color = Color::Rgb(12, 10, 12);
    pub const ACCENT: Color = Color::Rgb(255, 82, 34);
    pub const ACCENT_DIM: Color = Color::Rgb(58, 56, 57);
    pub const SUCCESS: Color = Color::Rgb(122, 158, 112);
    pub const WARN: Color = ACCENT;
    pub const FG: Color = Color::Rgb(233, 234, 234);
    pub const DIM: Color = Color::Rgb(143, 143, 144);
    pub const FAINT: Color = Color::Rgb(58, 56, 57);
}
