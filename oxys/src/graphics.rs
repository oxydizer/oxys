use std::{
    collections::BTreeSet,
    fs,
    path::{Path, PathBuf},
};

use serde::Deserialize;
use thiserror::Error;

use crate::{
    manifest::{
        DrmDriver, DrmDrivers, Graphics, Nvidia, NvidiaDriver, PrimeMode, SoftwareRenderer,
        SystemManifest, VideoCard, VideoCards, VmGraphics,
    },
    session::DecisionSource,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GraphicsDecision {
    pub field: String,
    pub value: String,
    pub source: DecisionSource,
    pub reason: String,
    pub affected: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MesaArtifactRequirement {
    pub capability: String,
    pub alternatives: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrimeRouting {
    pub mode: PrimeMode,
    pub compositor_gpu: String,
    pub offload_gpu: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RequiredKernelArg {
    pub value: String,
    pub source_field: String,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct GraphicsRequirements {
    pub mesa_video_cards: Vec<VideoCard>,
    pub mesa_artifacts: Vec<MesaArtifactRequirement>,
    pub drm_drivers: Vec<DrmDriver>,
    pub kernel_config: Vec<String>,
    pub packages: Vec<String>,
    pub kernel_modules: Vec<String>,
    pub initramfs_modules: Vec<String>,
    pub module_blacklist: Vec<String>,
    pub kernel_args: Vec<RequiredKernelArg>,
    pub prime: Option<PrimeRouting>,
    pub diagnostics: Vec<String>,
    pub proprietary_nvidia: bool,
    pub vm_support: VmGraphics,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GraphicsPolicy {
    pub mesa_video_cards: Vec<VideoCard>,
    pub drm_drivers: Vec<DrmDriver>,
    pub software_fallback: SoftwareRenderer,
    pub nvidia: Option<Nvidia>,
    pub vm_support: VmGraphics,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct GraphicsCapabilityComparison {
    pub source_root: Option<PathBuf>,
    pub capability_manifest_path: Option<PathBuf>,
    pub capability_manifest_checksum_verified: bool,
    pub mesa_checked: bool,
    pub kernel_config_path: Option<PathBuf>,
    pub available_mesa_artifacts: Vec<String>,
    pub available_kernel_config: Vec<String>,
    pub missing: Vec<String>,
    pub nvidia_driver_version: Option<String>,
    pub nvidia_kernel_abi: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedGraphics {
    pub policy: GraphicsPolicy,
    pub requirements: GraphicsRequirements,
    pub capabilities: GraphicsCapabilityComparison,
    pub decisions: Vec<GraphicsDecision>,
    pub warnings: Vec<String>,
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum GraphicsResolveError {
    #[error("invalid graphics configuration: {0}")]
    Invalid(String),
    #[error("source image does not satisfy graphics policy: {0}")]
    Capability(String),
}

impl SystemManifest {
    pub fn resolved_graphics(&self) -> Result<ResolvedGraphics, GraphicsResolveError> {
        resolve_graphics_with_detection(self, None)
    }

    pub fn resolved_graphics_for_source(
        &self,
        source_root: &Path,
    ) -> Result<ResolvedGraphics, GraphicsResolveError> {
        // `detect_graphics()` is intentionally a manifest-construction helper:
        // bundled configs call it while collecting target hardware. Planning
        // must remain deterministic and therefore resolves only the policy
        // serialized into this manifest.
        resolve_graphics_with_detection(self, None)?.validate_source(source_root)
    }
}

pub fn resolve_graphics(
    manifest: &SystemManifest,
) -> Result<ResolvedGraphics, GraphicsResolveError> {
    manifest.resolved_graphics()
}

fn resolve_graphics_with_detection(
    manifest: &SystemManifest,
    detected: Option<&Graphics>,
) -> Result<ResolvedGraphics, GraphicsResolveError> {
    let input = &manifest.hardware.graphics;
    let mut decisions = Vec::new();
    let mut warnings = Vec::new();

    if let Some(legacy) = &manifest.legacy_gpu {
        decisions.push(decision(
            "hardware.gpu",
            &format!("{legacy:?} -> {:?}", input),
            DecisionSource::LegacyInference,
            "retired hardware.gpu was converted to the unified graphics policy",
            &["hardware.graphics"],
        ));
    }

    let mut cards = match &input.mesa.video_cards {
        VideoCards::Explicit(cards) if cards.is_empty() => {
            return Err(invalid(
                "hardware.graphics.mesa.video_cards is explicit but empty",
            ));
        }
        VideoCards::Explicit(cards) => {
            decisions.push(decision(
                "hardware.graphics.mesa.video_cards",
                &debug_list(cards),
                DecisionSource::Explicit,
                "Mesa VIDEO_CARDS were selected explicitly",
                &["media-libs/mesa"],
            ));
            cards.clone()
        }
        VideoCards::Auto => match detected.map(|value| &value.mesa.video_cards) {
            Some(VideoCards::Explicit(cards)) if !cards.is_empty() => {
                decisions.push(decision(
                    "hardware.graphics.mesa.video_cards",
                    &debug_list(cards),
                    DecisionSource::Default,
                    "automatic graphics detection selected Mesa drivers",
                    &["media-libs/mesa"],
                ));
                cards.clone()
            }
            _ => {
                warnings.push(
                    "hardware.graphics.mesa.video_cards = auto detected no physical GPU; only explicitly requested VM/NVIDIA capabilities will be validated"
                        .to_owned(),
                );
                Vec::new()
            }
        },
    };

    match input.vm_support {
        VmGraphics::Virgl => push_unique(&mut cards, VideoCard::Virgl),
        VmGraphics::Vmware => push_unique(&mut cards, VideoCard::Vmware),
        VmGraphics::None => {}
    }
    if input.mesa.software_fallback == SoftwareRenderer::Required {
        push_unique(&mut cards, VideoCard::Lavapipe);
    }

    if let Some(nvidia) = input.nvidia {
        match nvidia.driver {
            NvidiaDriver::Proprietary => {
                if cards.contains(&VideoCard::Nouveau) {
                    return Err(invalid(
                        "hardware.graphics.nvidia.driver = proprietary conflicts with VideoCard::Nouveau",
                    ));
                }
            }
            NvidiaDriver::Nouveau => {
                if nvidia.prime != PrimeMode::Disabled {
                    return Err(invalid(
                        "Nouveau PRIME routing is not supported by the initial graphics resolver; use prime = disabled",
                    ));
                }
                push_unique(&mut cards, VideoCard::Nouveau);
            }
        }
    }

    let mut drivers = match &input.drm.drivers {
        DrmDrivers::Explicit(drivers) if drivers.is_empty() => {
            return Err(invalid(
                "hardware.graphics.drm.drivers is explicit but empty",
            ));
        }
        DrmDrivers::Explicit(drivers) => {
            decisions.push(decision(
                "hardware.graphics.drm.drivers",
                &debug_list(drivers),
                DecisionSource::Explicit,
                "kernel DRM drivers were selected explicitly",
                &["kernel config"],
            ));
            drivers.clone()
        }
        DrmDrivers::Auto => {
            let mut derived = Vec::new();
            for card in &cards {
                if let Some(driver) = drm_for_card(*card) {
                    push_unique(&mut derived, driver);
                }
            }
            if !derived.is_empty() {
                decisions.push(decision(
                    "hardware.graphics.drm.drivers",
                    &debug_list(&derived),
                    DecisionSource::Dependency,
                    "derived kernel DRM drivers from the resolved Mesa and VM policy",
                    &["kernel config"],
                ));
            }
            derived
        }
    };

    if input.vm_support == VmGraphics::Virgl {
        push_unique(&mut drivers, DrmDriver::VirtioGpu);
    } else if input.vm_support == VmGraphics::Vmware {
        push_unique(&mut drivers, DrmDriver::Vmwgfx);
    }
    if input
        .nvidia
        .is_some_and(|nvidia| nvidia.driver == NvidiaDriver::Nouveau)
    {
        push_unique(&mut drivers, DrmDriver::Nouveau);
    }

    validate_card_driver_pairs(&cards, &drivers)?;
    validate_nvidia_policy(manifest, &cards, &drivers)?;

    let requirements = build_requirements(input, &cards, &drivers)?;
    for card in &cards {
        decisions.push(decision(
            "graphics.requirement.video_card",
            &format!("{card:?}"),
            DecisionSource::Dependency,
            "required by the resolved graphics policy",
            &["media-libs/mesa"],
        ));
    }
    for option in &requirements.kernel_config {
        decisions.push(decision(
            "graphics.requirement.kernel_config",
            option,
            DecisionSource::Dependency,
            "required by the resolved DRM driver set",
            &["kernel config"],
        ));
    }

    Ok(ResolvedGraphics {
        policy: GraphicsPolicy {
            mesa_video_cards: cards,
            drm_drivers: drivers,
            software_fallback: input.mesa.software_fallback,
            nvidia: input.nvidia,
            vm_support: input.vm_support,
        },
        requirements,
        capabilities: GraphicsCapabilityComparison::default(),
        decisions,
        warnings,
    })
}

impl ResolvedGraphics {
    /// Portage VIDEO_CARDS values required when building Mesa for this policy.
    pub fn mesa_build_values(&self) -> Vec<&'static str> {
        self.policy
            .mesa_video_cards
            .iter()
            .map(|card| match card {
                VideoCard::Intel => "intel",
                VideoCard::Amdgpu => "amdgpu",
                VideoCard::Radeon => "radeon",
                VideoCard::Radeonsi => "radeonsi",
                VideoCard::Nouveau => "nouveau",
                VideoCard::Virgl => "virgl",
                VideoCard::Vmware => "vmware",
                VideoCard::Lavapipe => "lavapipe",
            })
            .collect()
    }

    /// Stable build-script names for the required kernel DRM drivers.
    pub fn drm_build_values(&self) -> Vec<&'static str> {
        self.policy
            .drm_drivers
            .iter()
            .map(|driver| match driver {
                DrmDriver::Intel => "intel",
                DrmDriver::Amdgpu => "amdgpu",
                DrmDriver::Radeon => "radeon",
                DrmDriver::Nouveau => "nouveau",
                DrmDriver::VirtioGpu => "virtio_gpu",
                DrmDriver::Vmwgfx => "vmwgfx",
            })
            .collect()
    }

    pub fn resolve_runtime_nodes(mut self) -> Result<Self, GraphicsResolveError> {
        let Some(prime) = self.requirements.prime.as_mut() else {
            return Ok(self);
        };
        let nodes = detect_render_nodes(Path::new("/sys/class/drm"));
        let compositor_vendor = match prime.mode {
            PrimeMode::Primary => "0x10de",
            PrimeMode::Offload if self.policy.mesa_video_cards.contains(&VideoCard::Intel) => {
                "0x8086"
            }
            PrimeMode::Offload => "0x1002",
            PrimeMode::Disabled => unreachable!("disabled PRIME has no routing requirement"),
        };
        let compositor = nodes
            .iter()
            .find(|node| node.vendor == compositor_vendor)
            .ok_or_else(|| {
                invalid(format!(
                    "PRIME {:?} could not find the required compositor render node for PCI vendor {compositor_vendor}",
                    prime.mode
                ))
            })?;
        prime.compositor_gpu = compositor.path.clone();
        if prime.mode == PrimeMode::Offload {
            let offload = nodes
                .iter()
                .find(|node| node.vendor == "0x10de")
                .ok_or_else(|| invalid("PRIME offload could not find an NVIDIA render node"))?;
            prime.offload_gpu = Some(format!("{} via prime-run", offload.path));
        }
        Ok(self)
    }

    pub fn validate_source(mut self, source_root: &Path) -> Result<Self, GraphicsResolveError> {
        self.capabilities = compare_source_capabilities(&self.requirements, source_root);
        if !self.capabilities.missing.is_empty() {
            return Err(GraphicsResolveError::Capability(
                self.capabilities.missing.join("; "),
            ));
        }
        Ok(self)
    }

    pub fn materialize_manifest(&self, manifest: &SystemManifest) -> SystemManifest {
        let mut result = manifest.clone();
        for atom in &self.requirements.packages {
            if !result
                .packages
                .iter()
                .any(|package| package.package.trim().starts_with(atom))
            {
                let mut package = crate::manifest::Package::new(atom);
                if atom == "x11-drivers/nvidia-drivers" {
                    package = package.accept_licenses(["NVIDIA-r2"]);
                }
                result.packages.push(package);
            }
        }
        result
    }

    pub fn render(&self) -> String {
        let mut lines = vec![format!(
            "graphics policy: Mesa [{}], DRM [{}]",
            debug_list(&self.policy.mesa_video_cards),
            debug_list(&self.policy.drm_drivers)
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
        if let Some(root) = &self.capabilities.source_root {
            lines.push(format!("source image: {}", root.display()));
            if let Some(manifest) = &self.capabilities.capability_manifest_path {
                lines.push(format!(
                    "image capability manifest: {} (checksum {})",
                    manifest.display(),
                    if self.capabilities.capability_manifest_checksum_verified {
                        "verified"
                    } else {
                        "not verified"
                    }
                ));
            }
            lines.push(format!(
                "Mesa capability check: {}",
                if self.capabilities.mesa_checked {
                    "passed"
                } else {
                    "not required"
                }
            ));
            if let Some(config) = &self.capabilities.kernel_config_path {
                lines.push(format!("kernel capability check: {}", config.display()));
            }
            if let (Some(version), Some(abi)) = (
                &self.capabilities.nvidia_driver_version,
                &self.capabilities.nvidia_kernel_abi,
            ) {
                lines.push(format!(
                    "NVIDIA capability: driver {version}, kernel ABI {abi}"
                ));
            }
        }
        if let Some(prime) = &self.requirements.prime {
            lines.push(format!("PRIME mode: {:?}", prime.mode));
            lines.push(format!("compositor GPU: {}", prime.compositor_gpu));
            if let Some(offload) = &prime.offload_gpu {
                lines.push(format!("offload GPU: {offload}"));
            }
        }
        if !self.requirements.packages.is_empty() {
            lines.push(format!(
                "packages: {}",
                self.requirements.packages.join(", ")
            ));
        }
        if !self.requirements.kernel_modules.is_empty() {
            lines.push(format!(
                "kernel modules: {}",
                self.requirements.kernel_modules.join(", ")
            ));
        }
        if !self.requirements.initramfs_modules.is_empty() {
            lines.push(format!(
                "initramfs modules: {}",
                self.requirements.initramfs_modules.join(", ")
            ));
        }
        if !self.requirements.module_blacklist.is_empty() {
            lines.push(format!(
                "module blacklist: {}",
                self.requirements.module_blacklist.join(", ")
            ));
        }
        for diagnostic in &self.requirements.diagnostics {
            lines.push(format!("diagnostic: {diagnostic}"));
        }
        lines.join("\n")
    }
}

fn build_requirements(
    graphics: &Graphics,
    cards: &[VideoCard],
    drivers: &[DrmDriver],
) -> Result<GraphicsRequirements, GraphicsResolveError> {
    let mut requirements = GraphicsRequirements {
        mesa_video_cards: cards.to_vec(),
        drm_drivers: drivers.to_vec(),
        diagnostics: vec![
            "report LIBSEAT_BACKEND and active session backend".to_owned(),
            "list /dev/dri/card* and /dev/dri/renderD* nodes".to_owned(),
            "report loaded DRM modules and Mesa renderer".to_owned(),
        ],
        vm_support: graphics.vm_support,
        ..GraphicsRequirements::default()
    };

    for card in cards {
        if let Some(artifact) = mesa_artifact_requirement(*card) {
            if !requirements
                .mesa_artifacts
                .iter()
                .any(|existing| existing.capability == artifact.capability)
            {
                requirements.mesa_artifacts.push(artifact);
            }
        }
    }
    for driver in drivers {
        for option in kernel_options(*driver) {
            push_unique_string(&mut requirements.kernel_config, option);
        }
    }

    if let Some(nvidia) = graphics.nvidia {
        if nvidia.prime != PrimeMode::Disabled && !nvidia.modeset {
            return Err(invalid(
                "NVIDIA PRIME primary/offload routing requires hardware.graphics.nvidia.modeset = true",
            ));
        }
        match nvidia.driver {
            NvidiaDriver::Proprietary => {
                requirements.proprietary_nvidia = true;
                requirements
                    .packages
                    .push("x11-drivers/nvidia-drivers".to_owned());
                requirements.kernel_modules.extend(
                    ["nvidia", "nvidia_modeset", "nvidia_uvm", "nvidia_drm"]
                        .into_iter()
                        .map(str::to_owned),
                );
                requirements.initramfs_modules = requirements.kernel_modules.clone();
                requirements.module_blacklist.push("nouveau".to_owned());
                if nvidia.modeset {
                    requirements.kernel_args.push(RequiredKernelArg {
                        value: "nvidia_drm.modeset=1".to_owned(),
                        source_field: "hardware.graphics.nvidia.modeset".to_owned(),
                        reason: "proprietary NVIDIA DRM modesetting requested".to_owned(),
                    });
                }
            }
            NvidiaDriver::Nouveau => {
                requirements.module_blacklist.push("nvidia".to_owned());
            }
        }

        requirements.prime = match nvidia.prime {
            PrimeMode::Disabled => None,
            PrimeMode::Primary => Some(PrimeRouting {
                mode: PrimeMode::Primary,
                compositor_gpu: "NVIDIA primary render node".to_owned(),
                offload_gpu: None,
            }),
            PrimeMode::Offload => {
                let integrated = integrated_gpu(cards).ok_or_else(|| {
                    invalid(
                        "hardware.graphics.nvidia.prime = offload requires an Intel or AMD integrated Mesa path",
                    )
                })?;
                Some(PrimeRouting {
                    mode: PrimeMode::Offload,
                    compositor_gpu: format!("{integrated} integrated render node"),
                    offload_gpu: Some("NVIDIA render node via prime-run".to_owned()),
                })
            }
        };
    }
    Ok(requirements)
}

fn validate_card_driver_pairs(
    cards: &[VideoCard],
    drivers: &[DrmDriver],
) -> Result<(), GraphicsResolveError> {
    for card in cards {
        if let Some(driver) = drm_for_card(*card) {
            if !drivers.contains(&driver) {
                return Err(invalid(format!(
                    "hardware.graphics Mesa card {card:?} requires DRM driver {driver:?}"
                )));
            }
        }
    }
    Ok(())
}

fn validate_nvidia_policy(
    manifest: &SystemManifest,
    cards: &[VideoCard],
    drivers: &[DrmDriver],
) -> Result<(), GraphicsResolveError> {
    let Some(nvidia) = manifest.hardware.graphics.nvidia else {
        return Ok(());
    };
    let proprietary_package = manifest
        .packages
        .iter()
        .any(|package| package.package.starts_with("x11-drivers/nvidia-drivers"));
    match nvidia.driver {
        NvidiaDriver::Proprietary => {
            if cards.contains(&VideoCard::Nouveau) || drivers.contains(&DrmDriver::Nouveau) {
                return Err(invalid(
                    "proprietary NVIDIA and Nouveau cannot be active in the same resolved target",
                ));
            }
        }
        NvidiaDriver::Nouveau if proprietary_package => {
            return Err(invalid(
                "hardware.graphics.nvidia.driver = nouveau conflicts with package x11-drivers/nvidia-drivers",
            ));
        }
        NvidiaDriver::Nouveau => {}
    }
    Ok(())
}

fn compare_source_capabilities(
    requirements: &GraphicsRequirements,
    source_root: &Path,
) -> GraphicsCapabilityComparison {
    let mut comparison = GraphicsCapabilityComparison {
        source_root: Some(source_root.to_path_buf()),
        ..GraphicsCapabilityComparison::default()
    };

    if compare_capability_manifest(requirements, source_root, &mut comparison) {
        return comparison;
    }

    if !requirements.mesa_artifacts.is_empty() {
        comparison.mesa_checked = true;
        for requirement in &requirements.mesa_artifacts {
            let found = requirement
                .alternatives
                .iter()
                .find(|relative| source_root.join(relative.trim_start_matches('/')).is_file());
            if let Some(relative) = found {
                comparison.available_mesa_artifacts.push(relative.clone());
            } else {
                comparison.missing.push(format!(
                    "Mesa capability {} requires one of: {}",
                    requirement.capability,
                    requirement.alternatives.join(", ")
                ));
            }
        }
    }

    if !requirements.kernel_config.is_empty() {
        match find_kernel_config(source_root) {
            Some(path) => match fs::read_to_string(&path) {
                Ok(contents) => {
                    comparison.kernel_config_path = Some(path);
                    let enabled = parse_enabled_kernel_options(&contents);
                    comparison.available_kernel_config = enabled.iter().cloned().collect();
                    for option in &requirements.kernel_config {
                        if !enabled.contains(option) {
                            comparison
                                .missing
                                .push(format!("kernel config is missing {option}"));
                        }
                    }
                }
                Err(error) => comparison.missing.push(format!(
                    "could not read kernel config {}: {error}",
                    path.display()
                )),
            },
            None => comparison.missing.push(
                "kernel DRM requirements exist but the source image has no readable boot/config-* or usr/src/linux/.config"
                    .to_owned(),
            ),
        }
    }
    if requirements.proprietary_nvidia {
        compare_nvidia_capability(source_root, &mut comparison);
    }
    comparison
}

const IMAGE_CAPABILITY_MANIFEST: &str = "usr/share/oxys/image-capabilities.toml";
const IMAGE_CAPABILITY_CHECKSUM: &str = "usr/share/oxys/image-capabilities.toml.sha256";

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

fn compare_capability_manifest(
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

fn compare_nvidia_capability(root: &Path, comparison: &mut GraphicsCapabilityComparison) {
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
struct DetectedRenderNode {
    path: String,
    vendor: String,
}

fn detect_render_nodes(drm_root: &Path) -> Vec<DetectedRenderNode> {
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

fn find_kernel_config(root: &Path) -> Option<PathBuf> {
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

fn parse_enabled_kernel_options(contents: &str) -> BTreeSet<String> {
    contents
        .lines()
        .filter_map(|line| {
            let (key, value) = line.split_once('=')?;
            matches!(value.trim(), "y" | "m").then(|| key.trim().to_owned())
        })
        .collect()
}

fn mesa_artifact_requirement(card: VideoCard) -> Option<MesaArtifactRequirement> {
    let (capability, names): (&str, &[&str]) = match card {
        VideoCard::Intel => ("video_cards_intel", &["iris_dri.so", "crocus_dri.so"]),
        VideoCard::Amdgpu => ("video_cards_amdgpu", &["radeonsi_dri.so"]),
        VideoCard::Radeonsi => ("video_cards_radeonsi", &["radeonsi_dri.so"]),
        VideoCard::Radeon => ("video_cards_radeon", &["r600_dri.so", "radeon_dri.so"]),
        VideoCard::Nouveau => ("video_cards_nouveau", &["nouveau_dri.so"]),
        VideoCard::Virgl => ("video_cards_virgl", &["virtio_gpu_dri.so"]),
        VideoCard::Vmware => ("video_cards_vmware", &["vmwgfx_dri.so"]),
        VideoCard::Lavapipe => (
            "video_cards_lavapipe",
            &["libvulkan_lvp.so", "libvulkan_lvp.so.1"],
        ),
    };
    let alternatives = ["usr/lib64", "usr/lib", "usr/lib/x86_64-linux-gnu"]
        .into_iter()
        .flat_map(|base| {
            names.iter().map(move |name| {
                if name.starts_with("libvulkan") {
                    format!("{base}/{name}")
                } else {
                    format!("{base}/dri/{name}")
                }
            })
        })
        .collect();
    Some(MesaArtifactRequirement {
        capability: capability.to_owned(),
        alternatives,
    })
}

fn drm_for_card(card: VideoCard) -> Option<DrmDriver> {
    match card {
        VideoCard::Intel => Some(DrmDriver::Intel),
        VideoCard::Amdgpu | VideoCard::Radeonsi => Some(DrmDriver::Amdgpu),
        VideoCard::Radeon => Some(DrmDriver::Radeon),
        VideoCard::Nouveau => Some(DrmDriver::Nouveau),
        VideoCard::Virgl => Some(DrmDriver::VirtioGpu),
        VideoCard::Vmware => Some(DrmDriver::Vmwgfx),
        VideoCard::Lavapipe => None,
    }
}

fn kernel_options(driver: DrmDriver) -> &'static [&'static str] {
    match driver {
        DrmDriver::Intel => &["CONFIG_DRM", "CONFIG_DRM_KMS_HELPER", "CONFIG_DRM_I915"],
        DrmDriver::Amdgpu => &["CONFIG_DRM", "CONFIG_DRM_KMS_HELPER", "CONFIG_DRM_AMDGPU"],
        DrmDriver::Radeon => &["CONFIG_DRM", "CONFIG_DRM_KMS_HELPER", "CONFIG_DRM_RADEON"],
        DrmDriver::Nouveau => &["CONFIG_DRM", "CONFIG_DRM_KMS_HELPER", "CONFIG_DRM_NOUVEAU"],
        DrmDriver::VirtioGpu => &[
            "CONFIG_DRM",
            "CONFIG_DRM_KMS_HELPER",
            "CONFIG_DRM_GEM_SHMEM_HELPER",
            "CONFIG_DRM_VIRTIO_GPU",
            "CONFIG_VIRTIO",
            "CONFIG_VIRTIO_PCI",
        ],
        DrmDriver::Vmwgfx => &["CONFIG_DRM", "CONFIG_DRM_KMS_HELPER", "CONFIG_DRM_VMWGFX"],
    }
}

fn integrated_gpu(cards: &[VideoCard]) -> Option<&'static str> {
    if cards.contains(&VideoCard::Intel) {
        Some("Intel")
    } else if cards.contains(&VideoCard::Amdgpu) || cards.contains(&VideoCard::Radeonsi) {
        Some("AMD")
    } else {
        None
    }
}

fn push_unique<T: PartialEq>(values: &mut Vec<T>, value: T) {
    if !values.contains(&value) {
        values.push(value);
    }
}

fn push_unique_string(values: &mut Vec<String>, value: &str) {
    if !values.iter().any(|existing| existing == value) {
        values.push(value.to_owned());
    }
}

fn debug_list<T: std::fmt::Debug>(values: &[T]) -> String {
    values
        .iter()
        .map(|value| format!("{value:?}"))
        .collect::<Vec<_>>()
        .join(", ")
}

fn decision(
    field: &str,
    value: &str,
    source: DecisionSource,
    reason: impl Into<String>,
    affected: &[&str],
) -> GraphicsDecision {
    GraphicsDecision {
        field: field.to_owned(),
        value: value.to_owned(),
        source,
        reason: reason.into(),
        affected: affected.iter().map(|value| (*value).to_owned()).collect(),
    }
}

fn invalid(message: impl Into<String>) -> GraphicsResolveError {
    GraphicsResolveError::Invalid(message.into())
}

#[cfg(test)]
mod tests {
    use std::time::{SystemTime, UNIX_EPOCH};

    use crate::manifest::{Drm, Hardware, MesaGraphics, Nvidia, VideoCards};

    use super::*;

    #[test]
    fn virgl_policy_derives_mesa_artifact_and_kernel_requirements() {
        let manifest = manifest_with_graphics(Graphics {
            mesa: MesaGraphics {
                video_cards: VideoCards::Explicit(vec![VideoCard::Virgl]),
                ..MesaGraphics::default()
            },
            drm: Drm {
                drivers: DrmDrivers::Auto,
            },
            vm_support: VmGraphics::Virgl,
            ..Graphics::default()
        });

        let resolved = resolve_graphics(&manifest).unwrap();
        assert_eq!(resolved.policy.drm_drivers, [DrmDriver::VirtioGpu]);
        assert!(
            resolved
                .requirements
                .kernel_config
                .contains(&"CONFIG_DRM_VIRTIO_GPU".to_owned())
        );
        assert_eq!(
            resolved.requirements.mesa_artifacts[0].capability,
            "video_cards_virgl"
        );
        assert_eq!(resolved.mesa_build_values(), ["virgl"]);
        assert_eq!(resolved.drm_build_values(), ["virtio_gpu"]);
    }

    #[test]
    fn proprietary_nvidia_and_nouveau_are_mutually_exclusive() {
        let manifest = manifest_with_graphics(Graphics {
            mesa: MesaGraphics {
                video_cards: VideoCards::Explicit(vec![VideoCard::Nouveau]),
                ..MesaGraphics::default()
            },
            nvidia: Some(Nvidia::default()),
            ..Graphics::default()
        });

        assert!(
            resolve_graphics(&manifest)
                .unwrap_err()
                .to_string()
                .contains("conflicts with VideoCard::Nouveau")
        );
    }

    #[test]
    fn offload_requires_and_records_an_integrated_gpu() {
        let manifest = manifest_with_graphics(Graphics {
            mesa: MesaGraphics {
                video_cards: VideoCards::Explicit(vec![VideoCard::Intel]),
                ..MesaGraphics::default()
            },
            nvidia: Some(Nvidia {
                prime: PrimeMode::Offload,
                ..Nvidia::default()
            }),
            ..Graphics::default()
        });

        let resolved = resolve_graphics(&manifest).unwrap();
        let prime = resolved.requirements.prime.unwrap();
        assert_eq!(prime.mode, PrimeMode::Offload);
        assert!(prime.compositor_gpu.contains("Intel"));
        assert!(prime.offload_gpu.unwrap().contains("prime-run"));
    }

    #[test]
    fn source_capability_comparison_accepts_matching_virgl_image() {
        let root = test_root("virgl-capabilities");
        fs::create_dir_all(root.join("usr/lib64/dri")).unwrap();
        fs::create_dir_all(root.join("boot")).unwrap();
        fs::write(root.join("usr/lib64/dri/virtio_gpu_dri.so"), "fixture").unwrap();
        fs::write(
            root.join("boot/config-test"),
            "CONFIG_DRM=y\nCONFIG_DRM_KMS_HELPER=y\nCONFIG_DRM_GEM_SHMEM_HELPER=y\nCONFIG_DRM_VIRTIO_GPU=m\nCONFIG_VIRTIO=y\nCONFIG_VIRTIO_PCI=y\n",
        )
        .unwrap();
        let manifest = manifest_with_graphics(Graphics {
            mesa: MesaGraphics {
                video_cards: VideoCards::Explicit(vec![VideoCard::Virgl]),
                ..MesaGraphics::default()
            },
            vm_support: VmGraphics::Virgl,
            ..Graphics::default()
        });

        let resolved = manifest.resolved_graphics_for_source(&root).unwrap();
        assert!(resolved.capabilities.missing.is_empty());
        assert!(resolved.capabilities.mesa_checked);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn source_capability_comparison_reports_missing_artifact_and_kernel_option() {
        let root = test_root("missing-capabilities");
        fs::create_dir_all(root.join("boot")).unwrap();
        fs::write(root.join("boot/config-test"), "CONFIG_DRM=y\n").unwrap();
        let manifest = manifest_with_graphics(Graphics {
            mesa: MesaGraphics {
                video_cards: VideoCards::Explicit(vec![VideoCard::Virgl]),
                ..MesaGraphics::default()
            },
            vm_support: VmGraphics::Virgl,
            ..Graphics::default()
        });

        let error = manifest
            .resolved_graphics_for_source(&root)
            .unwrap_err()
            .to_string();
        assert!(error.contains("video_cards_virgl"));
        assert!(error.contains("CONFIG_DRM_VIRTIO_GPU"));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn checksummed_image_capability_manifest_is_preferred_over_legacy_probing() {
        let root = test_root("capability-manifest");
        fs::create_dir_all(root.join("usr/lib64/dri")).unwrap();
        fs::create_dir_all(root.join("boot")).unwrap();
        fs::write(root.join("usr/lib64/dri/virtio_gpu_dri.so"), "fixture").unwrap();
        fs::write(root.join("boot/config-test"), "contract fixture").unwrap();
        write_capability_manifest(
            &root,
            r#"format_version = 1

[graphics.mesa]
video_cards = ["video_cards_virgl"]
artifacts = ["usr/lib64/dri/virtio_gpu_dri.so"]

[graphics.kernel]
config_path = "boot/config-test"
enabled = ["CONFIG_DRM", "CONFIG_DRM_KMS_HELPER", "CONFIG_DRM_GEM_SHMEM_HELPER", "CONFIG_DRM_VIRTIO_GPU", "CONFIG_VIRTIO", "CONFIG_VIRTIO_PCI"]
[graphics.vm]
virgl = true
virgl_launch_device = "virtio-vga-gl"
"#,
        );
        let manifest = manifest_with_graphics(Graphics {
            mesa: MesaGraphics {
                video_cards: VideoCards::Explicit(vec![VideoCard::Virgl]),
                ..MesaGraphics::default()
            },
            vm_support: VmGraphics::Virgl,
            ..Graphics::default()
        });

        let resolved = manifest.resolved_graphics_for_source(&root).unwrap();
        assert!(resolved.capabilities.capability_manifest_checksum_verified);
        assert!(resolved.capabilities.capability_manifest_path.is_some());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn tampered_image_capability_manifest_is_rejected() {
        let root = test_root("tampered-capability-manifest");
        write_capability_manifest(
            &root,
            "format_version = 1\n[graphics.mesa]\nvideo_cards = []\n",
        );
        fs::write(
            root.join(IMAGE_CAPABILITY_MANIFEST),
            "format_version = 1\n[graphics.mesa]\nvideo_cards = [\"video_cards_virgl\"]\n",
        )
        .unwrap();
        let manifest = manifest_with_graphics(Graphics {
            mesa: MesaGraphics {
                video_cards: VideoCards::Explicit(vec![VideoCard::Virgl]),
                ..MesaGraphics::default()
            },
            vm_support: VmGraphics::Virgl,
            ..Graphics::default()
        });

        let error = manifest
            .resolved_graphics_for_source(&root)
            .unwrap_err()
            .to_string();
        assert!(error.contains("checksum mismatch"));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn retired_gpu_field_is_reported_as_legacy_inference() {
        let manifest: SystemManifest = toml::from_str(
            r#"
                [hardware]
                gpu = { igpu = "intel", dgpu = "nvidia" }
            "#,
        )
        .unwrap();

        let resolved = resolve_graphics(&manifest).unwrap();
        assert!(resolved.decisions.iter().any(|decision| {
            decision.field == "hardware.gpu"
                && decision.source == DecisionSource::LegacyInference
                && decision.value.contains("Offload")
        }));
    }

    #[test]
    fn proprietary_nvidia_capability_requires_driver_and_matching_kernel_module() {
        let root = test_root("nvidia-capability");
        fs::create_dir_all(root.join("boot")).unwrap();
        fs::create_dir_all(root.join("var/db/pkg/x11-drivers/nvidia-drivers-580.1")).unwrap();
        fs::create_dir_all(root.join("lib/modules/6.12-test/extra")).unwrap();
        fs::write(root.join("boot/vmlinuz-6.12-test"), "fixture").unwrap();
        fs::write(
            root.join("lib/modules/6.12-test/extra/nvidia_drm.ko.zst"),
            "fixture",
        )
        .unwrap();
        let manifest = manifest_with_graphics(Graphics {
            nvidia: Some(Nvidia {
                prime: PrimeMode::Primary,
                ..Nvidia::default()
            }),
            ..Graphics::default()
        });

        let resolved = manifest.resolved_graphics_for_source(&root).unwrap();
        assert_eq!(
            resolved.capabilities.nvidia_driver_version.as_deref(),
            Some("580.1")
        );
        assert_eq!(
            resolved.capabilities.nvidia_kernel_abi.as_deref(),
            Some("6.12-test")
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn render_node_detection_maps_drm_nodes_to_pci_vendors() {
        let root = test_root("render-nodes");
        fs::create_dir_all(root.join("renderD128/device")).unwrap();
        fs::create_dir_all(root.join("renderD129/device")).unwrap();
        fs::write(root.join("renderD128/device/vendor"), "0x8086\n").unwrap();
        fs::write(root.join("renderD129/device/vendor"), "0x10DE\n").unwrap();

        let nodes = detect_render_nodes(&root);
        assert_eq!(nodes[0].path, "/dev/dri/renderD128");
        assert_eq!(nodes[0].vendor, "0x8086");
        assert_eq!(nodes[1].vendor, "0x10de");
        fs::remove_dir_all(root).unwrap();
    }

    fn manifest_with_graphics(graphics: Graphics) -> SystemManifest {
        SystemManifest {
            hardware: Hardware {
                graphics,
                ..Hardware::default()
            },
            ..SystemManifest::default()
        }
    }

    fn test_root(name: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("oxys_graphics_{name}_{nanos}"))
    }

    fn write_capability_manifest(root: &Path, contents: &str) {
        let path = root.join(IMAGE_CAPABILITY_MANIFEST);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, contents).unwrap();
        fs::write(
            root.join(IMAGE_CAPABILITY_CHECKSUM),
            format!(
                "{}  image-capabilities.toml\n",
                crate::util::sha256_hex(contents.as_bytes())
            ),
        )
        .unwrap();
    }
}
