use serde::{Deserialize, Serialize};

use crate::detect::default_swap;

use super::{DiskLayout, Encryption, GB, MB};

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Disk {
    #[serde(default)]
    pub device: String,
    #[serde(default)]
    pub layout: DiskLayout,
    #[serde(default)]
    pub encryption: Encryption,
    #[serde(default)]
    pub subvolumes: Vec<Subvolume>,
    #[serde(default)]
    pub partitions: DiskPartitions,
    #[serde(default)]
    pub snapshots: bool,
    #[serde(default)]
    pub zfs: ZfsOptions,
    #[serde(default)]
    pub ext4: Ext4Options,
}

// Hand-written so a generated manifest only carries the sub-config that belongs
// to the chosen layout. The derived impl serialized `subvolumes` (btrfs), the
// whole `[disk.zfs]` table, and `[disk.ext4]` unconditionally, so an ext4
// install produced a manifest.toml full of dead ZFS datasets and btrfs
// subvolumes -- confusing to read and easy to mistake for the layout still
// being active. Every field is `#[serde(default)]` on the read side, so the
// omitted ones round-trip back to their defaults. (toml groups scalar keys
// ahead of tables itself, so field order here doesn't affect the output.)
impl Serialize for Disk {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeStruct;

        let want_subvolumes = matches!(self.layout, DiskLayout::Btrfs | DiskLayout::LuksBtrfs);
        let want_zfs = matches!(self.layout, DiskLayout::Zfs);
        let want_ext4 = matches!(self.layout, DiskLayout::Ext4);

        let len = 5 + usize::from(want_subvolumes) + usize::from(want_zfs) + usize::from(want_ext4);
        let mut disk = serializer.serialize_struct("Disk", len)?;
        disk.serialize_field("device", &self.device)?;
        disk.serialize_field("layout", &self.layout)?;
        disk.serialize_field("encryption", &self.encryption)?;
        disk.serialize_field("snapshots", &self.snapshots)?;
        if want_subvolumes {
            disk.serialize_field("subvolumes", &self.subvolumes)?;
        }
        disk.serialize_field("partitions", &self.partitions)?;
        if want_zfs {
            disk.serialize_field("zfs", &self.zfs)?;
        }
        if want_ext4 {
            disk.serialize_field("ext4", &self.ext4)?;
        }
        disk.end()
    }
}

impl Default for Disk {
    fn default() -> Self {
        Self {
            device: String::new(),
            layout: DiskLayout::Ext4,
            encryption: Encryption::None,
            subvolumes: vec![
                Subvolume {
                    name: "@".to_owned(),
                    mount: "/".to_owned(),
                },
                Subvolume {
                    name: "@home".to_owned(),
                    mount: "/home".to_owned(),
                },
                Subvolume {
                    name: "@snapshots".to_owned(),
                    mount: "/.snapshots".to_owned(),
                },
                Subvolume {
                    name: "@log".to_owned(),
                    mount: "/var/log".to_owned(),
                },
                Subvolume {
                    name: "@pkg".to_owned(),
                    mount: "/var/cache/portage".to_owned(),
                },
            ],
            partitions: DiskPartitions::default(),
            snapshots: true,
            zfs: ZfsOptions::default(),
            ext4: Ext4Options::default(),
        }
    }
}

