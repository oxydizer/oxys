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
