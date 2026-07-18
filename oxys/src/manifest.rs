use serde::{Deserialize, Serialize};

mod accounts;
mod compiler;
mod disk;
mod packages;
mod settings;
mod swap;

pub use accounts::{OpenrcServices, Services, User};
pub use compiler::{BuildOptimisation, Compiler, March};
pub use disk::{
    Disk, DiskPartitions, EfiPartition, Ext4Options, Subvolume, SwapConfig, ZfsCanmount,
    ZfsDataset, ZfsOptions,
};
pub(crate) use packages::PlannerManifest;
pub use packages::{ManifestPackage, Package};
pub use settings::{
    AudioStack, Bootloader, Compositor, DesktopShell, DiskLayout, DisplayStack, Drm, DrmDriver,
    DrmDrivers, Encryption, Gpu, GpuVendor, Graphics, JournalStorage, Libc, LoginFrontend,
    MakeOpts, MesaGraphics, Nvidia, NvidiaDriver, Password, Power, PrimeMode, SeatBackend, Session,
    SessionMode, SessionTracker, SessionUser, Shell, SoftwareRenderer, Timezone, Username,
    VideoCard, VideoCards, VmGraphics,
};
pub use swap::{
    Compression, DEFAULT_SWAPPINESS, DISK_SWAP_PRIORITY, RamFraction, ResolvedDiskSwap,
    ResolvedSwap, ResolvedZram, Swap, SwapDiskOptions, SwapResolveError, SwapSize, SwapStrategy,
    ZRAM_SWAP_PRIORITY, ZramOptions, resolve_swap_for_ram,
};

pub const MB: u64 = 1024 * 1024;
pub const GB: u64 = 1024 * MB;
pub const MIB: u64 = MB;
pub const GIB: u64 = GB;

/// User-facing declarative system definition.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize)]
pub struct SystemManifest {
    #[serde(default)]
    pub os: Os,
    #[serde(default)]
    pub disk: Disk,
    #[serde(default)]
    pub swap: Swap,
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
    pub session: Session,
    #[serde(default)]
    pub prefer_binary: bool,
    #[serde(default)]
    pub services: Services,
    #[serde(default)]
    pub users: Vec<User>,
    /// Deserialization-only provenance for the retired `hardware.gpu` field.
    #[doc(hidden)]
    #[serde(skip)]
    pub legacy_gpu: Option<Gpu>,
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

    /// True when the timezone must be collected interactively at install time
    /// because the config declared [`Timezone::Prompt`].
    pub fn prompts_timezone(&self) -> bool {
        self.os.timezone == Timezone::Prompt
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
    pub timezone: Timezone,
    #[serde(default)]
    pub locale: String,
    #[serde(default)]
    pub shell: Shell,
    #[serde(default)]
    pub libc: Libc,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize)]
pub struct Hardware {
    #[serde(default)]
    pub graphics: Graphics,
    #[serde(default)]
    pub power: Power,
}

#[derive(Deserialize, Default)]
struct HardwareCompat {
    #[serde(default)]
    graphics: Option<Graphics>,
    #[serde(default)]
    gpu: Option<Gpu>,
    #[serde(default)]
    power: Power,
}

impl HardwareCompat {
    fn resolve<E: serde::de::Error>(self) -> Result<(Hardware, Option<Gpu>), E> {
        if self.graphics.is_some() && self.gpu.is_some() {
            return Err(E::custom(
                "hardware.graphics and retired hardware.gpu cannot both be set",
            ));
        }
        let legacy_gpu = self.gpu;
        let graphics = self
            .graphics
            .unwrap_or_else(|| legacy_gpu.clone().map(Into::into).unwrap_or_default());
        Ok((
            Hardware {
                graphics,
                power: self.power,
            },
            legacy_gpu,
        ))
    }
}

impl<'de> Deserialize<'de> for Hardware {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = HardwareCompat::deserialize(deserializer)?;
        value.resolve().map(|(hardware, _)| hardware)
    }
}

impl<'de> Deserialize<'de> for SystemManifest {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize, Default)]
        struct SystemManifestCompat {
            #[serde(default)]
            os: Os,
            #[serde(default)]
            disk: Disk,
            #[serde(default)]
            swap: Swap,
            #[serde(default)]
            hardware: HardwareCompat,
            #[serde(default)]
            kernel: Kernel,
            #[serde(default)]
            journal: Journal,
            #[serde(default)]
            environment: Vec<String>,
            #[serde(default)]
            packages: Vec<Package>,
            #[serde(default)]
            compiler: Compiler,
            #[serde(default)]
            init_system: InitSystem,
            #[serde(default)]
            bootloader: Option<Bootloader>,
            #[serde(default)]
            display_stack: Option<DisplayStack>,
            #[serde(default)]
            audio_stack: Option<AudioStack>,
            #[serde(default)]
            session: Session,
            #[serde(default)]
            prefer_binary: bool,
            #[serde(default)]
            services: Services,
            #[serde(default)]
            users: Vec<User>,
        }

        let value = SystemManifestCompat::deserialize(deserializer)?;
        let (hardware, legacy_gpu) = value.hardware.resolve()?;
        Ok(Self {
            os: value.os,
            disk: value.disk,
            swap: value.swap,
            hardware,
            kernel: value.kernel,
            journal: value.journal,
            environment: value.environment,
            packages: value.packages,
            compiler: value.compiler,
            init_system: value.init_system,
            bootloader: value.bootloader,
            display_stack: value.display_stack,
            audio_stack: value.audio_stack,
            session: value.session,
            prefer_binary: value.prefer_binary,
            services: value.services,
            users: value.users,
            legacy_gpu,
        })
    }
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
mod tests;
