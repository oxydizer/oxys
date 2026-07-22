use std::{fs, os::unix::fs::PermissionsExt, path::Path, sync::mpsc::Sender};

use crate::{
    graphics::ResolvedGraphics,
    manifest::{DesktopShell, SystemManifest, Terminal, User},
    session::{ResolvedSession, ResolvedSessionMode},
};

use super::{SystemInstallError, SystemInstallEvent, run_chroot, write_file};

/// Configure how the installed system logs in on tty1, and undo the live ISO's
/// self-launch so the target never re-runs the installer.
///
/// The live medium wires tty1 to `agetty --autologin root` plus a
/// `/root/.bash_profile` that execs `oxys-installer` (see the ISO fsscript).
/// Both files are copied into the target by the rsync, so without this step a
/// freshly installed system would boot, auto-log-in as root, and start the
/// installer all over again. We rewrite the tty1 inittab entry to a normal
/// login: Remi-style user autologin into a shell that starts Niri for a
/// graphical config, or a plain `agetty` prompt otherwise. We also strip the
/// installer launch from root's profile.
pub(super) fn setup_login(
    manifest: &SystemManifest,
    resolved: &ResolvedSession,
    resolved_graphics: &ResolvedGraphics,
    target_mount: &Path,
    sender: &Sender<SystemInstallEvent>,
) -> Result<(), SystemInstallError> {
    // Stop the installed system from re-launching the installer on tty1.
    let bash_profile = target_mount.join("root/.bash_profile");
    if let Ok(contents) = fs::read_to_string(&bash_profile)
        && contents.contains("oxys-installer")
    {
        fs::remove_file(&bash_profile)?;
        let _ = sender.send(SystemInstallEvent::StepOutput {
            line: "removed live-medium installer autostart from /root/.bash_profile".to_owned(),
        });
    }

    let login_user = resolved
        .policy
        .user_index
        .and_then(|index| manifest.users.get(index));
    if let Some(user) = login_user {
        write_welcome_launcher(resolved.policy.terminal, target_mount)?;
        write_graphical_session_files(user, resolved, resolved_graphics, target_mount, sender)?;
    }
    write_file(
        &target_mount.join("etc/oxys/session.env"),
        &system_session_config(resolved),
    )?;

    let tty1 = if resolved.policy.mode == ResolvedSessionMode::Graphical {
        // Hand the raw console to oxys-login (running as root under agetty).
        // `--skip-login` suppresses agetty's own "login:" prompt because
        // oxys-login draws its own PAM login TUI; on success it setuids to the
        // chosen user and execs `dbus-run-session -- niri --session`. Ctrl+Q
        // execs the standard /bin/login for a diagnostic TTY instead. agetty
        // respawns oxys-login when either session exits.
        "c1:12345:respawn:/sbin/agetty --noclear --skip-login \
--login-program /usr/local/bin/oxys-login 38400 tty1 linux"
            .to_owned()
    } else {
        "c1:12345:respawn:/sbin/agetty --noclear 38400 tty1 linux".to_owned()
    };

    // Replace the existing tty1 (`c1:`) entry in place, or append one if none.
    let inittab = target_mount.join("etc/inittab");
    let existing = fs::read_to_string(&inittab).unwrap_or_default();
    let mut lines: Vec<String> = Vec::new();
    let mut replaced = false;
    for line in existing.lines() {
        if line.trim_start().starts_with("c1:") {
            lines.push(tty1.clone());
            replaced = true;
        } else {
            lines.push(line.to_owned());
        }
    }
    if !replaced {
        lines.push(tty1.clone());
    }
    let mut body = lines.join("\n");
    body.push('\n');
    write_file(&inittab, &body)?;

    let _ = sender.send(SystemInstallEvent::StepOutput {
        line: if let Some(user) = login_user {
            format!(
                "tty1 login: oxys-login (PAM) -> niri --session, seeded for {}",
                user.name.as_str()
            )
        } else {
            "tty1 login: agetty text login".to_owned()
        },
    });

    Ok(())
}

/// Install the Niri first-login launcher globally. Its marker is deliberately
/// per-user, so every account sees the welcome once without requiring write
/// access to shared state under /var/lib.
fn write_welcome_launcher(
    terminal: Terminal,
    target_mount: &Path,
) -> Result<(), SystemInstallError> {
    let path = target_mount.join("usr/local/bin/oxys-welcome-once");
    write_file(&path, &welcome_launcher_contents(terminal))?;
    fs::set_permissions(path, fs::Permissions::from_mode(0o755))?;
    Ok(())
}

