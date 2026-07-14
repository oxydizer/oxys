use std::{
    fs,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
};

use thiserror::Error;

use crate::manifest::{
    Graphics, NvidiaDriver, PrimeMode as GraphicsPrimeMode, SystemManifest, VideoCard, VideoCards,
};
const PRIME_ENV_PATH: &str = "etc/environment.d/90-oxys-nvidia-primary.conf";
const PRIME_PROFILE_PATH: &str = "etc/profile.d/oxys-nvidia-primary.sh";
const LEGACY_PRIME_ENV_PATH: &str = "etc/environment.d/90-oxys-prime-offload.conf";
const LEGACY_PRIME_PROFILE_PATH: &str = "etc/profile.d/oxys-prime-offload.sh";
const PRIME_MODPROBE_PATH: &str = "etc/modprobe.d/oxys-nvidia-prime.conf";
const NVIDIA_BLACKLIST_PATH: &str = "etc/modprobe.d/oxys-nvidia-stack.conf";
const PRIME_RUN_PATH: &str = "usr/local/bin/prime-run";
const GRAPHICS_DIAGNOSTICS_PATH: &str = "usr/local/bin/oxys-graphics-diagnostics";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeConfigOutcome {
    pub prime_offload_configured: bool,
    pub prime_primary_configured: bool,
    pub graphics_diagnostics_configured: bool,
}

