use super::*;

pub(super) const IMAGE_CAPABILITY_MANIFEST: &str = "usr/share/oxys/image-capabilities.toml";
pub(super) const IMAGE_CAPABILITY_CHECKSUM: &str = "usr/share/oxys/image-capabilities.toml.sha256";

#[derive(Debug, Deserialize)]
struct ImageCapabilityManifest {
    format_version: u32,
    graphics: ImageGraphicsCapabilities,
}

#[derive(Debug, Deserialize, Default)]
struct ImageGraphicsCapabilities {
    #[serde(default)]
    mesa: ImageMesaCapabilities,
    #[serde(default)]
    kernel: ImageKernelCapabilities,
    #[serde(default)]
    nvidia: ImageNvidiaCapabilities,
    #[serde(default)]
    vm: ImageVmCapabilities,
}

#[derive(Debug, Deserialize, Default)]
struct ImageMesaCapabilities {
    #[serde(default)]
    video_cards: Vec<String>,
    #[serde(default)]
    artifacts: Vec<String>,
}

#[derive(Debug, Deserialize, Default)]
struct ImageKernelCapabilities {
    #[serde(default)]
    config_path: String,
    #[serde(default)]
    enabled: Vec<String>,
}

#[derive(Debug, Deserialize, Default)]
struct ImageNvidiaCapabilities {
    #[serde(default)]
    proprietary: bool,
    driver_version: Option<String>,
    kernel_abi: Option<String>,
}

#[derive(Debug, Deserialize, Default)]
struct ImageVmCapabilities {
    #[serde(default)]
    virgl: bool,
    #[serde(default)]
    vmware: bool,
    virgl_launch_device: Option<String>,
    vmware_launch_device: Option<String>,
}

pub(super) fn compare_capability_manifest(
    requirements: &GraphicsRequirements,
    source_root: &Path,
    comparison: &mut GraphicsCapabilityComparison,
) -> bool {
    let path = source_root.join(IMAGE_CAPABILITY_MANIFEST);
    if !path.is_file() {
        return false;
    }
    comparison.capability_manifest_path = Some(path.clone());

    let contents = match fs::read(&path) {
        Ok(contents) => contents,
        Err(error) => {
            comparison.missing.push(format!(
                "could not read image capability manifest {}: {error}",
                path.display()
            ));
            return true;
        }
    };
    let checksum_path = source_root.join(IMAGE_CAPABILITY_CHECKSUM);
    let expected_checksum = fs::read_to_string(&checksum_path)
        .ok()
        .and_then(|value| value.split_whitespace().next().map(str::to_owned));
    let actual_checksum = crate::util::sha256_hex(&contents);
    match expected_checksum {
        Some(expected) if expected == actual_checksum => {
            comparison.capability_manifest_checksum_verified = true;
        }
        Some(expected) => {
            comparison.missing.push(format!(
                "image capability manifest checksum mismatch: expected {expected}, calculated {actual_checksum}"
            ));
            return true;
        }
        None => {
            comparison.missing.push(format!(
                "image capability manifest has no readable checksum at {}",
                checksum_path.display()
            ));
            return true;
        }
    }

    let manifest_text = match std::str::from_utf8(&contents) {
        Ok(contents) => contents,
        Err(error) => {
            comparison
                .missing
                .push(format!("image capability manifest is not UTF-8: {error}"));
            return true;
        }
    };
    let manifest: ImageCapabilityManifest = match toml::from_str(manifest_text) {
        Ok(manifest) => manifest,
        Err(error) => {
            comparison
                .missing
                .push(format!("invalid image capability manifest: {error}"));
            return true;
        }
    };
    if manifest.format_version != 1 {
        comparison.missing.push(format!(
            "unsupported image capability manifest format version {}",
            manifest.format_version
        ));
        return true;
    }

    comparison.mesa_checked = !requirements.mesa_artifacts.is_empty();
    comparison.available_mesa_artifacts = manifest.graphics.mesa.artifacts.clone();
    for requirement in &requirements.mesa_artifacts {
        let declared = manifest
            .graphics
            .mesa
            .video_cards
            .contains(&requirement.capability);
        let artifact_present = requirement.alternatives.iter().any(|candidate| {
            manifest.graphics.mesa.artifacts.contains(candidate)
                && source_root
                    .join(candidate.trim_start_matches('/'))
                    .is_file()
        });
        if !declared {
            comparison.missing.push(format!(
                "image capability manifest does not provide {}",
                requirement.capability
            ));
        } else if !artifact_present {
            comparison.missing.push(format!(
                "image capability manifest declares {} but its expected artifact is absent",
                requirement.capability
            ));
        }
    }

    comparison.available_kernel_config = manifest.graphics.kernel.enabled.clone();
    if !manifest.graphics.kernel.config_path.is_empty() {
        let config_path =
            source_root.join(manifest.graphics.kernel.config_path.trim_start_matches('/'));
        if !config_path.is_file() {
            comparison.missing.push(format!(
                "image capability manifest names missing kernel config {}",
                config_path.display()
            ));
        }
        comparison.kernel_config_path = Some(config_path);
    }
    for option in &requirements.kernel_config {
        if !manifest.graphics.kernel.enabled.contains(option) {
            comparison.missing.push(format!(
                "image capability manifest is missing kernel config {option}"
            ));
        }
    }

    match requirements.vm_support {
        VmGraphics::Virgl => {
            if !manifest.graphics.vm.virgl {
                comparison.missing.push(
                    "image capability manifest does not advertise Virgl VM support".to_owned(),
                );
            }
            if manifest.graphics.vm.virgl_launch_device.is_none() {
                comparison.missing.push(
                    "image capability manifest lacks the Virgl launch-device contract".to_owned(),
                );
            }
        }
        VmGraphics::Vmware => {
            if !manifest.graphics.vm.vmware {
                comparison.missing.push(
                    "image capability manifest does not advertise VMware VM graphics support"
                        .to_owned(),
                );
            }
            if manifest.graphics.vm.vmware_launch_device.is_none() {
                comparison.missing.push(
                    "image capability manifest lacks the VMware launch-device contract".to_owned(),
                );
            }
        }
        VmGraphics::None => {}
    }

    if requirements.proprietary_nvidia {
        comparison.nvidia_driver_version = manifest.graphics.nvidia.driver_version;
        comparison.nvidia_kernel_abi = manifest.graphics.nvidia.kernel_abi;
        if !manifest.graphics.nvidia.proprietary {
            comparison.missing.push(
                "image capability manifest does not provide proprietary NVIDIA support".to_owned(),
            );
        } else if comparison.nvidia_driver_version.is_none()
            || comparison.nvidia_kernel_abi.is_none()
        {
            comparison.missing.push(
                "image capability manifest lacks NVIDIA driver version or kernel ABI".to_owned(),
            );
        }
    }
    true
}