fn system_session_config(resolved: &ResolvedSession) -> String {
    let fallback = match resolved.policy.login {
        crate::manifest::LoginFrontend::OxysLogin {
            fallback_tty_login, ..
        } => fallback_tty_login,
        crate::manifest::LoginFrontend::Tty { .. } => true,
    };
    let mut body = session_environment_contents(resolved);
    body.push_str(&format!("OXYS_FALLBACK_TTY_LOGIN={fallback}\n"));
    body
}

fn write_graphical_session_files(
    user: &User,
    resolved: &ResolvedSession,
    resolved_graphics: &ResolvedGraphics,
    target_mount: &Path,
    sender: &Sender<SystemInstallEvent>,
) -> Result<(), SystemInstallError> {
    let name = user.name.as_str();
    let home = target_mount.join("home").join(name);
    write_file(&home.join(".bash_profile"), &bash_profile_contents())?;
    write_file(&home.join(".bashrc"), &bashrc_contents())?;
    write_file(
        &home.join(".config/environment.d/90-oxys-session.conf"),
        &session_environment_contents(resolved),
    )?;
    write_file(
        &home.join(".config/niri/config.kdl"),
        &niri_config_contents(resolved, resolved_graphics),
    )?;
    if resolved.policy.desktop_shell == Some(DesktopShell::Noctalia) {
        write_file(&home.join(".config/noctalia/config.toml"), NOCTALIA_CONFIG)?;
        write_file(
            &home.join(".config/noctalia/palettes/OxysOS.json"),
            NOCTALIA_OXYSOS_PALETTE,
        )?;
    }
    if resolved.policy.terminal == Terminal::Foot {
        write_file(&home.join(".config/foot/foot.ini"), FOOT_CONFIG)?;
    }
    fs::create_dir_all(home.join("Pictures/Screenshots"))?;

    if target_passwd_has_user(target_mount, name) {
        let target = target_mount.display().to_string();
        run_chroot(
            &target,
            &[
                "chown".to_owned(),
                "-R".to_owned(),
                format!("{name}:{name}"),
                format!("/home/{name}/.bash_profile"),
                format!("/home/{name}/.bashrc"),
                format!("/home/{name}/.config"),
                format!("/home/{name}/Pictures"),
            ],
            sender,
        )?;
    }

    let _ = sender.send(SystemInstallEvent::StepOutput {
        line: format!("seeded Niri/Noctalia session files for {name}"),
    });
    Ok(())
}

fn session_environment_contents(resolved: &ResolvedSession) -> String {
    let mut lines = vec!["# generated by oxys".to_owned()];
    lines.extend(
        resolved
            .requirements
            .environment
            .iter()
            .map(|(name, value)| format!("{name}={value}")),
    );
    lines.push(String::new());
    lines.join("\n")
}

fn niri_config_contents(
    resolved: &ResolvedSession,
    resolved_graphics: &ResolvedGraphics,
) -> String {
    let mut config = NIRI_CONFIG
        .lines()
        .filter(|line| {
            (resolved.policy.audio_stack == Some(crate::manifest::AudioStack::Pipewire)
                || !line.contains("gentoo-pipewire-launcher"))
                && (resolved.policy.desktop_shell == Some(DesktopShell::Noctalia)
                    || !line.contains("until noctalia"))
        })
        .collect::<Vec<_>>()
        .join("\n");
    config = config.replace("__OXYS_TERMINAL__", resolved.policy.terminal.executable());
    if let Some(prime) = &resolved_graphics.requirements.prime {
        config.push_str(&format!(
            "\n\ndebug {{\n    render-drm-device \"{}\"\n}}",
            prime.compositor_gpu
        ));
    }
    config + "\n"
}

fn target_passwd_has_user(target_mount: &Path, username: &str) -> bool {
    fs::read_to_string(target_mount.join("etc/passwd"))
        .map(|passwd| {
            passwd
                .lines()
                .any(|line| line.split(':').next() == Some(username))
        })
        .unwrap_or(false)
}

fn bash_profile_contents() -> String {
    // The desktop session is launched by oxys-login on tty1 (see setup_login),
    // not from the shell, so this profile only sources .bashrc. `startniri`
    // lives in .bashrc as a manual restart helper.
    r#"# ~/.bash_profile generated by oxys.
[[ -f ~/.bashrc ]] && . ~/.bashrc
"#
    .to_owned()
}

