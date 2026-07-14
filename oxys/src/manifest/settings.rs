use serde::{
    de::{self, MapAccess, Visitor},
    Deserialize, Deserializer, Serialize, Serializer,
};
use std::fmt;

/// Boot manager written to the ESP during install.
///
/// This is independent of [`super::InitSystem`]: any combination is valid (for
/// example OpenRC with systemd-boot, or systemd with GRUB).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Bootloader {
    SystemdBoot,
    Grub,
}

impl Default for Bootloader {
    fn default() -> Self {
        Self::Grub
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Libc {
    Glibc,
}

impl Default for Libc {
    fn default() -> Self {
        Self::Glibc
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DisplayStack {
    Wayland,
    X11,
}

impl Default for DisplayStack {
    fn default() -> Self {
        Self::Wayland
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AudioStack {
    Pipewire,
    Pulseaudio,
}

impl Default for AudioStack {
    fn default() -> Self {
        Self::Pipewire
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Shell {
    Bash,
    Zsh,
    Fish,
}

impl Default for Shell {
    fn default() -> Self {
        Self::Bash
    }
}

impl Shell {
    /// Absolute path to the login shell binary on the installed target.
    pub fn path(&self) -> &'static str {
        match self {
            Shell::Bash => "/bin/bash",
            Shell::Zsh => "/bin/zsh",
            Shell::Fish => "/usr/bin/fish",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DiskLayout {
    Btrfs,
    LuksBtrfs,
    Zfs,
    Ext4,
}

impl Default for DiskLayout {
    fn default() -> Self {
        Self::Ext4
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Encryption {
    /// TPM-backed unlock. Planned, not provisioned yet.
    Tpm,
    /// Passphrase-backed LUKS unlock. Planned, not provisioned yet.
    Password,
    /// No disk encryption.
    None,
}

impl Default for Encryption {
    fn default() -> Self {
        Self::None
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GpuVendor {
    Amd,
    Intel,
    Nvidia,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Gpu {
    Auto,
    Single(GpuVendor),
    Hybrid { igpu: GpuVendor, dgpu: GpuVendor },
}

impl Gpu {
    pub fn prime_offloading_enabled(&self) -> bool {
        matches!(self, Self::Hybrid { .. })
    }
}

impl Default for Gpu {
    fn default() -> Self {
        Self::Auto
    }
}

impl Serialize for Gpu {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match self {
            Self::Auto => serializer.serialize_str("auto"),
            Self::Single(vendor) => vendor.serialize(serializer),
            Self::Hybrid { igpu, dgpu } => {
                use serde::ser::SerializeMap;

                let mut map = serializer.serialize_map(Some(2))?;
                map.serialize_entry("igpu", igpu)?;
                map.serialize_entry("dgpu", dgpu)?;
                map.end()
            }
        }
    }
}

impl<'de> Deserialize<'de> for Gpu {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct GpuVisitor;

        impl<'de> Visitor<'de> for GpuVisitor {
            type Value = Gpu;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("a GPU vendor string or a hybrid GPU table")
            }

            fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
            where
                E: de::Error,
            {
                match value {
                    "auto" => Ok(Gpu::Auto),
                    "amd" => Ok(Gpu::Single(GpuVendor::Amd)),
                    "intel" => Ok(Gpu::Single(GpuVendor::Intel)),
                    "nvidia" => Ok(Gpu::Single(GpuVendor::Nvidia)),
                    _ => Err(E::unknown_variant(
                        value,
                        &["auto", "amd", "intel", "nvidia"],
                    )),
                }
            }

            fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
            where
                A: MapAccess<'de>,
            {
                let mut igpu = None;
                let mut dgpu = None;

                while let Some(key) = map.next_key::<String>()? {
                    match key.as_str() {
                        "igpu" => {
                            if igpu.is_some() {
                                return Err(de::Error::duplicate_field("igpu"));
                            }
                            igpu = Some(map.next_value()?);
                        }
                        "dgpu" => {
                            if dgpu.is_some() {
                                return Err(de::Error::duplicate_field("dgpu"));
                            }
                            dgpu = Some(map.next_value()?);
                        }
                        _ => return Err(de::Error::unknown_field(&key, &["igpu", "dgpu"])),
                    }
                }

                let igpu = igpu.ok_or_else(|| de::Error::missing_field("igpu"))?;
                let dgpu = dgpu.ok_or_else(|| de::Error::missing_field("dgpu"))?;

                if igpu == dgpu {
                    return Err(de::Error::custom(
                        "hybrid GPU config requires two different vendors",
                    ));
                }

                Ok(Gpu::Hybrid { igpu, dgpu })
            }
        }

        deserializer.deserialize_any(GpuVisitor)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MakeOpts {
    Auto,
    Jobs(usize),
}

impl Default for MakeOpts {
    fn default() -> Self {
        Self::Auto
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Power {
    Auto,
    None,
    Tlp,
    AsusCtl,
}

impl Default for Power {
    fn default() -> Self {
        Self::Auto
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JournalStorage {
    Auto,
    Persistent,
    Volatile,
}

impl Default for JournalStorage {
    fn default() -> Self {
        Self::Persistent
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Password {
    /// No password set; the account is created locked.
    None,
    /// Plaintext password baked into the config. Convenient but the value is
    /// stored verbatim in `manifest.toml`, so a compile-time warning is
    /// emitted. Prefer [`Password::Hashed`] or [`Password::Prompt`].
    Plain(String),
    /// A pre-hashed password (as produced by e.g. `openssl passwd -6`) baked
    /// into the config. Applied with `chpasswd -e`.
    Hashed(String),
    /// Collected interactively by the installer at install time. The secret is
    /// never written to the config or to `manifest.toml`.
    Prompt,
}

impl Default for Password {
    fn default() -> Self {
        Self::None
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Username {
    /// A fixed name baked into the config.
    Literal(String),
    /// Collected interactively by the installer at install time.
    Prompt,
}

impl Username {
    /// The literal name, or an empty string for a still-unresolved
    /// [`Username::Prompt`]. Install-time code may assume this is never
    /// reached for `Prompt` because `plan_system_install` refuses to build a
    /// plan while any user's name is unresolved.
    pub fn as_str(&self) -> &str {
        match self {
            Self::Literal(name) => name.as_str(),
            Self::Prompt => "",
        }
    }
}

impl Default for Username {
    fn default() -> Self {
        Self::Literal(String::new())
    }
}
