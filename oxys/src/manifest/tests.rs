use super::*;

#[test]
fn native_march_has_no_binhost_url() {
    assert_eq!(March::Native.binhost_url(), None);
}

#[test]
fn baseline_marches_map_to_short_oxys_binhost_urls() {
    assert_eq!(
        March::X86_64.binhost_url().as_deref(),
        Some("https://packages.oxysos.org/x86-64/ https://kernel.oxysos.org/x86-64/")
    );
    assert_eq!(
        March::X86_64V3.binhost_url().as_deref(),
        Some("https://packages.oxysos.org/x86-64-v3/ https://kernel.oxysos.org/x86-64-v3/")
    );
}

#[test]
fn compiler_default_binhost_follows_default_march() {
    let compiler = Compiler::default();
    assert_eq!(compiler.binhost, compiler.march.binhost_url());
}

#[test]
fn user_builder_populates_expected_fields() {
    let user = User::new("testuser")
        .wheel()
        .groups(["video", "audio"])
        .wheel()
        .shell(Shell::Zsh)
        .password(Password::Prompt);

    assert_eq!(user.name, Username::Literal("testuser".into()));
    assert_eq!(user.shell, Shell::Zsh);
    assert_eq!(user.password, Password::Prompt);
    // groups() replaces, then wheel() appends without duplicating.
    assert_eq!(user.groups, vec!["video", "audio", "wheel"]);
    assert!(user.is_wheel());
}

#[test]
fn prompt_users_lists_only_prompt_passwords() {
    let manifest = SystemManifest {
        users: vec![
            User::new("root").password(Password::Hashed("$6$x".into())),
            User::new("testuser").password(Password::Prompt),
            User::new("guest").password(Password::None),
            User::new("dev").password(Password::Prompt),
        ],
        ..SystemManifest::default()
    };
    assert_eq!(manifest.prompt_users(), vec!["testuser", "dev"]);
}

#[test]
fn prompt_usernames_lists_only_prompt_names() {
    let manifest = SystemManifest {
        users: vec![
            User::new("root"),
            User::prompt().password(Password::Prompt),
            User::new("guest"),
            User::prompt(),
        ],
        ..SystemManifest::default()
    };
    assert_eq!(manifest.prompt_usernames(), vec![1, 3]);
}

#[test]
fn timezone_from_str_builds_a_literal() {
    let os = Os {
        timezone: "Europe/London".into(),
        ..Os::default()
    };
    assert_eq!(os.timezone, Timezone::Literal("Europe/London".into()));
    assert_eq!(os.timezone.as_str(), "Europe/London");
    assert!(
        !SystemManifest {
            os,
            ..SystemManifest::default()
        }
        .prompts_timezone()
    );
}

#[test]
fn prompt_timezone_is_reported_and_round_trips_through_toml() {
    let manifest = SystemManifest {
        os: Os {
            timezone: Timezone::Prompt,
            ..Os::default()
        },
        ..SystemManifest::default()
    };
    assert!(manifest.prompts_timezone());

    let toml = toml::to_string(&manifest.os).expect("serialise os");
    assert!(toml.contains("timezone = \"prompt\""), "got: {toml}");
    let parsed: Os = toml::from_str(&toml).expect("deserialise os");
    assert_eq!(parsed.timezone, Timezone::Prompt);
}

#[test]
fn literal_timezone_stays_a_plain_toml_string() {
    // Manifests written before Timezone existed store a bare string; the
    // literal form must keep serialising exactly that way.
    let os = Os {
        timezone: "Europe/London".into(),
        ..Os::default()
    };
    let toml = toml::to_string(&os).expect("serialise os");
    assert!(toml.contains("timezone = \"Europe/London\""), "got: {toml}");

    let parsed: Os = toml::from_str("timezone = \"Europe/London\"").expect("deserialise os");
    assert_eq!(parsed.timezone, Timezone::Literal("Europe/London".into()));
}

#[test]
fn password_warnings_flag_plaintext_only() {
    let manifest = SystemManifest {
        users: vec![
            User::new("testuser").password(Password::Plain("hunter2".into())),
            User::new("root").password(Password::Hashed("$6$x".into())),
            User::new("bot").password(Password::Prompt),
        ],
        ..SystemManifest::default()
    };
    let warnings = manifest.password_warnings();
    assert_eq!(warnings.len(), 1);
    assert!(warnings[0].contains("testuser"));
    assert!(warnings[0].contains("Password::Plain"));
}

