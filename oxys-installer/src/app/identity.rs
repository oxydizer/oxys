use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use oxys::manifest::{Password, User, Username};

use crate::provisioning;

use super::{is_valid_login_name, App, Screen};

impl App {
    /// Load the compiled manifest to see which users need an interactive
    /// name. Returns the next screen: `Usernames` when there is at least one
    /// such user, otherwise falls through to password collection.
    pub(super) fn begin_identity_collection(&mut self) -> Screen {
        if let Some(error) = provisioning::install_permission_error() {
            self.install_lines = vec![format!("[error] {error}")];
            self.install_progress = 100;
            self.install_rx = None;
            if let Some(handle) = self.install_task.take() {
                handle.abort();
            }
            return Screen::Installing;
        }

        let indices: Vec<usize> = self
            .compiled_manifest
            .as_ref()
            .and_then(|path| oxys::compile::load_manifest(path).ok())
            .map(|manifest| manifest.prompt_usernames())
            .unwrap_or_default();

        if indices.is_empty() {
            return self.begin_password_collection();
        }

        self.prompt_username_indices = indices;
        self.username_idx = 0;
        self.username_input.clear();
        self.username_error = None;
        self.collected_usernames.clear();
        Screen::Usernames
    }

    /// Index of the user currently being prompted for a name, if any.
    pub(crate) fn current_prompt_username_index(&self) -> Option<usize> {
        self.prompt_username_indices.get(self.username_idx).copied()
    }

    /// This user's name: the literal baked into the config, or -- for a
    /// `Username::Prompt` user -- whatever has been collected for it so far
    /// on the `Usernames` screen.
    fn resolved_name(&self, index: usize, user: &User) -> String {
        match &user.name {
            Username::Literal(name) => name.clone(),
            Username::Prompt => self
                .collected_usernames
                .get(&index)
                .cloned()
                .unwrap_or_default(),
        }
    }

    /// Handle a keystroke on the username entry screen. Owns all keys so typed
    /// characters (including `q`) never trigger global shortcuts.
    pub(super) fn username_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Esc => {
                // Abandon collection and return to the confirm screen.
                self.prompt_username_indices.clear();
                self.collected_usernames.clear();
                self.username_input.clear();
                self.username_error = None;
                self.current = Screen::Confirm;
            }
            KeyCode::Backspace => {
                self.username_input.pop();
            }
            KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.username_input.push(c);
            }
            KeyCode::Enter => self.username_submit(),
            _ => {}
        }
    }

    fn username_submit(&mut self) {
        let name = self.username_input.trim().to_owned();
        if !is_valid_login_name(&name) {
            self.username_error = Some(
                "enter a valid login name: lowercase letters, digits, - or _, starting with a letter or _"
                    .to_owned(),
            );
            return;
        }

        if let Some(index) = self.current_prompt_username_index() {
            self.collected_usernames.insert(index, name);
        }
        self.username_input.clear();
        self.username_error = None;
        self.username_idx += 1;

        if self.username_idx >= self.prompt_username_indices.len() {
            // All names collected; move on to any password prompts.
            self.current = self.begin_password_collection();
        }
    }

    /// Load the compiled manifest to see which users need an interactive
    /// password. Returns the next screen: `Passwords` when there is at least one
    /// such user, otherwise it kicks off the install and returns `Installing`.
    fn begin_password_collection(&mut self) -> Screen {
        let names: Vec<String> = self
            .compiled_manifest
            .as_ref()
            .and_then(|path| oxys::compile::load_manifest(path).ok())
            .map(|manifest| {
                manifest
                    .users
                    .iter()
                    .enumerate()
                    .filter(|(_, user)| user.password == Password::Prompt)
                    .map(|(index, user)| self.resolved_name(index, user))
                    .collect()
            })
            .unwrap_or_default();

        if names.is_empty() {
            self.start_install();
            return Screen::Installing;
        }

        self.prompt_users = names;
        self.password_idx = 0;
        self.password_input.clear();
        self.password_confirm_input.clear();
        self.password_confirming = false;
        self.password_error = None;
        self.collected_passwords.clear();
        Screen::Passwords
    }

    /// Name of the user currently being prompted, if any.
    pub(crate) fn current_prompt_user(&self) -> Option<&str> {
        self.prompt_users.get(self.password_idx).map(String::as_str)
    }

    /// Handle a keystroke on the password entry screen. Owns all keys so typed
    /// characters (including `q`) never trigger global shortcuts.
    pub(super) fn password_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Esc => {
                // Abandon collection and return to the confirm screen.
                self.prompt_users.clear();
                self.collected_passwords.clear();
                self.password_input.clear();
                self.password_confirm_input.clear();
                self.password_confirming = false;
                self.password_error = None;
                self.current = Screen::Confirm;
            }
            KeyCode::Backspace => {
                if self.password_confirming {
                    self.password_confirm_input.pop();
                } else {
                    self.password_input.pop();
                }
            }
            KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                if self.password_confirming {
                    self.password_confirm_input.push(c);
                } else {
                    self.password_input.push(c);
                }
            }
            KeyCode::Enter => self.password_submit(),
            _ => {}
        }
    }

    fn password_submit(&mut self) {
        if !self.password_confirming {
            if self.password_input.is_empty() {
                self.password_error = Some("password cannot be empty".to_owned());
                return;
            }
            self.password_confirming = true;
            self.password_error = None;
            return;
        }

        if self.password_confirm_input != self.password_input {
            // Mismatch: wipe both entries and restart this user.
            self.password_input.clear();
            self.password_confirm_input.clear();
            self.password_confirming = false;
            self.password_error = Some("passwords did not match, try again".to_owned());
            return;
        }

        if let Some(name) = self.current_prompt_user().map(str::to_owned) {
            self.collected_passwords
                .insert(name, std::mem::take(&mut self.password_input));
        }
        self.password_confirm_input.clear();
        self.password_confirming = false;
        self.password_error = None;
        self.password_idx += 1;

        if self.password_idx >= self.prompt_users.len() {
            // All secrets collected; proceed to install.
            self.start_install();
            self.current = Screen::Installing;
        }
    }
}
