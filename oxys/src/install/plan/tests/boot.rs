use super::*;

#[test]
fn system_install_plan_uses_custom_efi_mount() {
    let temp = TempTree::new("custom-efi");
    let source = temp.path().join("source");
    let target = temp.path().join("target");
    fs::create_dir_all(source.join("boot")).unwrap();
    fs::create_dir_all(&target).unwrap();

    let manifest = SystemManifest {
        disk: Disk {
            device: "/dev/vda".to_owned(),
            layout: DiskLayout::Ext4,
            partitions: DiskPartitions {
                efi: EfiPartition {
                    size: 512 * MB,
                    mount: "/efi".to_owned(),
                },
                ..DiskPartitions::default()
            },
            ext4: Ext4Options {
                separate_home: false,
                root_size: 32 * GB,
            },
            ..Disk::default()
        },
        bootloader: Some(crate::manifest::Bootloader::SystemdBoot),
        ..SystemManifest::default()
    };

    let plan = plan_system_install(&manifest, &source, &target, None).unwrap();
    assert!(plan.render().contains("--esp-path"));
    assert!(plan.render().contains("/efi"));
    assert!(matches!(
        plan.steps.iter().rev().nth(4), // ..., <step>, SetupLogin, GraphicsRuntime, SeedOxysConfig, Finalize
        Some(SystemInstallStep::GenerateSystemdBoot { .. })
    ));
}

#[test]
fn grub_bootloader_replaces_systemd_boot_steps() {
    let temp = TempTree::new("grub-bootloader");
    let source = temp.path().join("source");
    let target = temp.path().join("target");
    fs::create_dir_all(source.join("boot")).unwrap();
    fs::create_dir_all(&target).unwrap();

    let manifest = SystemManifest {
        disk: Disk {
            device: "/dev/vda".to_owned(),
            layout: DiskLayout::Ext4,
            ..Disk::default()
        },
        bootloader: Some(crate::manifest::Bootloader::Grub),
        ..SystemManifest::default()
    };

    let plan = plan_system_install(&manifest, &source, &target, None).unwrap();
    let rendered = plan.render();
    assert!(rendered.contains("grub-install"));
    assert!(rendered.contains("--removable"));
    assert!(!rendered.contains("bootctl"));
    assert!(
        plan.steps
            .iter()
            .any(|step| matches!(step, SystemInstallStep::GenerateGrubConfig { .. }))
    );
    assert!(
        !plan
            .steps
            .iter()
            .any(|step| matches!(step, SystemInstallStep::GenerateSystemdBoot { .. }))
    );
}

#[test]
fn kernel_cmdline_conflicts_are_rejected_before_a_plan_is_returned() {
    let temp = TempTree::new("kernel-cmdline-conflict");
    let source = temp.path().join("source");
    let target = temp.path().join("target");
    fs::create_dir_all(source.join("boot")).unwrap();
    fs::create_dir_all(&target).unwrap();

    let manifest = SystemManifest {
        hardware: crate::manifest::Hardware {
            graphics: crate::manifest::Graphics {
                nvidia: Some(crate::manifest::Nvidia::default()),
                ..crate::manifest::Graphics::default()
            },
            ..crate::manifest::Hardware::default()
        },
        kernel: crate::manifest::Kernel {
            cmdline: vec!["nvidia_drm.modeset=0".to_owned()],
        },
        ..SystemManifest::default()
    };

    let error = plan_system_install(&manifest, &source, &target, None).unwrap_err();
    assert!(error.to_string().contains("conflicting kernel arguments"));
    assert!(
        error
            .to_string()
            .contains("hardware.graphics.nvidia.modeset")
    );
}

