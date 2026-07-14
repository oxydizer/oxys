use serde::{Deserialize, Serialize};

mod accounts;
mod compiler;
mod disk;
mod packages;
mod settings;

pub use accounts::{Services, User};
pub use compiler::{BuildOptimisation, Compiler, March};
pub use disk::{
    Disk, DiskPartitions, EfiPartition, Ext4Options, Subvolume, SwapConfig, ZfsCanmount,
    ZfsDataset, ZfsOptions,
};
pub(crate) use packages::PlannerManifest;
pub use packages::{ManifestPackage, Package};
pub use settings::{
    AudioStack, Bootloader, DiskLayout, DisplayStack, Encryption, Gpu, GpuVendor, JournalStorage,
    Libc, MakeOpts, Password, Power, Shell, Username,
};

pub const MB: u64 = 1024 * 1024;
pub const GB: u64 = 1024 * MB;

/// User-facing declarative system definition.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct SystemManifest {
    #[serde(default)]
    pub os: Os,
    #[serde(default)]
    pub disk: Disk,
    #[serde(default)]
    pub hardware: Hardware,
    #[serde(default)]
    pub kernel: Kernel,
    #[serde(default)]
    pub journal: Journal,
    #[serde(default)]
    pub environment: Vec<String>,
    #[serde(default)]
    pub packages: Vec<Package>,
    #[serde(default)]
    pub compiler: Compiler,
    #[serde(default)]
    pub init_system: InitSystem,
    #[serde(default)]
    pub bootloader: Option<Bootloader>,
    #[serde(default)]
    pub display_stack: Option<DisplayStack>,
    #[serde(default)]
    pub audio_stack: Option<AudioStack>,
    #[serde(default)]
    pub prefer_binary: bool,
    #[serde(default)]
    pub services: Services,
    #[serde(default)]
    pub users: Vec<User>,
}

pub type Oxys = SystemManifest;

impl SystemManifest {
    /// Boot manager to install, applying the default when the manifest leaves
    /// it unset.
    pub fn resolved_bootloader(&self) -> Bootloader {
        self.bootloader.unwrap_or_default()
    }

    /// Names of users whose password must be collected interactively at install
    /// time because they declared [`Password::Prompt`].
    pub fn prompt_users(&self) -> Vec<&str> {
        self.users
            .iter()
            .filter(|user| user.password == Password::Prompt)
            .map(|user| user.name.as_str())
            .collect()
    }

    /// Indices of users whose name must be collected interactively at install
    /// time because they declared [`Username::Prompt`]. Indices rather than
    /// names, since the name itself is exactly what's missing.
    pub fn prompt_usernames(&self) -> Vec<usize> {
        self.users
            .iter()
            .enumerate()
            .filter(|(_, user)| user.name == Username::Prompt)
            .map(|(index, _)| index)
            .collect()
    }