#[test]
fn prompt_password_serialises_without_a_secret() {
    let user = User::new("testuser").password(Password::Prompt);
    let toml = toml::to_string(&user).expect("serialise user");
    assert!(toml.contains("password = \"prompt\""), "got: {toml}");
    assert!(!toml.contains("hunter"));
}

#[test]
fn prompt_username_round_trips_through_toml() {
    let user = User::prompt().password(Password::Prompt);
    let toml = toml::to_string(&user).expect("serialise user");
    assert!(toml.contains("name = \"prompt\""), "got: {toml}");

    let parsed: User = toml::from_str(&toml).expect("deserialise user");
    assert_eq!(parsed.name, Username::Prompt);
}

#[test]
fn literal_username_round_trips_through_toml() {
    let user = User::new("testuser");
    let toml = toml::to_string(&user).expect("serialise user");

    let parsed: User = toml::from_str(&toml).expect("deserialise user");
    assert_eq!(parsed.name, Username::Literal("testuser".into()));
}

#[test]
fn shell_paths_are_absolute() {
    assert_eq!(Shell::Bash.path(), "/bin/bash");
    assert_eq!(Shell::Zsh.path(), "/bin/zsh");
    assert_eq!(Shell::Fish.path(), "/usr/bin/fish");
}

#[test]
fn package_builder_populates_expected_fields() {
    let package = Package::new("gui-wm/niri")
        .binary(true)
        .version("25.11-r1")
        .use_flags(["screencast", "-debug"]);

    assert_eq!(package.package, "gui-wm/niri");
    assert!(package.binary);
    assert_eq!(package.version.as_deref(), Some("25.11-r1"));
    assert_eq!(package.use_flags, vec!["screencast", "-debug"]);
}

#[test]
fn converts_system_manifest_to_internal_manifest() {
    let manifest = PlannerManifest::from(SystemManifest {
        os: Os {
            libc: Libc::Glibc,
            ..Os::default()
        },
        packages: vec![Package::new("gui-wm/niri").version("25.11-r1")],
        ..SystemManifest::default()
    });

    assert_eq!(manifest.libc, Some(Libc::Glibc));
    assert_eq!(manifest.packages.len(), 1);
    assert_eq!(manifest.packages[0].version.as_deref(), Some("25.11-r1"));
}

#[test]
fn disk_default_uses_ext4_whole_disk() {
    let disk = Disk::default();

    assert_eq!(disk.layout, DiskLayout::Ext4);
    assert_eq!(disk.encryption, Encryption::None);
    // ext4 whole-disk: single root partition, no separate /home.
    assert!(!disk.ext4.separate_home);
}

#[test]
fn disk_encryption_deserializes_password_mode() {
    let manifest = toml::from_str::<SystemManifest>(
        r#"
                [disk]
                device = "/dev/nvme0n1"
                layout = "ext4"
                encryption = "password"
            "#,
    )
    .expect("manifest should parse");

    assert_eq!(manifest.disk.encryption, Encryption::Password);
}

#[test]
fn efi_partition_default_uses_512mb() {
    assert_eq!(EfiPartition::default().size, 512 * MB);
}

#[test]
fn graphics_round_trips_through_toml() {
    let manifest = SystemManifest {
        hardware: Hardware {
            graphics: Graphics {
                mesa: MesaGraphics {
                    video_cards: VideoCards::Explicit(vec![VideoCard::Amdgpu, VideoCard::Radeonsi]),
                    ..MesaGraphics::default()
                },
                vm_support: VmGraphics::Virgl,
                ..Graphics::default()
            },
            ..Hardware::default()
        },
        ..SystemManifest::default()
    };

    let toml = toml::to_string(&manifest).expect("manifest should serialize");
    assert!(toml.contains("[hardware.graphics.mesa.video_cards]"));
    assert!(!toml.contains("hardware.gpu"));
    let parsed: SystemManifest = toml::from_str(&toml).expect("manifest should deserialize");
    assert_eq!(parsed.hardware.graphics, manifest.hardware.graphics);
}

#[test]
fn gpu_deserializes_legacy_single_vendor_string() {
    let manifest = toml::from_str::<SystemManifest>(
        r#"
                [hardware]
                gpu = "nvidia"
            "#,
    )
    .expect("manifest should parse");

    assert_eq!(
        manifest.hardware.graphics.nvidia.unwrap().prime,
        PrimeMode::Primary
    );
}

