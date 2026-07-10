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
        disk: Disk {
            // Whole-disk ext4: EFI system partition + a single ext4 root
            // filling the drive. The installer supplies `device`.
            layout: DiskLayout::Ext4,
            ext4: Ext4Options {
                separate_home: false,
                ..Default::default()
            },
            ..Default::default()
        },
        hardware: Hardware {
            gpu: detect_gpu(),
            power: match (is_laptop(), is_vendor("asus")) {
                (true, true) => Power::AsusCtl,
                (true, false) => Power::Tlp,
                (false, _) => Power::None,
            },
        },
        compiler: Compiler {
            march: March::X86_64V3,
            ..Default::default()
        },
        init_system: InitSystem::Openrc,
        // Try binary first for everything. Packages with custom use_flags that
        // conflict with a binary install (e.g. niri's screencast flag below)
        // automatically fall back to building from source and just warn --
        // see the "falling back to source" note in `oxys check` output.
        prefer_binary: true,
        packages: vec![
            // Base CLI tooling — present on the ISO and rsync'd to the target,
            // listed here so they're tracked in @world and kept across updates.
            Package::new("net-misc/curl"),
            Package::new("dev-vcs/git"),
            // --- Wayland desktop, baked into the ISO (see installcd-stage1.spec)
            // and rsync'd to the target. Compositor + shell:
            Package::new("gui-wm/niri").use_flags(vec!["screencast"]).keywords(["**"]),
            Package::new("gui-shells/noctalia").keywords(["**"]),
            // audio/video + session manager:
            Package::new("media-video/pipewire"),
            Package::new("media-video/wireplumber"),
            // session plumbing:
            Package::new("sys-auth/seatd"),
            Package::new("sys-auth/polkit"),
            Package::new("app-crypt/p11-kit"),
            Package::new("sys-apps/xdg-desktop-portal"),
            Package::new("sys-apps/xdg-desktop-portal-gtk"),
            Package::new("x11-misc/xdg-user-dirs"),
            Package::new("sys-fs/udisks"),
            Package::new("gnome-base/gvfs"),
            // terminal + Wayland tools:
            Package::new("gui-apps/foot"),
            Package::new("gui-apps/wl-clipboard"),
            Package::new("gui-apps/xwayland-satellite").keywords(["**"]),
            Package::new("gui-apps/wlsunset").keywords(["**"]),
            Package::new("x11-base/xwayland"),
            // power / hardware:
            Package::new("sys-power/power-profiles-daemon"),
            Package::new("app-misc/ddcutil"),
            // fonts + icon theme:
            Package::new("media-fonts/noto"),
            Package::new("media-fonts/noto-emoji"),
            Package::new("x11-themes/papirus-icon-theme"),
            // browser:
            Package::new("www-client/firefox-bin"),
        ],
        services: oxys::services! {
            enabled: ["dbus", "seatd", "NetworkManager", "sshd"],
            disabled: ["lvm2-monitor", "multipathd"],
        },
        users: vec![User::prompt()
            .wheel()
            .shell(Shell::Bash)
            .password(Password::Prompt)],
        ..Default::default()
    }
}

oxys::main!(config);
