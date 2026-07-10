use std::fs;
use std::path::Path;

use crate::manifest::{Gpu, GpuVendor, SwapConfig, GB};

const DRM_PATH: &str = "/sys/class/drm";
const POWER_SUPPLY_PATH: &str = "/sys/class/power_supply";
const BOARD_VENDOR_PATH: &str = "/sys/class/dmi/id/board_vendor";
const CPU_PRESENT_PATH: &str = "/sys/devices/system/cpu/present";
const SYS_BLOCK_PATH: &str = "/sys/block";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DetectedDisk {
    pub device: String,
    pub model: String,
    pub size: u64,
}

pub fn detect_gpu() -> Gpu {
    detect_gpu_from_drm_vendors().unwrap_or_else(detect_gpu_from_drm_entries)
}

pub fn detect_igpu() -> Option<GpuVendor> {
    match detect_gpu() {
        Gpu::Hybrid { igpu, .. } => Some(igpu),
        Gpu::Single(vendor) => Some(vendor),
        Gpu::Auto => None,
    }
}

pub fn detect_dgpu() -> Option<GpuVendor> {
    match detect_gpu() {
        Gpu::Hybrid { dgpu, .. } => Some(dgpu),
        _ => None,
    }
}

pub fn is_laptop() -> bool {
    fs::read_dir(POWER_SUPPLY_PATH)
        .map(|entries| {
            entries
                .filter_map(Result::ok)
                .any(|entry| entry.file_name().to_string_lossy().starts_with("BAT"))
        })
        .unwrap_or(false)
}

pub fn is_vendor(vendor: &str) -> bool {
    let vendor = vendor.trim().to_lowercase();
    if vendor.is_empty() {
        return false;
    }

    fs::read_to_string(BOARD_VENDOR_PATH)
        .map(|detected| vendor_matches(&detected, &vendor))
        .unwrap_or(false)
}

fn vendor_matches(detected: &str, vendor: &str) -> bool {
    detected.trim().to_lowercase().contains(vendor)
}

/// Reads total RAM from /proc/meminfo and returns it in bytes.
/// Returns None if /proc/meminfo cannot be read or parsed.
pub fn detect_ram() -> Option<u64> {
    let meminfo = fs::read_to_string("/proc/meminfo").ok()?;
    let line = meminfo.lines().find(|line| line.starts_with("MemTotal:"))?;
    let mut parts = line.split_whitespace();
    let _label = parts.next()?;
    let kb = parts.next()?.parse::<u64>().ok()?;

    kb.checked_mul(1024)
}

/// Returns the recommended swap config.
/// Defaults to Zram sized at half of detected RAM.
/// Falls back to Zram { size: 4GB } if RAM detection fails.
pub fn default_swap() -> SwapConfig {
    let ram = detect_ram().unwrap_or(8 * GB);
    SwapConfig::Zram { size: ram / 2 }
}

/// Returns the number of logical CPUs available.
/// Reads /sys/devices/system/cpu/present and counts the range.
/// Falls back to 1 on any parse failure.
pub fn detect_cpu_count() -> usize {
    fs::read_to_string(CPU_PRESENT_PATH)
        .ok()
        .and_then(|present| parse_cpu_present(&present))
        .unwrap_or(1)
}

pub fn detect_disks() -> Vec<DetectedDisk> {
    let entries = match fs::read_dir(SYS_BLOCK_PATH) {
        Ok(entries) => entries,
        Err(_) => return Vec::new(),
    };

    let mut disks = Vec::new();
    for entry in entries.filter_map(Result::ok) {
        let name = entry.file_name().to_string_lossy().to_string();
        if !is_whole_disk_name(&name) {
            continue;
        }

        let path = entry.path();
        let removable = fs::read_to_string(path.join("removable"))
            .map(|value| value.trim() == "1")
            .unwrap_or(false);
        let kind = fs::read_to_string(path.join("queue/rotational"))
            .map(|value| if value.trim() == "1" { "HDD" } else { "SSD" })
            .unwrap_or("disk");
        let model = fs::read_to_string(path.join("device/model"))
            .or_else(|_| fs::read_to_string(path.join("device/name")))
            .map(|value| value.trim().to_owned())
            .unwrap_or_else(|_| {
                if removable {
                    "removable".to_owned()
                } else {
                    kind.to_owned()
                }
            });
        let size = fs::read_to_string(path.join("size"))
            .ok()
            .and_then(|sectors| sectors.trim().parse::<u64>().ok())
            .and_then(|sectors| sectors.checked_mul(512))
            .unwrap_or(0);

        disks.push(DetectedDisk {
            device: format!("/dev/{name}"),
            model,
            size,
        });
    }

    disks.sort_by(|left, right| left.device.cmp(&right.device));
    disks
}