fn bashrc_contents() -> String {
    r#"# ~/.bashrc generated by oxys.
[[ $- != *i* ]] && return

export EDITOR="${EDITOR:-nano}"
export PATH="$HOME/.cargo/bin:$HOME/go/bin:$HOME/.local/bin:$PATH"
PS1='\[\e[1;36m\]oxys\[\e[0m\]:\[\e[1;34m\]\w\[\e[0m\]\$ '

startniri() {
    if [ -z "${XDG_RUNTIME_DIR}" ]; then
        uid="$(id -u)"
        if [ -d "/run/user/${uid}" ] && [ -w "/run/user/${uid}" ]; then
            export XDG_RUNTIME_DIR="/run/user/${uid}"
        else
            echo "Oxys session tracker did not provide /run/user/${uid}" >&2
            return 1
        fi
    fi
    export XDG_SESSION_TYPE=wayland
    export XDG_CURRENT_DESKTOP=niri
    export MOZ_ENABLE_WAYLAND=1
    export QT_QPA_PLATFORM="${QT_QPA_PLATFORM:-wayland;xcb}"
    dbus-run-session -- niri 2>&1 | tee "${HOME}/niri.log"
}

if [ -z "${WAYLAND_DISPLAY}" ] && [ "$(tty)" != "/dev/tty1" ] \
   && pgrep -x niri >/dev/null 2>&1; then
    export WAYLAND_DISPLAY=wayland-1
fi
"#
    .to_owned()
}

const NIRI_CONFIG: &str = r##"// OxysOS Niri configuration.

input {
    keyboard {
        xkb {
            layout "us"
        }
    }
    touchpad {
        tap
        natural-scroll
    }
    mouse {}
}

layout {
    gaps 12
    center-focused-column "never"
    preset-column-widths {
        proportion 0.33333
        proportion 0.5
        proportion 0.66667
    }
    default-column-width { proportion 0.5; }
    focus-ring {
        width 1
        active-color "#F55926"
        inactive-color "#505050"
    }
    border {
        off
    }
}

hotkey-overlay {
    skip-at-startup
}

spawn-at-startup "sh" "-c" "command -v xwayland-satellite >/dev/null 2>&1 && exec xwayland-satellite"
spawn-at-startup "sh" "-c" "command -v udiskie >/dev/null 2>&1 && exec udiskie --no-tray"
spawn-at-startup "/usr/local/bin/oxys-welcome-once"
// On OpenRC, PipeWire and WirePlumber are user-session processes rather than
// system services. Gentoo's launcher starts both inside Niri's D-Bus session;
// Noctalia retries below until PipeWire is ready because it requires a daemon.
spawn-at-startup "sh" "-c" "command -v gentoo-pipewire-launcher >/dev/null 2>&1 && exec gentoo-pipewire-launcher"
spawn-at-startup "sh" "-c" "until noctalia; do sleep 2; done"
spawn-at-startup "sh" "-c" "for agent in /usr/libexec/polkit-gnome-authentication-agent-1 /usr/lib/polkit-gnome/polkit-gnome-authentication-agent-1; do [ -x \"$agent\" ] && exec \"$agent\"; done"
spawn-at-startup "sh" "-c" "gsettings set org.gnome.desktop.interface color-scheme 'prefer-dark' 2>/dev/null || true; gsettings set org.gnome.desktop.interface icon-theme 'Adwaita' 2>/dev/null || true"

screenshot-path "~/Pictures/Screenshots/screenshot-%Y%m%d-%H%M%S.png"

environment {
    DISPLAY ":0"
    XDG_CURRENT_DESKTOP "niri"
    GTK_THEME "Adwaita:dark"
    QT_QPA_PLATFORMTHEME "gtk3"
}

cursor {
    xcursor-theme "default"
    xcursor-size 24
}

// Keep the first-login TUI distinct from ordinary terminal windows and present
// it as a floating window. The launcher assigns this app-id on every supported
// terminal.
window-rule {
    match app-id="^oxys-welcome$"
    open-floating true
}

window-rule {
    geometry-corner-radius 10
    clip-to-geometry true
    open-floating true
    shadow {
        on
        softness 30
        spread 5
        offset x=0 y=5
        color "#00000070"
    }
}

