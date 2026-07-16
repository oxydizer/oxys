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
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use pam::{Client, ffi as pam_ffi};
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Clear, Paragraph};
use ratatui::{Frame, Terminal};
use uzers::get_user_by_name;
use uzers::os::unix::UserExt;

mod runtime;
mod session;
mod system;
mod theme;
mod ui;

use runtime::{AppExit, restore_terminal, run_app, setup_terminal};
use session::{exec_niri_session, exec_tty_login, session_config_value};
use system::{
    current_hostname, formatted_boot_time, formatted_clock, formatted_uptime, random_logo,
};

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
    fallback_tty_login: bool,
}

impl App {
    fn new() -> Self {
        Self {
            username: String::new(),
            password: String::new(),
            focus: FocusField::Username,
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
            fallback_tty_login: session_config_value("OXYS_FALLBACK_TTY_LOGIN").as_deref()
                != Some("false"),
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
        Ok(AppExit::Quit) => exec_tty_login(),
        Ok(AppExit::LaunchSession { username, password }) => {
            exec_niri_session(&username, &password)
        }
        Err(err) => Err(err),
    }
}