fn detect_gpu_from_drm_vendors() -> Option<Gpu> {
    let vendors = read_drm_vendors(Path::new(DRM_PATH));
    classify_gpu_vendors(&vendors)
}

fn detect_gpu_from_drm_entries() -> Gpu {
    let entries = match fs::read_dir(Path::new(DRM_PATH)) {
        Ok(entries) => entries,
        Err(_) => return Gpu::Auto,
    };

    let mut detected = Vec::new();
    for entry in entries.filter_map(Result::ok) {
        let name = entry.file_name().to_string_lossy().to_string();
        if !name.starts_with("card") || name.contains('-') {
            continue;
        }

        let uevent_path = entry.path().join("device/uevent");
        let Ok(uevent) = fs::read_to_string(uevent_path) else {
            continue;
        };

        if let Some(vendor) = parse_drm_driver(&uevent) {
            detected.push(vendor);
        }
    }

    classify_gpu_vendors(&detected).unwrap_or(Gpu::Auto)
}

fn parse_drm_driver(uevent: &str) -> Option<GpuVendor> {
    let driver = uevent
        .lines()
        .find_map(|line| line.strip_prefix("DRIVER="))?
        .trim();

    match driver {
        "amdgpu" => Some(GpuVendor::Amd),
        "i915" | "xe" => Some(GpuVendor::Intel),
        "nvidia" | "nvidia_drm" => Some(GpuVendor::Nvidia),
        _ => None,
    }
}

fn read_drm_vendors(drm_path: &Path) -> Vec<GpuVendor> {
    let entries = match fs::read_dir(drm_path) {
        Ok(entries) => entries,
        Err(_) => return Vec::new(),
    };

    let mut detected = Vec::new();
    for entry in entries.filter_map(Result::ok) {
        let name = entry.file_name().to_string_lossy().to_string();
        if !name.starts_with("card") || name.contains('-') {
            continue;
        }

        let vendor_path = entry.path().join("device/vendor");
        let Ok(vendor) = fs::read_to_string(vendor_path) else {
            continue;
        };

        if let Some(vendor) = parse_vendor_id(vendor.trim()) {
            detected.push(vendor);
        }
    }

    detected
}

fn parse_vendor_id(vendor: &str) -> Option<GpuVendor> {
    match vendor {
        "0x1002" => Some(GpuVendor::Amd),
        "0x8086" => Some(GpuVendor::Intel),
        "0x10de" => Some(GpuVendor::Nvidia),
        _ => None,
    }
}

fn classify_gpu_vendors(vendors: &[GpuVendor]) -> Option<Gpu> {
    let first = vendors.first().copied()?;
    let second = vendors.iter().copied().find(|vendor| *vendor != first);

    match second {
        Some(second_vendor) => classify_hybrid_pair(first, second_vendor).or(Some(Gpu::Hybrid {
            igpu: first,
            dgpu: second_vendor,
        })),
        None => Some(Gpu::Single(first)),
    }
}

fn classify_hybrid_pair(first: GpuVendor, second: GpuVendor) -> Option<Gpu> {
    use GpuVendor::{Amd, Intel, Nvidia};

    match (first, second) {
        (Intel, Nvidia) | (Nvidia, Intel) => Some(Gpu::Hybrid {
            igpu: Intel,
            dgpu: Nvidia,
        }),
        (Amd, Nvidia) | (Nvidia, Amd) => Some(Gpu::Hybrid {
            igpu: Amd,
            dgpu: Nvidia,
        }),
        (Intel, Amd) | (Amd, Intel) => Some(Gpu::Hybrid {
            igpu: Intel,
            dgpu: Amd,
        }),
        _ => None,
    }
}

fn is_whole_disk_name(name: &str) -> bool {
    if name.starts_with("loop") || name.starts_with("ram") || name.starts_with("zram") {
        return false;
    }
    if name.starts_with("dm-") || name.starts_with("md") {
        return false;
    }
    if name.starts_with("sr") {
        return false;
    }

    if let Some(rest) = name.strip_prefix("nvme") {
        let Some((controller, namespace)) = rest.split_once('n') else {
            return false;
        };
        return !controller.is_empty()
            && controller
                .chars()
                .all(|character| character.is_ascii_digit())
            && !namespace.is_empty()
            && namespace
                .chars()
                .all(|character| character.is_ascii_digit());
    }

    if let Some(rest) = name.strip_prefix("mmcblk") {
        return !rest.is_empty() && rest.chars().all(|character| character.is_ascii_digit());
    }

    !name
        .chars()
        .last()
        .is_some_and(|character| character.is_ascii_digit())
}