#[derive(Debug, Error)]
pub enum RuntimeConfigError {
    #[error("failed to create directory {path}: {source}")]
    CreateDir {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("failed to write runtime config {path}: {source}")]
    WriteFile {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("failed to set permissions on {path}: {source}")]
    SetPermissions {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("failed to remove stale runtime config {path}: {source}")]
    RemoveFile {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

pub fn sync_runtime_config(
    manifest: &SystemManifest,
    root: &Path,
) -> Result<RuntimeConfigOutcome, RuntimeConfigError> {
    let mode = prime_mode(&manifest.hardware.graphics);
    let proprietary_modeset = manifest
        .hardware
        .graphics
        .nvidia
        .is_some_and(|nvidia| nvidia.driver == NvidiaDriver::Proprietary && nvidia.modeset);
    let files = [
        GeneratedFile {
            path: root.join(PRIME_ENV_PATH),
            contents: (mode == Some(PrimeMode::Primary)).then_some(render_prime_environment()),
            mode: 0o644,
        },
        GeneratedFile {
            path: root.join(PRIME_PROFILE_PATH),
            contents: (mode == Some(PrimeMode::Primary)).then_some(render_prime_profile()),
            mode: 0o644,
        },
        GeneratedFile {
            path: root.join(PRIME_MODPROBE_PATH),
            contents: proprietary_modeset.then_some(render_prime_modprobe()),
            mode: 0o644,
        },
        GeneratedFile {
            path: root.join(PRIME_RUN_PATH),
            contents: mode.and_then(|mode| match mode {
                PrimeMode::OffloadIntel | PrimeMode::OffloadAmd => Some(render_prime_run(mode)),
                PrimeMode::Primary => None,
            }),
            mode: 0o755,
        },
        GeneratedFile {
            path: root.join(NVIDIA_BLACKLIST_PATH),
            contents: render_nvidia_blacklist(&manifest.hardware.graphics),
            mode: 0o644,
        },
        GeneratedFile {
            path: root.join(GRAPHICS_DIAGNOSTICS_PATH),
            contents: Some(GRAPHICS_DIAGNOSTICS),
            mode: 0o755,
        },
    ];

    for file in files {
        match file.contents {
            Some(contents) => {
                write_generated_file(&file.path, contents, file.mode)?;
            }
            None => remove_if_exists(&file.path)?,
        }
    }
    remove_if_exists(&root.join(LEGACY_PRIME_ENV_PATH))?;
    remove_if_exists(&root.join(LEGACY_PRIME_PROFILE_PATH))?;

    Ok(RuntimeConfigOutcome {
        prime_offload_configured: matches!(
            mode,
            Some(PrimeMode::OffloadIntel | PrimeMode::OffloadAmd)
        ),
        prime_primary_configured: mode == Some(PrimeMode::Primary),
        graphics_diagnostics_configured: true,
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PrimeMode {
    Primary,
    OffloadIntel,
    OffloadAmd,
}

struct GeneratedFile {
    path: PathBuf,
    contents: Option<&'static str>,
    mode: u32,
}

fn prime_mode(graphics: &Graphics) -> Option<PrimeMode> {
    let nvidia = graphics.nvidia?;
    if nvidia.driver != NvidiaDriver::Proprietary {
        return None;
    }
    if nvidia.prime == GraphicsPrimeMode::Primary {
        return Some(PrimeMode::Primary);
    }
    if nvidia.prime != GraphicsPrimeMode::Offload {
        return None;
    }
    match &graphics.mesa.video_cards {
        VideoCards::Explicit(cards) if cards.contains(&VideoCard::Intel) => {
            Some(PrimeMode::OffloadIntel)
        }
        VideoCards::Explicit(cards) if cards.contains(&VideoCard::Amdgpu) => {
            Some(PrimeMode::OffloadAmd)
        }
        _ => None,
    }
}

fn render_prime_environment() -> &'static str {
    concat!(
        "# THIS FILE IS AUTO-GENERATED BY OXYS - DO NOT EDIT MANUALLY\n",
        "# NVIDIA owns the primary rendering path\n",
        "__NV_PRIME_RENDER_OFFLOAD=1\n",
        "__GLX_VENDOR_LIBRARY_NAME=nvidia\n",
        "__VK_LAYER_NV_optimus=NVIDIA_only\n",
        "LIBVA_DRIVER_NAME=nvidia\n"
    )
}

fn render_prime_profile() -> &'static str {
    concat!(
        "#!/bin/sh\n",
        "# THIS FILE IS AUTO-GENERATED BY OXYS - DO NOT EDIT MANUALLY\n",
        "# NVIDIA owns the primary rendering path\n",
        "export __NV_PRIME_RENDER_OFFLOAD=1\n",
        "export __GLX_VENDOR_LIBRARY_NAME=nvidia\n",
        "export __VK_LAYER_NV_optimus=NVIDIA_only\n",
        "export LIBVA_DRIVER_NAME=nvidia\n"
    )
}

fn render_prime_modprobe() -> &'static str {
    concat!(
        "# THIS FILE IS AUTO-GENERATED BY OXYS - DO NOT EDIT MANUALLY\n",
        "# Proprietary NVIDIA DRM KMS policy\n",
        "options nvidia-drm modeset=1\n"
    )
}

fn render_prime_run(mode: PrimeMode) -> &'static str {
    match mode {
        PrimeMode::OffloadIntel => concat!(
            "#!/bin/sh\n",
            "# THIS FILE IS AUTO-GENERATED BY OXYS - DO NOT EDIT MANUALLY\n",
            "# Intel iGPU + NVIDIA dGPU PRIME wrapper\n",
            "export __NV_PRIME_RENDER_OFFLOAD=1\n",
            "export __GLX_VENDOR_LIBRARY_NAME=nvidia\n",
            "export __VK_LAYER_NV_optimus=NVIDIA_only\n",
            "export LIBVA_DRIVER_NAME=nvidia\n",
            "exec \"$@\"\n"
        ),
        PrimeMode::OffloadAmd => concat!(
            "#!/bin/sh\n",
            "# THIS FILE IS AUTO-GENERATED BY OXYS - DO NOT EDIT MANUALLY\n",
            "# AMD iGPU + NVIDIA dGPU PRIME wrapper\n",
            "export __NV_PRIME_RENDER_OFFLOAD=1\n",
            "export __GLX_VENDOR_LIBRARY_NAME=nvidia\n",
            "export __VK_LAYER_NV_optimus=NVIDIA_only\n",
            "export LIBVA_DRIVER_NAME=nvidia\n",
            "exec \"$@\"\n"
        ),
        PrimeMode::Primary => unreachable!("primary mode does not generate prime-run"),
    }
}

fn render_nvidia_blacklist(graphics: &Graphics) -> Option<&'static str> {
    match graphics.nvidia?.driver {
        NvidiaDriver::Proprietary => Some(concat!(
            "# THIS FILE IS AUTO-GENERATED BY OXYS - DO NOT EDIT MANUALLY\n",
            "blacklist nouveau\n",
            "options nouveau modeset=0\n"
        )),
        NvidiaDriver::Nouveau => Some(concat!(
            "# THIS FILE IS AUTO-GENERATED BY OXYS - DO NOT EDIT MANUALLY\n",
            "blacklist nvidia\n",
            "blacklist nvidia_drm\n",
            "blacklist nvidia_modeset\n",
            "blacklist nvidia_uvm\n"
        )),
    }
}