#[test]
fn graphics_capabilities_are_validated_and_rendered_before_copy() {
    let temp = TempTree::new("graphics-capabilities");
    let source = temp.path().join("source");
    let target = temp.path().join("target");
    fs::create_dir_all(source.join("boot")).unwrap();
    fs::create_dir_all(source.join("usr/lib64/dri")).unwrap();
    fs::create_dir_all(&target).unwrap();
    fs::write(source.join("usr/lib64/dri/virtio_gpu_dri.so"), "fixture").unwrap();
    fs::write(
            source.join("boot/config-test"),
            "CONFIG_DRM=y\nCONFIG_DRM_KMS_HELPER=y\nCONFIG_DRM_GEM_SHMEM_HELPER=y\nCONFIG_DRM_VIRTIO_GPU=m\nCONFIG_VIRTIO=y\nCONFIG_VIRTIO_PCI=y\n",
        )
        .unwrap();
    let manifest = SystemManifest {
        hardware: crate::manifest::Hardware {
            graphics: crate::manifest::Graphics {
                mesa: crate::manifest::MesaGraphics {
                    video_cards: crate::manifest::VideoCards::Explicit(vec![
                        crate::manifest::VideoCard::Virgl,
                    ]),
                    ..crate::manifest::MesaGraphics::default()
                },
                vm_support: crate::manifest::VmGraphics::Virgl,
                ..crate::manifest::Graphics::default()
            },
            ..crate::manifest::Hardware::default()
        },
        ..SystemManifest::default()
    };

    let plan = plan_system_install(&manifest, &source, &target, None).unwrap();
    assert!(matches!(
        plan.steps.get(1),
        Some(SystemInstallStep::ResolveGraphics { .. })
    ));
    let rendered = plan.render();
    assert!(rendered.contains("Mesa capability check: passed"));
    assert!(rendered.contains("boot/config-test"));
    assert!(rendered.find("graphics policy:").unwrap() < rendered.find("rsync").unwrap());
}

#[test]
fn missing_graphics_capability_rejects_install_plan() {
    let temp = TempTree::new("missing-graphics-capability");
    let source = temp.path().join("source");
    let target = temp.path().join("target");
    fs::create_dir_all(source.join("boot")).unwrap();
    fs::create_dir_all(&target).unwrap();
    fs::write(source.join("boot/config-test"), "CONFIG_DRM=y\n").unwrap();
    let manifest = SystemManifest {
        hardware: crate::manifest::Hardware {
            graphics: crate::manifest::Graphics {
                mesa: crate::manifest::MesaGraphics {
                    video_cards: crate::manifest::VideoCards::Explicit(vec![
                        crate::manifest::VideoCard::Virgl,
                    ]),
                    ..crate::manifest::MesaGraphics::default()
                },
                vm_support: crate::manifest::VmGraphics::Virgl,
                ..crate::manifest::Graphics::default()
            },
            ..crate::manifest::Hardware::default()
        },
        ..SystemManifest::default()
    };

    let error = plan_system_install(&manifest, &source, &target, None).unwrap_err();
    assert!(error.to_string().contains("video_cards_virgl"));
    assert!(error.to_string().contains("CONFIG_DRM_VIRTIO_GPU"));
}

#[test]
fn grub_is_the_default_bootloader() {
    let temp = TempTree::new("default-bootloader");
    let source = temp.path().join("source");
    let target = temp.path().join("target");
    fs::create_dir_all(source.join("boot")).unwrap();
    fs::create_dir_all(&target).unwrap();

    let manifest = SystemManifest {
        disk: Disk {
            device: "/dev/vda".to_owned(),
            layout: DiskLayout::Ext4,
            ..Disk::default()
        },
        ..SystemManifest::default()
    };

    let plan = plan_system_install(&manifest, &source, &target, None).unwrap();
    assert!(
        plan.steps
            .iter()
            .any(|step| matches!(step, SystemInstallStep::GenerateGrubConfig { .. }))
    );
}

#[test]
fn openrc_manifest_adds_symlink_service_activation() {
    let temp = TempTree::new("openrc-services");
    let source = temp.path().join("source");
    let target = temp.path().join("target");
    fs::create_dir_all(source.join("boot")).unwrap();
    fs::create_dir_all(&target).unwrap();

    let manifest = SystemManifest {
        disk: Disk {
            device: "/dev/vda".to_owned(),
            layout: DiskLayout::Ext4,
            ..Disk::default()
        },
        init_system: InitSystem::Openrc,
        services: crate::manifest::Services {
            enabled: vec!["NetworkManager".to_owned()],
            disabled: vec!["sshd".to_owned()],
        },
        ..SystemManifest::default()
    };

    let plan = plan_system_install(&manifest, &source, &target, None).unwrap();
    assert!(matches!(
        plan.steps.iter().rev().nth(4), // ..., <step>, SetupLogin, GraphicsRuntime, SeedOxysConfig, Finalize
        Some(SystemInstallStep::ActivateOpenrcServices { .. })
    ));
    assert!(plan.render().contains("apply openrc service state"));
    assert!(
        !plan
            .steps
            .iter()
            .any(|step| matches!(step, SystemInstallStep::ActivateSystemdServices { .. }))
    );
}