fn parse_cpu_present(present: &str) -> Option<usize> {
    let total = present
        .trim()
        .split(',')
        .map(|segment| {
            let segment = segment.trim();
            if segment.is_empty() {
                return None;
            }

            if let Some((start, end)) = segment.split_once('-') {
                let start = start.trim().parse::<usize>().ok()?;
                let end = end.trim().parse::<usize>().ok()?;
                end.checked_sub(start)?.checked_add(1)
            } else {
                segment.parse::<usize>().ok().map(|_| 1)
            }
        })
        .try_fold(0usize, |acc, count| acc.checked_add(count?))?;

    (total > 0).then_some(total)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(target_os = "linux")]
    #[test]
    fn detect_ram_returns_positive_value() {
        let ram = detect_ram();

        assert!(ram.is_some_and(|value| value > 0));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn detect_cpu_count_returns_positive_value() {
        assert!(detect_cpu_count() > 0);
    }

    #[test]
    fn default_swap_returns_zram() {
        assert!(matches!(default_swap(), SwapConfig::Zram { .. }));
    }

    #[test]
    fn vendor_matches_detects_case_insensitive_substrings() {
        assert!(vendor_matches("ASUSTeK COMPUTER INC.", "asus"));
    }

    #[test]
    fn vendor_matches_rejects_other_vendors() {
        assert!(!vendor_matches("Dell Inc.", "asus"));
    }

    #[test]
    fn classifies_single_vendor_gpu() {
        assert_eq!(
            classify_gpu_vendors(&[GpuVendor::Amd]),
            Some(Gpu::Single(GpuVendor::Amd))
        );
    }

    #[test]
    fn classifies_intel_nvidia_hybrid_as_prime_offload() {
        assert_eq!(
            classify_gpu_vendors(&[GpuVendor::Nvidia, GpuVendor::Intel]),
            Some(Gpu::Hybrid {
                igpu: GpuVendor::Intel,
                dgpu: GpuVendor::Nvidia,
            })
        );
    }

    #[test]
    fn classifies_amd_nvidia_hybrid_with_amd_igpu() {
        assert_eq!(
            classify_gpu_vendors(&[GpuVendor::Amd, GpuVendor::Nvidia]),
            Some(Gpu::Hybrid {
                igpu: GpuVendor::Amd,
                dgpu: GpuVendor::Nvidia,
            })
        );
    }

    #[test]
    fn parse_vendor_id_maps_supported_vendors() {
        assert_eq!(parse_vendor_id("0x1002"), Some(GpuVendor::Amd));
        assert_eq!(parse_vendor_id("0x8086"), Some(GpuVendor::Intel));
        assert_eq!(parse_vendor_id("0x10de"), Some(GpuVendor::Nvidia));
    }

    #[test]
    fn parse_drm_driver_reads_driver_from_uevent() {
        assert_eq!(parse_drm_driver("DRIVER=amdgpu\n"), Some(GpuVendor::Amd));
        assert_eq!(parse_drm_driver("DRIVER=i915\n"), Some(GpuVendor::Intel));
        assert_eq!(
            parse_drm_driver("DRIVER=nvidia_drm\n"),
            Some(GpuVendor::Nvidia)
        );
        assert_eq!(parse_drm_driver("PCI_ID=1002:1638\n"), None);
    }

    #[test]
    fn whole_disk_detection_rejects_emmc_special_areas_and_partitions() {
        assert!(is_whole_disk_name("mmcblk0"));
        assert!(!is_whole_disk_name("mmcblk0p1"));
        assert!(!is_whole_disk_name("mmcblk0boot0"));
        assert!(!is_whole_disk_name("mmcblk0rpmb"));
    }

    #[test]
    fn whole_disk_detection_handles_nvme_names() {
        assert!(is_whole_disk_name("nvme0n1"));
        assert!(!is_whole_disk_name("nvme0n1p1"));
        assert!(!is_whole_disk_name("nvme0"));
    }

    #[test]
    fn parse_cpu_present_counts_non_contiguous_ranges() {
        assert_eq!(parse_cpu_present("0-3,6-7"), Some(6));
    }

    #[test]
    fn parse_cpu_present_rejects_invalid_ranges() {
        assert_eq!(parse_cpu_present("7-3"), None);
    }
}
