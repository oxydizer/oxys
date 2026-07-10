use serde::{Deserialize, Serialize};

use super::{Password, Shell, Username};

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Services {
    #[serde(default)]
    pub enabled: Vec<String>,
    #[serde(default)]
    pub disabled: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct User {
    #[serde(default)]
    pub name: Username,
    #[serde(default)]
    pub groups: Vec<String>,
    #[serde(default)]
    pub shell: Shell,
    #[serde(default)]
    pub password: Password,
}

impl User {
    /// Start a new user account with a fixed name. The password defaults to
    /// [`Password::None`] (locked) until set with [`User::password`].
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: Username::Literal(name.into()),
            groups: Vec::new(),
            shell: Shell::default(),
            password: Password::default(),
        }
    }

    /// Start a new user account whose name is collected interactively by the
    /// installer at install time, rather than baked into the config.
    pub fn prompt() -> Self {
        Self {
            name: Username::Prompt,
            groups: Vec::new(),
            shell: Shell::default(),
            password: Password::default(),
        }
    }

    /// Replace this user's supplementary groups.
    pub fn groups(mut self, groups: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.groups = groups.into_iter().map(Into::into).collect();
        self
    }

    /// Add the user to the `wheel` group, granting sudo access, without
    /// disturbing any other groups already set.
    pub fn wheel(mut self) -> Self {
        if !self.is_wheel() {
            self.groups.push("wheel".to_owned());
        }
        self
    }

    /// Set the user's login shell.
    pub fn shell(mut self, shell: Shell) -> Self {
        self.shell = shell;
        self
    }

    /// Set how the user's password is provisioned.
    pub fn password(mut self, password: Password) -> Self {
        self.password = password;
        self
    }

    /// True when the user belongs to the `wheel` group and should receive sudo.
    pub fn is_wheel(&self) -> bool {
        self.groups.iter().any(|group| group == "wheel")
    }
}