const GRAPHICS_DIAGNOSTICS: &str = r#"#!/bin/sh
# THIS FILE IS AUTO-GENERATED BY OXYS - DO NOT EDIT MANUALLY
echo "LIBSEAT_BACKEND=${LIBSEAT_BACKEND:-auto}"
echo "DRM nodes:"
ls -l /dev/dri/card* /dev/dri/renderD* 2>/dev/null || echo "  none"
echo "DRM modules:"
lsmod 2>/dev/null | grep -E '^(i915|xe|amdgpu|radeon|nouveau|virtio_gpu|vmwgfx|nvidia)' || echo "  none detected"
if command -v glxinfo >/dev/null 2>&1; then
    glxinfo -B 2>/dev/null | grep -E 'OpenGL vendor|OpenGL renderer' || true
elif command -v vulkaninfo >/dev/null 2>&1; then
    vulkaninfo --summary 2>/dev/null | sed -n '/Devices:/,$p'
else
    echo "Mesa renderer: install glxinfo or vulkaninfo for userspace diagnostics"
fi
"#;

fn write_generated_file(path: &Path, contents: &str, mode: u32) -> Result<(), RuntimeConfigError> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|source| RuntimeConfigError::CreateDir {
            path: parent.to_path_buf(),
            source,
        })?;
    }

    fs::write(path, contents).map_err(|source| RuntimeConfigError::WriteFile {
        path: path.to_path_buf(),
        source,
    })?;
    fs::set_permissions(path, fs::Permissions::from_mode(mode)).map_err(|source| {
        RuntimeConfigError::SetPermissions {
            path: path.to_path_buf(),
            source,
        }
    })?;
    Ok(())
}