binds {
    Mod+Return       { spawn "__OXYS_TERMINAL__"; }
    Mod+T            { spawn "__OXYS_TERMINAL__"; }
    Mod+D            { spawn "noctalia" "msg" "panel-toggle" "launcher"; }
    Mod+C            { spawn "noctalia" "msg" "panel-toggle" "control-center"; }
    Mod+B            { spawn "firefox"; }
    Mod+Alt+L        { spawn "noctalia" "msg" "session" "lock"; }
    Mod+Escape       { spawn "noctalia" "msg" "panel-toggle" "session-panel"; }
    Mod+Q            { close-window; }

    Mod+Left         { focus-column-left; }
    Mod+Right        { focus-column-right; }
    Mod+Up           { focus-window-up; }
    Mod+Down         { focus-window-down; }
    Mod+H            { focus-column-left; }
    Mod+L            { focus-column-right; }
    Mod+K            { focus-window-up; }
    Mod+J            { focus-window-down; }

    Mod+Shift+Left   { move-column-left; }
    Mod+Shift+Right  { move-column-right; }
    Mod+Shift+Up     { move-window-up; }
    Mod+Shift+Down   { move-window-down; }

    Mod+1            { focus-workspace 1; }
    Mod+2            { focus-workspace 2; }
    Mod+3            { focus-workspace 3; }
    Mod+4            { focus-workspace 4; }
    Mod+5            { focus-workspace 5; }
    Mod+Shift+1      { move-column-to-workspace 1; }
    Mod+Shift+2      { move-column-to-workspace 2; }
    Mod+Shift+3      { move-column-to-workspace 3; }

    Mod+F            { maximize-column; }
    Mod+Shift+F      { fullscreen-window; }
    Mod+R            { switch-preset-column-width; }
    Mod+Comma        { consume-window-into-column; }
    Mod+Period       { expel-window-from-column; }

    Mod+Shift+Slash  { show-hotkey-overlay; }

    XF86AudioRaiseVolume  allow-when-locked=true { spawn "noctalia" "msg" "volume-up"; }
    XF86AudioLowerVolume  allow-when-locked=true { spawn "noctalia" "msg" "volume-down"; }
    XF86AudioMute         allow-when-locked=true { spawn "noctalia" "msg" "volume-mute"; }
    XF86AudioMicMute      allow-when-locked=true { spawn "noctalia" "msg" "mic-mute"; }
    XF86AudioPlay         allow-when-locked=true { spawn "noctalia" "msg" "media" "toggle"; }
    XF86AudioNext         allow-when-locked=true { spawn "noctalia" "msg" "media" "next"; }
    XF86AudioPrev         allow-when-locked=true { spawn "noctalia" "msg" "media" "previous"; }
    XF86MonBrightnessUp   allow-when-locked=true { spawn "noctalia" "msg" "brightness-up"; }
    XF86MonBrightnessDown allow-when-locked=true { spawn "noctalia" "msg" "brightness-down"; }

    Print            { spawn "sh" "-c" "grim -g \"$(slurp)\" - | wl-copy"; }

    Mod+Shift+E      { quit; }
    Mod+Shift+P      { power-off-monitors; }
}
"##;

fn welcome_launcher_contents(terminal: Terminal) -> String {
    r#"#!/bin/sh

STATE_HOME="${XDG_STATE_HOME:-${HOME}/.local/state}"
MARKER="${STATE_HOME}/oxys/welcome-v1"

[ -e "${MARKER}" ] && exit 0
command -v __OXYS_TERMINAL__ >/dev/null 2>&1 || exit 0
[ -x /usr/bin/oxys ] || exit 0

if __OXYS_WELCOME_COMMAND__; then
    mkdir -p "$(dirname "${MARKER}")" && : > "${MARKER}"
fi
"#
    .replace("__OXYS_TERMINAL__", terminal.executable())
    .replace("__OXYS_WELCOME_COMMAND__", terminal.welcome_command())
}

const NOCTALIA_CONFIG: &str = r#"# OxysOS Noctalia v5 config.

[shell]
setup_wizard_enabled = false

[bar.default]
position = "bottom"
background_opacity = 0.93
margin_edge = 4
start = [ "launcher", "active_window",  ]
center = [  ]
end = [ "workspaces", "tray", "notifications", "battery", "volume", "brightness", "control-center", "clock" ]
thickness = 46

[widget.launcher]
custom_image = "/usr/share/oxys/icons/launcher.png"
custom_image_colorize = false

[dock]
enabled = false
auto_hide = true
position = "bottom"

[wallpaper]
enabled = true
directory = "/usr/share/backgrounds"
fill_mode = "crop"

[wallpaper.default]
path = "/usr/share/backgrounds/default.png"

[theme]
source = "custom"
builtin = "Noctalia"
custom_palette = "OxysOS"
mode = "dark"
"#;

const NOCTALIA_OXYSOS_PALETTE: &str = include_str!("noctalia-palette.json");

const FOOT_CONFIG: &str = r#"font=monospace:size=11
dpi-aware=yes

[cursor]
style=beam
blink=yes

[colors]
alpha=0.95
background=111111
foreground=e8e8e8
regular0=111111
regular1=f66151
regular2=57e389
regular3=f8e45c
regular4=62a0ea
regular5=dc8add
regular6=5bc8af
regular7=e8e8e8