#[test]
fn gpu_deserializes_hybrid_gpu_table() {
    let manifest = toml::from_str::<SystemManifest>(
        r#"
                [hardware.gpu]
                igpu = "amd"
                dgpu = "nvidia"
            "#,
    )
    .expect("manifest should parse");

    let graphics = manifest.hardware.graphics;
    assert_eq!(graphics.nvidia.unwrap().prime, PrimeMode::Offload);
    assert!(
        matches!(graphics.mesa.video_cards, VideoCards::Explicit(ref cards) if cards.contains(&VideoCard::Amdgpu))
    );
}

#[test]
fn missing_firewall_field_defaults_to_disabled() {
    // Manifests serialized before the firewall field existed must keep their
    // behaviour: no firewall.
    let manifest: SystemManifest = toml::from_str("").expect("default manifest");
    assert_eq!(manifest.firewall, Firewall::Disabled);
    assert!(!manifest.firewall.enabled());

    let toml = toml::to_string(&manifest).expect("serialize manifest");
    assert!(toml.contains("firewall = \"disabled\""), "got: {toml}");
}

#[test]
fn firewall_nftables_round_trips_through_toml() {
    let manifest = SystemManifest {
        firewall: Firewall::Nftables {
            incoming: FirewallPolicy::Drop,
            forwarding: FirewallPolicy::Drop,
            outgoing: FirewallPolicy::Accept,
            allow_icmp: true,
            tcp_ports: vec![22],
            udp_ports: vec![],
        },
        ..SystemManifest::default()
    };

    let toml = toml::to_string(&manifest).expect("serialize manifest");
    assert!(toml.contains("[firewall.nftables]"), "got: {toml}");
    assert!(toml.contains("incoming = \"drop\""), "got: {toml}");
    assert!(toml.contains("outgoing = \"accept\""), "got: {toml}");
    assert!(toml.contains("tcp_ports = [22]"), "got: {toml}");
    let parsed: SystemManifest = toml::from_str(&toml).expect("deserialize manifest");
    assert_eq!(parsed.firewall, manifest.firewall);
}

#[test]
fn firewall_validation_covers_ports_package_service_and_init() {
    let firewall = Firewall::Nftables {
        incoming: FirewallPolicy::Drop,
        forwarding: FirewallPolicy::Drop,
        outgoing: FirewallPolicy::Accept,
        allow_icmp: true,
        tcp_ports: vec![22],
        udp_ports: vec![0],
    };
    assert_eq!(
        firewall.validate(),
        Err(FirewallValidationError::ZeroPort { protocol: "udp" })
    );

    let mut manifest = SystemManifest {
        firewall: Firewall::Nftables {
            incoming: FirewallPolicy::Drop,
            forwarding: FirewallPolicy::Drop,
            outgoing: FirewallPolicy::Accept,
            allow_icmp: true,
            tcp_ports: vec![],
            udp_ports: vec![],
        },
        ..SystemManifest::default()
    };
    assert_eq!(
        manifest.validate_firewall(),
        Err(FirewallValidationError::MissingPackage)
    );
    manifest.packages.push(Package::new(NFTABLES_PACKAGE));
    assert_eq!(
        manifest.validate_firewall(),
        Err(FirewallValidationError::MissingService)
    );
    manifest
        .services
        .openrc
        .default
        .push(NFTABLES_SERVICE.to_owned());
    assert_eq!(manifest.validate_firewall(), Ok(()));

    manifest.init_system = InitSystem::Systemd;
    assert_eq!(
        manifest.validate_firewall(),
        Err(FirewallValidationError::UnsupportedInit(
            InitSystem::Systemd
        ))
    );

    // A disabled firewall never demands the package or service.
    assert_eq!(SystemManifest::default().validate_firewall(), Ok(()));
}

#[test]
fn session_rust_dsl_round_trips_through_toml() {
    let manifest = SystemManifest {
        session: Session {
            mode: SessionMode::Graphical,
            user: SessionUser::Named("alex".into()),
            desktop_shell: Some(DesktopShell::Noctalia),
            seat: SeatBackend::Seatd,
            session_tracker: SessionTracker::Elogind,
            ..Session::default()
        },
        ..SystemManifest::default()
    };
    let toml = toml::to_string(&manifest).expect("serialize session");
    let parsed: SystemManifest = toml::from_str(&toml).expect("deserialize session");
    assert_eq!(parsed.session, manifest.session);
}

#[test]
fn missing_session_defaults_to_text_while_explicit_auto_is_accepted() {
    let defaulted: SystemManifest = toml::from_str("").expect("default manifest");
    assert_eq!(defaulted.session.mode, SessionMode::Text);

    let legacy: SystemManifest =
        toml::from_str("[session]\nmode = \"auto\"\n").expect("explicit legacy auto session");
    assert_eq!(legacy.session.mode, SessionMode::Auto);
}