fn remove_if_exists(path: &Path) -> Result<(), RuntimeConfigError> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(source) => Err(RuntimeConfigError::RemoveFile {
            path: path.to_path_buf(),
            source,
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manifest::GpuVendor;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn offload_writes_only_launcher_and_modeset_not_global_environment(
    ) -> Result<(), Box<dyn std::error::Error>> {
        let root = test_root("amd_nvidia_prime");
        let manifest = SystemManifest {
            hardware: crate::manifest::Hardware {
                graphics: Graphics::from(crate::manifest::Gpu::Hybrid {
                    igpu: GpuVendor::Amd,
                    dgpu: GpuVendor::Nvidia,
                }),
                ..crate::manifest::Hardware::default()
            },
            ..SystemManifest::default()
        };

        let outcome = sync_runtime_config(&manifest, &root)?;

        assert!(outcome.prime_offload_configured);
        assert!(!outcome.prime_primary_configured);
        assert!(!root.join(PRIME_ENV_PATH).exists());
        assert!(!root.join(PRIME_PROFILE_PATH).exists());
        assert!(fs::read_to_string(root.join(PRIME_MODPROBE_PATH))?.contains("modeset=1"));
        let launcher = fs::read_to_string(root.join(PRIME_RUN_PATH))?;
        assert!(launcher.contains("__NV_PRIME_RENDER_OFFLOAD=1"));
        assert!(launcher.contains("exec \"$@\""));
        assert!(root.join(GRAPHICS_DIAGNOSTICS_PATH).exists());

        cleanup(&root)?;
        Ok(())
    }

    #[test]
    fn primary_writes_global_environment_without_offload_launcher(
    ) -> Result<(), Box<dyn std::error::Error>> {
        let root = test_root("nvidia_primary");
        let manifest = SystemManifest {
            hardware: crate::manifest::Hardware {
                graphics: Graphics {
                    nvidia: Some(crate::manifest::Nvidia {
                        prime: GraphicsPrimeMode::Primary,
                        ..crate::manifest::Nvidia::default()
                    }),
                    ..Graphics::default()
                },
                ..crate::manifest::Hardware::default()
            },
            ..SystemManifest::default()
        };

        let outcome = sync_runtime_config(&manifest, &root)?;

        assert!(outcome.prime_primary_configured);
        assert!(!outcome.prime_offload_configured);
        assert!(fs::read_to_string(root.join(PRIME_ENV_PATH))?
            .contains("__NV_PRIME_RENDER_OFFLOAD=1"));
        assert!(!root.join(PRIME_RUN_PATH).exists());

        cleanup(&root)?;
        Ok(())
    }

    #[test]
    fn removes_prime_files_when_gpu_is_not_nvidia_hybrid() -> Result<(), Box<dyn std::error::Error>>
    {
        let root = test_root("prime_cleanup");
        let hybrid = SystemManifest {
            hardware: crate::manifest::Hardware {
                graphics: Graphics::from(crate::manifest::Gpu::Hybrid {
                    igpu: GpuVendor::Intel,
                    dgpu: GpuVendor::Nvidia,
                }),
                ..crate::manifest::Hardware::default()
            },
            ..SystemManifest::default()
        };
        let single = SystemManifest {
            hardware: crate::manifest::Hardware {
                graphics: Graphics::from(crate::manifest::Gpu::Single(GpuVendor::Intel)),
                ..crate::manifest::Hardware::default()
            },
            ..SystemManifest::default()
        };

        sync_runtime_config(&hybrid, &root)?;
        let outcome = sync_runtime_config(&single, &root)?;

        assert!(!outcome.prime_offload_configured);
        assert!(!root.join(PRIME_ENV_PATH).exists());
        assert!(!root.join(PRIME_PROFILE_PATH).exists());
        assert!(!root.join(PRIME_MODPROBE_PATH).exists());
        assert!(!root.join(PRIME_RUN_PATH).exists());

        cleanup(&root)?;
        Ok(())
    }

    #[test]
    fn does_not_configure_prime_for_non_nvidia_hybrid() -> Result<(), Box<dyn std::error::Error>> {
        let root = test_root("intel_amd_hybrid");
        let manifest = SystemManifest {
            hardware: crate::manifest::Hardware {
                graphics: Graphics::from(crate::manifest::Gpu::Hybrid {
                    igpu: GpuVendor::Intel,
                    dgpu: GpuVendor::Amd,
                }),
                ..crate::manifest::Hardware::default()
            },
            ..SystemManifest::default()
        };

        let outcome = sync_runtime_config(&manifest, &root)?;

        assert!(!outcome.prime_offload_configured);
        assert!(!root.join(PRIME_ENV_PATH).exists());

        cleanup(&root)?;
        Ok(())
    }

    fn test_root(name: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time should move forward")
            .as_nanos();
        std::env::temp_dir().join(format!("oxys_runtime_{name}_{nanos}"))
    }

    fn cleanup(path: &Path) -> Result<(), Box<dyn std::error::Error>> {
        if path.exists() {
            fs::remove_dir_all(path)?;
        }
        Ok(())
    }
}
