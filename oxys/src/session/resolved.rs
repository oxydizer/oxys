use super::*;

impl ResolvedSession {
    /// Validate immutable source-image pieces that cannot be satisfied by the
    /// later package-emerge step. This runs while the install plan is built,
    /// before rsync or any target mutation is returned to the executor.
    pub fn validate_source(&self, source_root: &Path) -> Result<(), SessionResolveError> {
        if self.policy.mode != ResolvedSessionMode::Graphical {
            return Ok(());
        }

        require_executable(
            source_root,
            &["usr/local/bin/oxys-login"],
            "session.login = oxys_login requires /usr/local/bin/oxys-login in the source image",
        )?;
        require_executable(
            source_root,
            &["sbin/agetty", "usr/sbin/agetty", "usr/bin/agetty"],
            "graphical tty login requires an executable agetty in the source image",
        )?;
        if matches!(
            self.policy.login,
            LoginFrontend::OxysLogin {
                fallback_tty_login: true,
                ..
            }
        ) {
            require_executable(
                source_root,
                &["bin/login", "usr/bin/login"],
                "session.login fallback_tty_login requires executable /bin/login in the source image",
            )?;
        }
        if !["etc/pam.d/login", "etc/pam.d/system-auth"]
            .iter()
            .any(|path| source_root.join(path).is_file())
        {
            return Err(invalid(
                "oxys-login requires source-image PAM service configuration at /etc/pam.d/login or /etc/pam.d/system-auth",
            ));
        }
        Ok(())
    }

    pub fn materialize_manifest(&self, manifest: &SystemManifest) -> SystemManifest {
        let mut result = manifest.clone();
        for atom in &self.requirements.packages {
            if !has_package(&result, atom) {
                let mut package = Package::new(atom);
                if atom == "media-video/pipewire" {
                    package = package
                        .from_source()
                        .use_flags(["sound-server", "pipewire-alsa"]);
                }
                result.packages.push(package);
            } else if atom == "media-video/pipewire" {
                if let Some(package) = result
                    .packages
                    .iter_mut()
                    .find(|p| package_matches(p, atom))
                {
                    for flag in ["sound-server", "pipewire-alsa"] {
                        if !package.use_flags.iter().any(|existing| existing == flag) {
                            package.use_flags.push(flag.to_owned());
                            package.from_source = true;
                        }
                    }
                }
            }
        }
        for service in &self.requirements.services {
            if !result.services.enabled.contains(service) {
                result.services.enabled.push(service.clone());
            }
            result
                .services
                .disabled
                .retain(|disabled| disabled != service);
        }
        if let Some(index) = self.policy.user_index {
            for group in &self.requirements.user_groups {
                if !result.users[index].groups.contains(group) {
                    result.users[index].groups.push(group.clone());
                }
            }
        }
        if self.policy.display_stack.is_some() {
            result.display_stack = self.policy.display_stack;
        }
        if self.policy.audio_stack.is_some() {
            result.audio_stack = self.policy.audio_stack;
        }
        result
    }

    pub fn render(&self) -> String {
        let mut lines = vec![format!(
            "session policy: {}",
            match self.policy.mode {
                ResolvedSessionMode::Text => "text",
                ResolvedSessionMode::Graphical => "graphical",
            }
        )];
        for decision in &self.decisions {
            lines.push(format!(
                "{} = {} [{}]: {}",
                decision.field, decision.value, decision.source, decision.reason
            ));
        }
        for warning in &self.warnings {
            lines.push(format!("warning: {warning}"));
        }
        if !self.requirements.packages.is_empty() {
            lines.push(format!(
                "packages: {}",
                self.requirements.packages.join(", ")
            ));
        }
        if !self.requirements.services.is_empty() {
            lines.push(format!(
                "services: {}",
                self.requirements.services.join(", ")
            ));
        }
        if !self.requirements.user_groups.is_empty() {
            lines.push(format!(
                "user groups: {}",
                self.requirements.user_groups.join(", ")
            ));
        }
        for (name, value) in &self.requirements.environment {
            lines.push(format!("environment: {name}={value}"));
        }
        for ordering in &self.requirements.startup {
            lines.push(format!("startup: {ordering}"));
        }
        lines.join("\n")
    }
}