    /// Warnings about insecure password declarations, surfaced when the config
    /// is compiled so plaintext secrets don't slip into `manifest.toml`
    /// unnoticed.
    pub fn password_warnings(&self) -> Vec<String> {
        self.users
            .iter()
            .filter(|user| matches!(user.password, Password::Plain(_)))
            .map(|user| {
                format!(
                    "user '{}' uses Password::Plain — the plaintext is stored in manifest.toml; \
                     prefer Password::Hashed or Password::Prompt",
                    user.name.as_str()
                )
            })
            .collect()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Os {
    #[serde(default)]
    pub hostname: String,
    #[serde(default)]
    pub timezone: String,
    #[serde(default)]
    pub locale: String,
    #[serde(default)]
    pub shell: Shell,
    #[serde(default)]
    pub libc: Libc,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Hardware {
    #[serde(default)]
    pub gpu: Gpu,
    #[serde(default)]
    pub power: Power,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Kernel {
    #[serde(default)]
    pub cmdline: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Journal {
    #[serde(default)]
    pub storage: JournalStorage,
    #[serde(default)]
    pub max_use: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InitSystem {
    Systemd,
    Openrc,
}

impl Default for InitSystem {
    fn default() -> Self {
        Self::Openrc
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_march_has_no_binhost_url() {
        assert_eq!(March::Native.binhost_url(), None);
    }

    #[test]
    fn baseline_marches_map_to_gentoo_binhost_urls() {
        assert_eq!(
            March::X86_64.binhost_url().as_deref(),
            Some("https://distfiles.gentoo.org/releases/amd64/binpackages/23.0/x86-64/")
        );
        assert_eq!(
            March::X86_64V3.binhost_url().as_deref(),
            Some("https://distfiles.gentoo.org/releases/amd64/binpackages/23.0/x86-64-v3/")
        );
    }

    #[test]
    fn compiler_default_binhost_follows_default_march() {
        let compiler = Compiler::default();
        assert_eq!(compiler.binhost, compiler.march.binhost_url());
    }

    #[test]
    fn user_builder_populates_expected_fields() {
        let user = User::new("testuser")
            .wheel()
            .groups(["video", "audio"])
            .wheel()
            .shell(Shell::Zsh)
            .password(Password::Prompt);

        assert_eq!(user.name, Username::Literal("testuser".into()));
        assert_eq!(user.shell, Shell::Zsh);
        assert_eq!(user.password, Password::Prompt);
        // groups() replaces, then wheel() appends without duplicating.
        assert_eq!(user.groups, vec!["video", "audio", "wheel"]);
        assert!(user.is_wheel());
    }

    #[test]
    fn prompt_users_lists_only_prompt_passwords() {
        let manifest = SystemManifest {
            users: vec![
                User::new("root").password(Password::Hashed("$6$x".into())),
                User::new("testuser").password(Password::Prompt),
                User::new("guest").password(Password::None),
                User::new("dev").password(Password::Prompt),
            ],
            ..SystemManifest::default()
        };
        assert_eq!(manifest.prompt_users(), vec!["testuser", "dev"]);
    }

    #[test]
    fn prompt_usernames_lists_only_prompt_names() {
        let manifest = SystemManifest {
            users: vec![
                User::new("root"),
                User::prompt().password(Password::Prompt),
                User::new("guest"),
                User::prompt(),
            ],
            ..SystemManifest::default()
        };
        assert_eq!(manifest.prompt_usernames(), vec![1, 3]);
    }

    #[test]
    fn password_warnings_flag_plaintext_only() {
        let manifest = SystemManifest {
            users: vec![
                User::new("testuser").password(Password::Plain("hunter2".into())),
                User::new("root").password(Password::Hashed("$6$x".into())),
                User::new("bot").password(Password::Prompt),
            ],
            ..SystemManifest::default()
        };
        let warnings = manifest.password_warnings();
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("testuser"));
        assert!(warnings[0].contains("Password::Plain"));
    }

    #[test]
    fn prompt_password_serialises_without_a_secret() {
        let user = User::new("testuser").password(Password::Prompt);
        let toml = toml::to_string(&user).expect("serialise user");
        assert!(toml.contains("password = \"prompt\""), "got: {toml}");
        assert!(!toml.contains("hunter"));
    }

    #[test]
    fn prompt_username_round_trips_through_toml() {
        let user = User::prompt().password(Password::Prompt);
        let toml = toml::to_string(&user).expect("serialise user");
        assert!(toml.contains("name = \"prompt\""), "got: {toml}");

        let parsed: User = toml::from_str(&toml).expect("deserialise user");
        assert_eq!(parsed.name, Username::Prompt);
    }

    #[test]
    fn literal_username_round_trips_through_toml() {
        let user = User::new("testuser");
        let toml = toml::to_string(&user).expect("serialise user");

        let parsed: User = toml::from_str(&toml).expect("deserialise user");
        assert_eq!(parsed.name, Username::Literal("testuser".into()));
    }

    #[test]
    fn shell_paths_are_absolute() {
        assert_eq!(Shell::Bash.path(), "/bin/bash");
        assert_eq!(Shell::Zsh.path(), "/bin/zsh");
        assert_eq!(Shell::Fish.path(), "/usr/bin/fish");
    }

    #[test]
    fn package_builder_populates_expected_fields() {
        let package = Package::new("gui-wm/niri")
            .binary(true)
            .version("25.11-r1")
            .use_flags(["screencast", "-debug"]);

        assert_eq!(package.package, "gui-wm/niri");
        assert!(package.binary);
        assert_eq!(package.version.as_deref(), Some("25.11-r1"));
        assert_eq!(package.use_flags, vec!["screencast", "-debug"]);
    }

    #[test]
    fn converts_system_manifest_to_internal_manifest() {
        let manifest = PlannerManifest::from(SystemManifest {
            os: Os {
                libc: Libc::Glibc,
                ..Os::default()
            },
            packages: vec![Package::new("gui-wm/niri").version("25.11-r1")],
            ..SystemManifest::default()
        });

        assert_eq!(manifest.libc, Some(Libc::Glibc));
        assert_eq!(manifest.packages.len(), 1);
        assert_eq!(manifest.packages[0].version.as_deref(), Some("25.11-r1"));
    }

    #[test]
    fn disk_default_uses_ext4_whole_disk() {
        let disk = Disk::default();

        assert_eq!(disk.layout, DiskLayout::Ext4);
        assert_eq!(disk.encryption, Encryption::None);
        // ext4 whole-disk: single root partition, no separate /home.
        assert!(!disk.ext4.separate_home);
    }

    #[test]
    fn disk_encryption_deserializes_password_mode() {
        let manifest = toml::from_str::<SystemManifest>(
            r#"
                [disk]
                device = "/dev/nvme0n1"
                layout = "ext4"
                encryption = "password"
            "#,
        )
        .expect("manifest should parse");

        assert_eq!(manifest.disk.encryption, Encryption::Password);
    }

    #[test]
    fn efi_partition_default_uses_512mb() {
        assert_eq!(EfiPartition::default().size, 512 * MB);
    }

    #[test]
    fn gpu_serializes_single_vendor_as_string() {
        let manifest = SystemManifest {
            hardware: Hardware {
                gpu: Gpu::Single(GpuVendor::Amd),
                ..Hardware::default()
            },
            ..SystemManifest::default()
        };

        let toml = toml::to_string(&manifest).expect("manifest should serialize");

        assert!(toml.contains("gpu = \"amd\""));
    }

    #[test]
    fn gpu_serializes_hybrid_as_table() {
        let manifest = SystemManifest {
            hardware: Hardware {
                gpu: Gpu::Hybrid {
                    igpu: GpuVendor::Amd,
                    dgpu: GpuVendor::Nvidia,
                },
                ..Hardware::default()
            },
            ..SystemManifest::default()
        };

        let toml = toml::to_string(&manifest).expect("manifest should serialize");

        assert!(toml.contains("[hardware.gpu]"));
        assert!(toml.contains("igpu = \"amd\""));
        assert!(toml.contains("dgpu = \"nvidia\""));
    }

    #[test]
    fn gpu_deserializes_legacy_single_vendor_string() {
        let manifest = toml::from_str::<SystemManifest>(
            r#"
                [hardware]
                gpu = "nvidia"
            "#,
        )
        .expect("manifest should parse");

        assert_eq!(manifest.hardware.gpu, Gpu::Single(GpuVendor::Nvidia));
    }

    #[test]
    fn gpu_deserializes_hybrid_gpu_table() {
        let manifest = toml::from_str::<SystemManifest>(
            r#"
                [hardware.gpu]
                igpu = "amd"
                dgpu = "nvidia"
            "#,
        )
        .expect("manifest should parse");

        assert_eq!(
            manifest.hardware.gpu,
            Gpu::Hybrid {
                igpu: GpuVendor::Amd,
                dgpu: GpuVendor::Nvidia,
            }
        );
        assert!(manifest.hardware.gpu.prime_offloading_enabled());
    }
}
