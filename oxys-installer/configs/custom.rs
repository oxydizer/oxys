use oxys::prelude::*;

pub fn config() -> Oxys {
    Oxys {
        os: Os {
            hostname: "oxys".into(),
            timezone: "Europe/London".into(),
            locale: "en_US.UTF-8".into(),
            shell: Shell::Bash,
            libc: Libc::Glibc,
        },
        hardware: Hardware {
            gpu: detect_gpu(),
            power: match (is_laptop(), is_vendor("asus")) {
                (true, true) => Power::AsusCtl,
                (true, false) => Power::Tlp,
                (false, _) => Power::None,
            },
        },
        init_system: InitSystem::Openrc,
        prefer_binary: true,
        packages: vec![
            Package::new("gui-wm/niri").use_flags(vec!["screencast"]),
            Package::new("gui-shells/noctalia")
                .binary(true)
                .keywords(["**"]),
            Package::new("www-client/firefox-bin"),
            Package::new("media-video/pipewire"),
        ],
        services: oxys::services! {
            enabled: ["dbus", "seatd", "NetworkManager", "sshd"],
            disabled: ["lvm2-monitor", "multipathd"],
        },
        ..Default::default()
    }
}

oxys::main!(config);