impl Disk {
    /// Creates a default ext4 whole-disk layout targeting the given device.
    /// RAM is auto-detected for Zram sizing.
    pub fn with_device(device: impl Into<String>) -> Self {
        Self {
            device: device.into(),
            ..Self::default()
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Subvolume {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub mount: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EfiPartition {
    #[serde(default)]
    pub size: u64,
    #[serde(default)]
    pub mount: String,
}

impl Default for EfiPartition {
    fn default() -> Self {
        Self {
            size: 512 * MB,
            mount: "/boot/efi".to_owned(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SwapConfig {
    /// Dedicated swap partition of given size in bytes
    Partition { size: u64 },
    /// Swapfile of given size in bytes.
    /// Note: Btrfs swapfiles require no-COW (chattr +C) — oxys apply handles this automatically.
    File { size: u64 },
    /// Compressed RAM swap — recommended default for laptops.
    /// size is the zram device size in bytes, typically RAM/2.
    Zram { size: u64 },
    /// No swap
    None,
}

impl Default for SwapConfig {
    fn default() -> Self {
        default_swap()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiskPartitions {
    #[serde(default)]
    pub efi: EfiPartition,
    #[serde(default)]
    pub swap: SwapConfig,
}

impl Default for DiskPartitions {
    fn default() -> Self {
        Self {
            efi: EfiPartition::default(),
            swap: default_swap(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ZfsOptions {
    #[serde(default = "default_zfs_pool")]
    pub pool: String,
    #[serde(default = "default_zfs_boot_pool")]
    pub boot_pool: String,
    #[serde(default = "default_zfs_boot_pool_size")]
    pub boot_pool_size: u64,
    #[serde(default = "default_zfs_ashift")]
    pub ashift: u8,
    #[serde(default = "default_zfs_compression")]
    pub compression: String,
    #[serde(default = "default_zfs_boot_compression")]
    pub boot_compression: String,
    #[serde(default = "default_zfs_datasets")]
    pub datasets: Vec<ZfsDataset>,
}

impl Default for ZfsOptions {
    fn default() -> Self {
        Self {
            pool: default_zfs_pool(),
            boot_pool: default_zfs_boot_pool(),
            boot_pool_size: default_zfs_boot_pool_size(),
            ashift: default_zfs_ashift(),
            compression: default_zfs_compression(),
            boot_compression: default_zfs_boot_compression(),
            datasets: default_zfs_datasets(),
        }
    }
}

fn default_zfs_pool() -> String {
    "rpool".to_owned()
}

fn default_zfs_boot_pool() -> String {
    "bpool".to_owned()
}

fn default_zfs_boot_pool_size() -> u64 {
    2 * GB
}

fn default_zfs_ashift() -> u8 {
    12
}

fn default_zfs_compression() -> String {
    "zstd".to_owned()
}

fn default_zfs_boot_compression() -> String {
    "lz4".to_owned()
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ZfsCanmount {
    On,
    Off,
    Noauto,
}

impl Default for ZfsCanmount {
    fn default() -> Self {
        Self::On
    }
}

impl ZfsCanmount {
    pub fn as_zfs_value(&self) -> &'static str {
        match self {
            Self::On => "on",
            Self::Off => "off",
            Self::Noauto => "noauto",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ZfsDataset {
    #[serde(default)]
    pub pool: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub mount: String,
    #[serde(default)]
    pub canmount: ZfsCanmount,
}

fn default_zfs_datasets() -> Vec<ZfsDataset> {
    vec![
        ZfsDataset {
            pool: "rpool".to_owned(),
            name: "ROOT".to_owned(),
            mount: "none".to_owned(),
            canmount: ZfsCanmount::Off,
        },
        ZfsDataset {
            pool: "rpool".to_owned(),
            name: "ROOT/os".to_owned(),
            mount: "/".to_owned(),
            canmount: ZfsCanmount::Noauto,
        },
        ZfsDataset {
            pool: "bpool".to_owned(),
            name: "BOOT".to_owned(),
            mount: "none".to_owned(),
            canmount: ZfsCanmount::Off,
        },
        ZfsDataset {
            pool: "bpool".to_owned(),
            name: "BOOT/os".to_owned(),
            mount: "/boot".to_owned(),
            canmount: ZfsCanmount::On,
        },
        ZfsDataset {
            pool: "rpool".to_owned(),
            name: "home".to_owned(),
            mount: "/home".to_owned(),
            canmount: ZfsCanmount::On,
        },
        ZfsDataset {
            pool: "rpool".to_owned(),
            name: "var".to_owned(),
            mount: "/var".to_owned(),
            canmount: ZfsCanmount::Off,
        },
        ZfsDataset {
            pool: "rpool".to_owned(),
            name: "var/log".to_owned(),
            mount: "/var/log".to_owned(),
            canmount: ZfsCanmount::On,
        },
        ZfsDataset {
            pool: "rpool".to_owned(),
            name: "var/cache".to_owned(),
            mount: "/var/cache".to_owned(),
            canmount: ZfsCanmount::On,
        },
        ZfsDataset {
            pool: "rpool".to_owned(),
            name: "gentoo".to_owned(),
            mount: "none".to_owned(),
            canmount: ZfsCanmount::Off,
        },
        ZfsDataset {
            pool: "rpool".to_owned(),
            name: "gentoo/repos".to_owned(),
            mount: "/var/db/repos".to_owned(),
            canmount: ZfsCanmount::On,
        },
        ZfsDataset {
            pool: "rpool".to_owned(),
            name: "gentoo/distfiles".to_owned(),
            mount: "/var/cache/distfiles".to_owned(),
            canmount: ZfsCanmount::On,
        },
    ]
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Ext4Options {
    #[serde(default = "default_ext4_separate_home")]
    pub separate_home: bool,
    #[serde(default = "default_ext4_root_size")]
    pub root_size: u64,
}

impl Default for Ext4Options {
    fn default() -> Self {
        Self {
            separate_home: default_ext4_separate_home(),
            root_size: default_ext4_root_size(),
        }
    }
}

fn default_ext4_separate_home() -> bool {
    false
}

fn default_ext4_root_size() -> u64 {
    50 * GB
}
