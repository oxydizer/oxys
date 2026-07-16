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

mod capability;
mod resolved;

#[cfg(test)]
use capability::{IMAGE_CAPABILITY_CHECKSUM, IMAGE_CAPABILITY_MANIFEST};
use capability::{
    compare_capability_manifest, compare_nvidia_capability, detect_render_nodes,
    find_kernel_config, parse_enabled_kernel_options,
};
use resolved::{build_requirements, validate_card_driver_pairs, validate_nvidia_policy};

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
mod tests;
