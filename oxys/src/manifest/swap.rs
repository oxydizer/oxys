use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::detect::detect_ram;

use super::{GB, Package, SystemManifest};

pub const DEFAULT_SWAPPINESS: u16 = 180;
pub const ZRAM_SWAP_PRIORITY: i16 = 100;
pub const DISK_SWAP_PRIORITY: i16 = 10;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Swap {
    #[serde(default)]
    pub strategy: SwapStrategy,
    #[serde(default = "default_swappiness")]
    pub swappiness: u16,
}

impl Default for Swap {
    fn default() -> Self {
        Self {
            strategy: SwapStrategy::Disabled,
            swappiness: DEFAULT_SWAPPINESS,
        }
    }
}

fn default_swappiness() -> u16 {
    DEFAULT_SWAPPINESS
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SwapStrategy {
    Disk {
        size: SwapSize,
    },
    Hybrid {
        zram: ZramOptions,
        disk: SwapDiskOptions,
    },
    ZramOnly {
        algorithm: Compression,
        fraction: RamFraction,
    },
    Disabled,
}

impl Default for SwapStrategy {
    fn default() -> Self {
        Self::Disabled
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SwapSize {
    MatchRam,
    Fixed(u64),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SwapDiskOptions {
    pub size: SwapSize,
}

impl Default for SwapDiskOptions {
    fn default() -> Self {
        Self {
            size: SwapSize::MatchRam,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ZramOptions {
    #[serde(default)]
    pub algorithm: Compression,
    #[serde(default)]
    pub fraction: RamFraction,
}

impl Default for ZramOptions {
    fn default() -> Self {
        Self {
            algorithm: Compression::Zstd,
            fraction: RamFraction::HALF,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Compression {
    Zstd,
    LzoRle,
    Lz4,
}

impl Compression {
    pub fn kernel_name(self) -> &'static str {
        match self {
            Self::Zstd => "zstd",
            Self::LzoRle => "lzo-rle",
            Self::Lz4 => "lz4",
        }
    }
}

impl Default for Compression {
    fn default() -> Self {
        Self::Zstd
    }
}

/// An exact fraction of system RAM. Rational values keep the manifest fully
/// comparable and avoid NaN and rounding surprises from floating point.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct RamFraction {
    pub numerator: u32,
    pub denominator: u32,
}

impl RamFraction {
    pub const HALF: Self = Self::new(1, 2);

    pub const fn new(numerator: u32, denominator: u32) -> Self {
        Self {
            numerator,
            denominator,
        }
    }

    fn bytes(self, ram_bytes: u64) -> Result<u64, SwapResolveError> {
        if self.numerator == 0 || self.denominator == 0 {
            return Err(SwapResolveError::InvalidFraction {
                numerator: self.numerator,
                denominator: self.denominator,
            });
        }
        ram_bytes
            .checked_mul(u64::from(self.numerator))
            .map(|bytes| bytes / u64::from(self.denominator))
            .filter(|bytes| *bytes > 0)
            .ok_or(SwapResolveError::SizeOverflow)
    }
}

impl Default for RamFraction {
    fn default() -> Self {
        Self::HALF
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedSwap {
    pub zram: Option<ResolvedZram>,
    pub disk: Option<ResolvedDiskSwap>,
    pub swappiness: u16,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedZram {
    pub size: u64,
    pub algorithm: Compression,
    pub priority: i16,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedDiskSwap {
    pub size: u64,
    pub priority: i16,
}

impl ResolvedSwap {
    pub fn materialize_manifest(&self, manifest: &SystemManifest) -> SystemManifest {
        let mut result = manifest.clone();
        if self.zram.is_some()
            && !result
                .packages
                .iter()
                .any(|package| package.package == "sys-block/zram-init")
        {
            result.packages.push(Package::new("sys-block/zram-init"));
        }
        result
    }
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum SwapResolveError {
    #[error("vm.swappiness must be between 0 and 200 (got {0})")]
    InvalidSwappiness(u16),
    #[error(
        "RAM fraction must have non-zero numerator and denominator (got {numerator}/{denominator})"
    )]
    InvalidFraction { numerator: u32, denominator: u32 },
    #[error("swap size must be greater than zero")]
    ZeroSize,
    #[error("swap size overflowed u64")]
    SizeOverflow,
    #[error(
        "legacy swapfile configuration is not supported; choose disk partition, hybrid, zram-only, or disabled"
    )]
    LegacySwapFile,
    #[error("top-level swap policy and legacy disk.partitions.swap cannot both be configured")]
    ConflictingLegacyPolicy,
}

pub fn resolve_swap_for_ram(swap: &Swap, ram_bytes: u64) -> Result<ResolvedSwap, SwapResolveError> {
    if swap.swappiness > 200 {
        return Err(SwapResolveError::InvalidSwappiness(swap.swappiness));
    }
    if ram_bytes == 0 {
        return Err(SwapResolveError::ZeroSize);
    }

    let disk = |size: &SwapSize| {
        Ok(ResolvedDiskSwap {
            size: resolve_size(size, ram_bytes)?,
            priority: DISK_SWAP_PRIORITY,
        })
    };
    let zram = |options: &ZramOptions| {
        Ok(ResolvedZram {
            size: options.fraction.bytes(ram_bytes)?,
            algorithm: options.algorithm,
            priority: ZRAM_SWAP_PRIORITY,
        })
    };

    let (zram, disk) = match &swap.strategy {
        SwapStrategy::Disk { size } => (None, Some(disk(size)?)),
        SwapStrategy::Hybrid { zram: z, disk: d } => (Some(zram(z)?), Some(disk(&d.size)?)),
        SwapStrategy::ZramOnly {
            algorithm,
            fraction,
        } => (
            Some(zram(&ZramOptions {
                algorithm: *algorithm,
                fraction: *fraction,
            })?),
            None,
        ),
        SwapStrategy::Disabled => (None, None),
    };

    Ok(ResolvedSwap {
        zram,
        disk,
        swappiness: swap.swappiness,
    })
}

fn resolve_size(size: &SwapSize, ram_bytes: u64) -> Result<u64, SwapResolveError> {
    match size {
        SwapSize::MatchRam => Ok(ram_bytes),
        SwapSize::Fixed(0) => Err(SwapResolveError::ZeroSize),
        SwapSize::Fixed(bytes) => Ok(*bytes),
    }
}

impl SystemManifest {
    pub fn resolved_swap(&self) -> Result<ResolvedSwap, SwapResolveError> {
        self.resolved_swap_for_ram(detect_ram().unwrap_or(8 * GB))
    }

    pub fn resolved_swap_for_ram(&self, ram_bytes: u64) -> Result<ResolvedSwap, SwapResolveError> {
        use super::SwapConfig;

        if !self.disk.partitions.swap.is_unspecified() && self.swap != Swap::default() {
            return Err(SwapResolveError::ConflictingLegacyPolicy);
        }

        match self.disk.partitions.swap {
            SwapConfig::Unspecified => resolve_swap_for_ram(&self.swap, ram_bytes),
            SwapConfig::Partition { size } => resolve_swap_for_ram(
                &Swap {
                    strategy: SwapStrategy::Disk {
                        size: SwapSize::Fixed(size),
                    },
                    swappiness: self.swap.swappiness,
                },
                ram_bytes,
            ),
            SwapConfig::Zram { size } => {
                if self.swap.swappiness > 200 {
                    return Err(SwapResolveError::InvalidSwappiness(self.swap.swappiness));
                }
                if size == 0 {
                    return Err(SwapResolveError::ZeroSize);
                }
                Ok(ResolvedSwap {
                    zram: Some(ResolvedZram {
                        size,
                        algorithm: Compression::Zstd,
                        priority: ZRAM_SWAP_PRIORITY,
                    }),
                    disk: None,
                    swappiness: self.swap.swappiness,
                })
            }
            SwapConfig::None => resolve_swap_for_ram(
                &Swap {
                    strategy: SwapStrategy::Disabled,
                    swappiness: self.swap.swappiness,
                },
                ram_bytes,
            ),
            SwapConfig::File { .. } => Err(SwapResolveError::LegacySwapFile),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn policy_for(gib: u64) -> SwapStrategy {
        match gib {
            0..=8 => SwapStrategy::Disk {
                size: SwapSize::MatchRam,
            },
            9..=16 => SwapStrategy::Hybrid {
                zram: ZramOptions::default(),
                disk: SwapDiskOptions {
                    size: SwapSize::Fixed(4 * GB),
                },
            },
            _ => SwapStrategy::ZramOnly {
                algorithm: Compression::Zstd,
                fraction: RamFraction::HALF,
            },
        }
    }

    #[test]
    fn policy_has_expected_ram_boundaries() {
        for (gib, want_disk, want_zram) in [
            (8, true, false),
            (9, true, true),
            (16, true, true),
            (17, false, true),
        ] {
            let resolved = resolve_swap_for_ram(
                &Swap {
                    strategy: policy_for(gib),
                    swappiness: 180,
                },
                gib * GB,
            )
            .unwrap();
            assert_eq!(resolved.disk.is_some(), want_disk, "{gib} GiB");
            assert_eq!(resolved.zram.is_some(), want_zram, "{gib} GiB");
        }
    }

    #[test]
    fn hybrid_uses_half_ram_and_prefers_zram() {
        let resolved = resolve_swap_for_ram(
            &Swap {
                strategy: policy_for(16),
                swappiness: 180,
            },
            16 * GB,
        )
        .unwrap();
        assert_eq!(resolved.zram.as_ref().unwrap().size, 8 * GB);
        assert_eq!(resolved.disk.as_ref().unwrap().size, 4 * GB);
        assert!(
            resolved.zram.as_ref().unwrap().priority > resolved.disk.as_ref().unwrap().priority
        );
    }

    #[test]
    fn rejects_invalid_values() {
        assert!(matches!(
            resolve_swap_for_ram(
                &Swap {
                    strategy: SwapStrategy::Disabled,
                    swappiness: 201,
                },
                8 * GB,
            ),
            Err(SwapResolveError::InvalidSwappiness(201))
        ));
        assert!(matches!(
            RamFraction::new(1, 0).bytes(8 * GB),
            Err(SwapResolveError::InvalidFraction { .. })
        ));
    }

    #[test]
    fn hybrid_policy_round_trips_through_toml() {
        let swap = Swap {
            strategy: policy_for(16),
            swappiness: 180,
        };
        let encoded = toml::to_string(&swap).unwrap();
        let decoded: Swap = toml::from_str(&encoded).unwrap();
        assert_eq!(decoded, swap);
    }

    #[test]
    fn legacy_partition_manifest_resolves_to_disk_swap() {
        let manifest: SystemManifest =
            toml::from_str("[disk.partitions.swap.partition]\nsize = 4294967296\n").unwrap();
        let resolved = manifest.resolved_swap_for_ram(16 * GB).unwrap();
        assert_eq!(resolved.disk.unwrap().size, 4 * GB);
        assert!(resolved.zram.is_none());
    }
}