[scrollback]
lines=10000
"#;

#[cfg(test)]
mod tests {
    use std::{os::unix::fs::PermissionsExt, process::Command};

    use crate::{
        graphics::resolve_graphics,
        manifest::{
            Graphics, Hardware, MesaGraphics, Nvidia, PrimeMode, Session, SessionMode,
            SystemManifest, User, VideoCard, VideoCards,
        },
    };

    #[test]
    fn niri_config_pins_the_resolved_prime_compositor_render_node() {
        let manifest = SystemManifest {
            session: Session {
                mode: SessionMode::Graphical,
                ..Session::default()
            },
            users: vec![User::new("alex")],
            hardware: Hardware {
                graphics: Graphics {
                    mesa: MesaGraphics {
                        video_cards: VideoCards::Explicit(vec![VideoCard::Intel]),
                        ..MesaGraphics::default()
                    },
                    nvidia: Some(Nvidia {
                        prime: PrimeMode::Offload,
                        ..Nvidia::default()
                    }),
                    ..Graphics::default()
                },
                ..Hardware::default()
            },
            ..SystemManifest::default()
        };
        let session = manifest.resolved_session().unwrap();
        let mut graphics = resolve_graphics(&manifest).unwrap();
        graphics.requirements.prime.as_mut().unwrap().compositor_gpu =
            "/dev/dri/renderD128".to_owned();

        let config = super::niri_config_contents(&session, &graphics);
        assert!(config.contains("debug {"));
        assert!(config.contains("render-drm-device \"/dev/dri/renderD128\""));
    }

    #[test]
    fn selected_terminal_drives_niri_shortcuts_and_welcome_launcher() {
        let manifest = SystemManifest {
            session: Session {
                mode: SessionMode::Graphical,
                terminal: crate::manifest::Terminal::Kitty,
                ..Session::default()
            },
            users: vec![User::new("alex")],
            ..SystemManifest::default()
        };
        let session = manifest.resolved_session().unwrap();
        let graphics = resolve_graphics(&manifest).unwrap();

        let config = super::niri_config_contents(&session, &graphics);
        assert!(config.contains("Mod+Return       { spawn \"kitty\"; }"));
        assert!(config.contains("Mod+T            { spawn \"kitty\"; }"));
        assert!(!config.contains("__OXYS_TERMINAL__"));

        let launcher = super::welcome_launcher_contents(crate::manifest::Terminal::Kitty);
        assert!(launcher.contains("command -v kitty"));
        assert!(launcher.contains("kitty --class oxys-welcome /usr/bin/oxys welcome"));
        assert!(!launcher.contains("foot"));
    }

    #[test]
    fn welcome_launcher_runs_once_per_user() {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().join("home");
        let bin = temp.path().join("bin");
        std::fs::create_dir_all(&home).unwrap();
        std::fs::create_dir_all(&bin).unwrap();

        let foot = bin.join("foot");
        std::fs::write(&foot, "#!/bin/sh\nexit 1\n").unwrap();
        std::fs::set_permissions(&foot, std::fs::Permissions::from_mode(0o755)).unwrap();

        // The production launcher intentionally requires the installed Oxys
        // binary at /usr/bin/oxys. Substitute a ubiquitous executable here so
        // this behavior test remains hermetic.
        let launcher = super::welcome_launcher_contents(crate::manifest::Terminal::Foot)
            .replace("[ -x /usr/bin/oxys ]", "[ -x /bin/true ]");
        let run = || {
            Command::new("sh")
                .arg("-c")
                .arg(&launcher)
                .env("HOME", &home)
                .env_remove("XDG_STATE_HOME")
                .env("PATH", format!("{}:/usr/bin:/bin", bin.display()))
                .status()
                .unwrap()
        };

        assert!(run().success());
        assert!(
            !home.join(".local/state/oxys/welcome-v1").exists(),
            "a failed welcome must remain retryable"
        );

        std::fs::write(
            &foot,
            "#!/bin/sh\nprintf 'run\\n' >> \"${HOME}/foot-runs\"\n",
        )
        .unwrap();
        assert!(run().success());
        assert!(home.join(".local/state/oxys/welcome-v1").is_file());
        assert_eq!(
            std::fs::read_to_string(home.join("foot-runs")).unwrap(),
            "run\n"
        );

        assert!(run().success());
        assert_eq!(
            std::fs::read_to_string(home.join("foot-runs")).unwrap(),
            "run\n",
            "the marker must suppress subsequent launches"
        );
    }
}
