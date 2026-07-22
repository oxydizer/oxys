use super::*;

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

pub(super) fn build_requirements(
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
        if let Some(artifact) = mesa_artifact_requirement(*card)
            && !requirements
                .mesa_artifacts
                .iter()
                .any(|existing| existing.capability == artifact.capability)
        {
            requirements.mesa_artifacts.push(artifact);
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

pub(super) fn validate_card_driver_pairs(
    cards: &[VideoCard],
    drivers: &[DrmDriver],
) -> Result<(), GraphicsResolveError> {
    for card in cards {
        if let Some(driver) = drm_for_card(*card)
            && !drivers.contains(&driver)
        {
            return Err(invalid(format!(
                "hardware.graphics Mesa card {card:?} requires DRM driver {driver:?}"
            )));
        }
    }
    Ok(())
}

pub(super) fn validate_nvidia_policy(
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

pub(super) fn compare_source_capabilities(
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