pub(super) fn compare_nvidia_capability(
    root: &Path,
    comparison: &mut GraphicsCapabilityComparison,
) {
    comparison.nvidia_driver_version = fs::read_dir(root.join("var/db/pkg/x11-drivers"))
        .ok()
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .filter_map(|entry| entry.file_name().into_string().ok())
        .find_map(|name| name.strip_prefix("nvidia-drivers-").map(str::to_owned));
    if comparison.nvidia_driver_version.is_none() {
        comparison.missing.push(
            "proprietary NVIDIA policy requires an installed x11-drivers/nvidia-drivers capability"
                .to_owned(),
        );
    }

    let boot_abi = newest_named_entry(root.join("boot"), "vmlinuz-");
    let module_abis = fs::read_dir(root.join("lib/modules"))
        .ok()
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
        .filter_map(|entry| entry.file_name().into_string().ok())
        .collect::<Vec<_>>();
    let matching_abi = module_abis.into_iter().find(|abi| {
        boot_abi.as_ref().is_none_or(|boot| boot == abi)
            && directory_contains_nvidia_drm(&root.join("lib/modules").join(abi))
    });
    comparison.nvidia_kernel_abi = matching_abi.clone();
    match (boot_abi, matching_abi) {
        (Some(boot), None) => comparison.missing.push(format!(
            "proprietary NVIDIA module nvidia_drm is missing for installed kernel ABI {boot}"
        )),
        (None, None) => comparison.missing.push(
            "proprietary NVIDIA policy requires a matching lib/modules/<kernel ABI>/nvidia_drm module"
                .to_owned(),
        ),
        _ => {}
    }
}

fn newest_named_entry(dir: PathBuf, prefix: &str) -> Option<String> {
    let mut names = fs::read_dir(dir)
        .ok()?
        .filter_map(Result::ok)
        .filter_map(|entry| entry.file_name().into_string().ok())
        .filter_map(|name| name.strip_prefix(prefix).map(str::to_owned))
        .collect::<Vec<_>>();
    names.sort();
    names.pop()
}

fn directory_contains_nvidia_drm(dir: &Path) -> bool {
    let Ok(entries) = fs::read_dir(dir) else {
        return false;
    };
    for entry in entries.filter_map(Result::ok) {
        let path = entry.path();
        if path.is_dir() {
            if directory_contains_nvidia_drm(&path) {
                return true;
            }
        } else if path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name == "nvidia_drm.ko" || name.starts_with("nvidia_drm.ko."))
        {
            return true;
        }
    }
    false
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct DetectedRenderNode {
    pub(super) path: String,
    pub(super) vendor: String,
}

pub(super) fn detect_render_nodes(drm_root: &Path) -> Vec<DetectedRenderNode> {
    let mut nodes = fs::read_dir(drm_root)
        .ok()
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let name = entry.file_name().into_string().ok()?;
            if !name.starts_with("renderD") {
                return None;
            }
            let vendor = fs::read_to_string(entry.path().join("device/vendor")).ok()?;
            Some(DetectedRenderNode {
                path: format!("/dev/dri/{name}"),
                vendor: vendor.trim().to_ascii_lowercase(),
            })
        })
        .collect::<Vec<_>>();
    nodes.sort_by(|left, right| left.path.cmp(&right.path));
    nodes
}

pub(super) fn find_kernel_config(root: &Path) -> Option<PathBuf> {
    let direct = root.join("usr/src/linux/.config");
    if direct.is_file() {
        return Some(direct);
    }
    let mut candidates = fs::read_dir(root.join("boot"))
        .ok()?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with("config-"))
        })
        .collect::<Vec<_>>();
    candidates.sort();
    candidates.pop()
}

pub(super) fn parse_enabled_kernel_options(contents: &str) -> BTreeSet<String> {
    contents
        .lines()
        .filter_map(|line| {
            let (key, value) = line.split_once('=')?;
            matches!(value.trim(), "y" | "m").then(|| key.trim().to_owned())
        })
        .collect()
}