#[test]
fn openrc_service_activation_manages_runlevel_symlinks() {
    let temp = TempTree::new("openrc-symlinks");
    let target = temp.path().join("target");
    let runlevel_dir = target.join("etc/runlevels/default");
    // A stale entry that should be removed by the disable pass.
    fs::create_dir_all(&runlevel_dir).unwrap();
    std::os::unix::fs::symlink("/etc/init.d/sshd", runlevel_dir.join("sshd")).unwrap();

    let manifest = SystemManifest {
        services: crate::manifest::Services {
            enabled: vec!["NetworkManager".to_owned()],
            disabled: vec!["sshd".to_owned()],
        },
        ..SystemManifest::default()
    };

    let (sender, _receiver) = mpsc::channel();
    services::activate_openrc_services(&manifest, &target, &sender).unwrap();

    let enabled_link = runlevel_dir.join("NetworkManager");
    assert_eq!(
        fs::read_link(&enabled_link).unwrap(),
        Path::new("/etc/init.d/NetworkManager")
    );
    assert!(fs::symlink_metadata(runlevel_dir.join("sshd")).is_err());
}

#[test]
fn zfs_openrc_services_are_implicit_boot_runlevel_links() {
    let temp = TempTree::new("zfs-openrc-symlinks");
    let target = temp.path().join("target");

    let manifest = SystemManifest {
        disk: Disk {
            device: "/dev/vda".to_owned(),
            layout: DiskLayout::Zfs,
            ..Disk::default()
        },
        init_system: InitSystem::Openrc,
        ..SystemManifest::default()
    };

    let (sender, _receiver) = mpsc::channel();
    services::activate_openrc_services(&manifest, &target, &sender).unwrap();

    let boot_runlevel = target.join("etc/runlevels/boot");
    assert_eq!(
        fs::read_link(boot_runlevel.join("zfs-import")).unwrap(),
        Path::new("/etc/init.d/zfs-import")
    );
    assert_eq!(
        fs::read_link(boot_runlevel.join("zfs-mount")).unwrap(),
        Path::new("/etc/init.d/zfs-mount")
    );
    assert!(fs::symlink_metadata(target.join("etc/runlevels/default/zfs-import")).is_err());
    assert!(fs::symlink_metadata(target.join("etc/runlevels/default/zfs-mount")).is_err());
}

#[test]
fn explicit_systemd_manifest_adds_service_activation_step() {
    let temp = TempTree::new("systemd-services");
    let source = temp.path().join("source");
    let target = temp.path().join("target");
    fs::create_dir_all(source.join("boot")).unwrap();
    fs::create_dir_all(&target).unwrap();

    let manifest = SystemManifest {
        disk: Disk {
            device: "/dev/vda".to_owned(),
            layout: DiskLayout::Ext4,
            ..Disk::default()
        },
        init_system: InitSystem::Systemd,
        services: crate::manifest::Services {
            enabled: vec!["systemd-networkd.service".to_owned()],
            disabled: vec!["sshd.service".to_owned()],
        },
        ..SystemManifest::default()
    };

    let plan = plan_system_install(&manifest, &source, &target, None).unwrap();
    assert!(matches!(
        plan.steps.iter().rev().nth(4), // ..., <step>, SetupLogin, GraphicsRuntime, SeedOxysConfig, Finalize
        Some(SystemInstallStep::ActivateSystemdServices { .. })
    ));
    assert!(plan.render().contains("1 enable, 1 disable"));
}

#[test]
fn zfs_system_install_plan_generates_initramfs_step() {
    let temp = TempTree::new("zfs-initramfs");
    let source = temp.path().join("source");
    let target = temp.path().join("target");
    fs::create_dir_all(source.join("boot")).unwrap();
    fs::create_dir_all(&target).unwrap();
    fs::write(source.join("boot/vmlinuz-6.6.21-gentoo"), "mock-kernel").unwrap();

    let manifest = SystemManifest {
        disk: Disk {
            device: "/dev/vda".to_owned(),
            layout: DiskLayout::Zfs,
            ..Disk::default()
        },
        bootloader: Some(crate::manifest::Bootloader::Grub),
        ..SystemManifest::default()
    };

    let plan = plan_system_install(&manifest, &source, &target, None).unwrap();
    let rendered = plan.render();
    assert!(rendered.contains("generate ZFS-root initramfs for kernel 6.6.21-gentoo"));
    assert!(plan
            .steps
            .iter()
            .any(|step| matches!(step, SystemInstallStep::GenerateInitramfs { kver, .. } if kver == "6.6.21-gentoo")));
}
