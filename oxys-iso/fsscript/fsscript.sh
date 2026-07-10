#!/bin/bash
# OxysOS fsscript — runs INSIDE the live chroot during livecd-stage2.
# ---------------------------------------------------------------------------
# Responsibilities:
#   1. Make the injected installer binary executable.
#   2. Ensure the ZFS kernel module is loaded BEFORE the installer runs.
#   3. Autologin root on tty1 and exec the installer there.
#
# catalyst copies this script into the chroot and executes it, so all paths
# below are relative to the live root filesystem.
# ---------------------------------------------------------------------------
set -euo pipefail

INSTALLER=/usr/local/bin/oxys-installer

# --- 1. installer binary (delivered via livecd/overlay) ---------------------
if [[ ! -x ${INSTALLER} ]]; then
	chmod 0755 "${INSTALLER}"
fi

# --- 2. ZFS module load ordering -------------------------------------------
# Two complementary mechanisms, so the module is guaranteed present before the
# installer touches any pool:
#
#   (a) OpenRC's `modules` service (runs in the *boot* runlevel, which always
#       completes before the *default* runlevel / the tty1 getty) loads it
#       early. This is the system-wide, declarative path.
#   (b) An explicit `modprobe zfs` immediately before `exec`-ing the installer
#       in /root/.bash_profile. This is synchronous and authoritative — even if
#       getty races the boot runlevel, the module is loaded right before use.
#
# (a): declare the module for /etc/conf.d/modules (consumed by OpenRC).
if ! grep -q '^modules=.*zfs' /etc/conf.d/modules 2>/dev/null; then
	printf '\n# OxysOS: load ZFS early (boot runlevel)\nmodules="${modules} zfs"\n' \
		>> /etc/conf.d/modules
fi

# --- 3. Autologin root on tty1 (Gentoo OpenRC uses sysvinit + /etc/inittab) -
# Rewrite the tty1 agetty line to auto-login root. Other ttys are untouched, so
# tty2-6 still give a normal login prompt (handy if the installer wedges).
if grep -qE '^c1:' /etc/inittab; then
	sed -i -E \
		's|^c1:.*tty1.*$|c1:12345:respawn:/sbin/agetty --autologin root --noclear 38400 tty1 linux|' \
		/etc/inittab
else
	printf 'c1:12345:respawn:/sbin/agetty --autologin root --noclear 38400 tty1 linux\n' \
		>> /etc/inittab
fi

# (3 cont.) On tty1, root's login shell launches the installer. The modprobe
# here is the authoritative ordering guarantee from (b) above. We deliberately
# do NOT `exec` it: when the installer quits — whether it finished or failed —
# the login shell falls through to an interactive prompt instead of agetty
# respawning the installer. That keeps the live system debuggable (read
# /var/log/oxys-install.log, re-run `oxys-installer`) rather than a locked kiosk
# loop you can't escape from tty1.
cat > /root/.bash_profile <<'PROFILE'
# OxysOS: launch the installer on the primary console only.
if [[ "$(tty)" == "/dev/tty1" ]]; then
	# Authoritative: ensure ZFS is ready before the installer runs.
	modprobe zfs 2>/dev/null || true
	# Run (not exec) so quitting the installer drops to this shell instead of
	# respawning it. `|| true` keeps the shell alive even on a non-zero exit.
	/usr/local/bin/oxys-installer || true
	echo
	echo "oxys-installer exited. Full install log: /var/log/oxys-install.log"
	echo "Re-run it any time with:  oxys-installer"
fi
PROFILE
chmod 0644 /root/.bash_profile

# --- 4. Pre-warm the config compile cache ----------------------------------
# The installer compiles the user's Rust config into a manifest ON first use
# (oxys::compile). Building the oxys dependency tree from cold would take
# minutes; do it once here — in the stage2 chroot, which has the native
# toolchain and the vendored deps shipped via the overlay — so the live compile
# only recompiles the user's single edited file. This mirrors oxys::compile's
# scaffold layout exactly ($HOME/.cache/oxys/build + crate "oxys-config-scaffold"),
# so cargo reuses this target cache verbatim at runtime. Non-fatal on failure.
if command -v cargo >/dev/null 2>&1 && [[ -f /usr/src/oxys/Cargo.toml ]]; then
	export HOME=/root
	WARM=/root/.cache/oxys/build
	mkdir -p "${WARM}/src"
	cp /root/configs/desktop.rs "${WARM}/src/main.rs"
	cat > "${WARM}/Cargo.toml" <<'SCAFFOLD'
[package]
name = "oxys-config-scaffold"
version = "0.0.0"
edition = "2021"

[dependencies]
oxys = { path = "/usr/src/oxys" }
SCAFFOLD
	if ( cd "${WARM}" && cargo build ); then
		echo "OxysOS fsscript: pre-warmed config compile cache."
	else
		echo "OxysOS fsscript: config compile pre-warm failed (non-fatal)." >&2
	fi
fi

# NOTE: the GURU + oxys overlays are shipped via the ISO root overlay (rsynced
# into /var/db/repos by catalyst), not cloned here -- see
# scripts/build-installer-overlay.sh (GURU checkout) and overlay/var/db/repos/.

echo "OxysOS fsscript: installer + autologin + zfs autoload wired up."
