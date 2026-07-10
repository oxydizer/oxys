use std::{fs, path::Path, sync::mpsc::Sender};

use crate::manifest::{SystemManifest, User};

use super::{run_chroot, write_file, SystemInstallError, SystemInstallEvent, OXYS_SHELL_BOOT_FLAG};

pub(super) fn manifest_wants_graphical(manifest: &SystemManifest) -> bool {
    manifest
        .packages
        .iter()
        .any(|package| package.package.trim().starts_with("gui-wm/niri"))
}

/// Pick the account that owns the desktop session. This intentionally uses the
/// first configured user: the installer UI already presents the manifest order
/// to the operator, and the bundled desktop config has a single user.
pub(super) fn graphical_login_user(manifest: &SystemManifest) -> Option<&User> {
    manifest
        .users
        .iter()
        .find(|user| !user.name.as_str().trim().is_empty())
}

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
    target_mount: &Path,
    sender: &Sender<SystemInstallEvent>,
) -> Result<(), SystemInstallError> {
    // Stop the installed system from re-launching the installer on tty1.
    let bash_profile = target_mount.join("root/.bash_profile");
    if let Ok(contents) = fs::read_to_string(&bash_profile) {
        if contents.contains("oxys-installer") {
            fs::remove_file(&bash_profile)?;
            let _ = sender.send(SystemInstallEvent::StepOutput {
                line: "removed live-medium installer autostart from /root/.bash_profile".to_owned(),
            });
        }
    }

    let login_user = manifest_wants_graphical(manifest)
        .then(|| graphical_login_user(manifest))
        .flatten();
    if let Some(user) = login_user {
        write_graphical_session_files(user, target_mount, sender)?;
    }

    let tty1 = if let Some(user) = login_user {
        format!(
            "c1:12345:respawn:/sbin/agetty --autologin {} --noclear 38400 tty1 linux",
            user.name.as_str()
        )
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
            format!("tty1 login: autologin {} -> startniri", user.name.as_str())
        } else if manifest_wants_graphical(manifest) {
            "tty1 login: agetty text login (no graphical user configured)".to_owned()
        } else {
            "tty1 login: agetty text login".to_owned()
        },
    });

    Ok(())
}

fn write_graphical_session_files(
    user: &User,
    target_mount: &Path,
    sender: &Sender<SystemInstallEvent>,
) -> Result<(), SystemInstallError> {
    let name = user.name.as_str();
    let home = target_mount.join("home").join(name);
    write_file(&home.join(".bash_profile"), &bash_profile_contents())?;
    write_file(&home.join(".bashrc"), &bashrc_contents())?;
    write_file(&home.join(".config/niri/config.kdl"), NIRI_CONFIG)?;
    write_file(&home.join(".config/noctalia/config.toml"), NOCTALIA_CONFIG)?;
    write_file(&home.join(".config/foot/foot.ini"), FOOT_CONFIG)?;
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
    format!(
        r#"# ~/.bash_profile generated by oxys.
[[ -f ~/.bashrc ]] && . ~/.bashrc

if ! pgrep -x niri >/dev/null 2>&1 && [ "$(tty)" = "/dev/tty1" ] \
   && ! grep -qw {OXYS_SHELL_BOOT_FLAG} /proc/cmdline; then
    startniri
    echo
    echo "Niri exited. You are on a shell on tty1."
    echo "  - restart it:        startniri"
    echo "  - inspect the log:   less ~/niri.log"
elif grep -qw {OXYS_SHELL_BOOT_FLAG} /proc/cmdline 2>/dev/null; then
    echo ":: {OXYS_SHELL_BOOT_FLAG} set - desktop autostart disabled. Run 'startniri' to launch Niri."
fi
"#
    )
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
            export XDG_RUNTIME_DIR="/tmp/xdg-runtime-${uid}"
            mkdir -p "${XDG_RUNTIME_DIR}"
            chmod 700 "${XDG_RUNTIME_DIR}"
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
        width 3
        active-color "#66d9ef"
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
spawn-at-startup "sh" "-c" "until noctalia; do sleep 2; done"
spawn-at-startup "sh" "-c" "for agent in /usr/libexec/polkit-gnome-authentication-agent-1 /usr/lib/polkit-gnome/polkit-gnome-authentication-agent-1; do [ -x \"$agent\" ] && exec \"$agent\"; done"
spawn-at-startup "sh" "-c" "gsettings set org.gnome.desktop.interface color-scheme 'prefer-dark' 2>/dev/null || true; gsettings set org.gnome.desktop.interface icon-theme 'Papirus-Dark' 2>/dev/null || true"

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
    Mod+Return       { spawn "foot"; }
    Mod+T            { spawn "foot"; }
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

const NOCTALIA_CONFIG: &str = r#"# OxysOS Noctalia v5 config.

[shell]
setup_wizard_enabled = false

[bar.default]
position = "bottom"
background_opacity = 0.93
margin_edge = 4
start = [ "launcher", "clock", "cpu", "active_window", "media" ]
center = [ "workspaces" ]
end = [ "tray", "notifications", "battery", "volume", "brightness", "control-center" ]

[dock]
enabled = false
auto_hide = true
position = "bottom"

[wallpaper]
enabled = true
directory = "/usr/share/backgrounds"

[theme]
builtin = "Noctalia"
mode = "dark"
"#;

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
