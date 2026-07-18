# OxysOS 0.1 Release Readiness Review

Reviewed: 2026-07-17  
Snapshot: `main` at `dee2895` (8 commits ahead of `origin/main`; source tree clean; a few untracked planning docs).

Compared against the prior review in this file (2026-07-13 / `cf57906`).

## Verdict

A lot really did get fixed. The high-risk install path is in much better shape:
root is locked on the target, the installer refuses to re-enter wipe/rsync, disk
preflight is real, system planning runs before wipe, hostname/timezone land, and
the target is seeded with a managed Oxys state plus a Portage-owned `oxys` CLI.

**Still not ready for a public `0.1.0` “supported OS” tag.** It is closer to a
credible **developer preview** if the support contract is narrowed and the
remaining safety/truthfulness gaps are finished—especially package failure
semantics, credential redaction in persisted state, remote config trust, typed
destructive confirmation, and one real ISO install→reboot→login matrix.

## Recommended 0.1 support contract

| Area | Supported for 0.1 | Explicitly out of scope |
| --- | --- | --- |
| CPU | x86-64-v3 capable AMD64 | Older AMD64 baselines, ARM |
| Firmware | UEFI | Installed-system legacy BIOS boot |
| Init | OpenRC | Systemd installs from the OpenRC live image |
| Bootloader | GRUB EFI | systemd-boot until separately tested |
| Storage | One dedicated, otherwise-unused disk | LVM, LUKS, MD RAID, multipath, in-use ZFS devices |
| Filesystem | Whole-disk, unencrypted ext4 | Btrfs, LUKS, ZFS-root, separate-home variants until tested |
| Profile | Desktop; base only after it creates an admin account | Arbitrary profiles that require unverified packages |
| Graphics | Intel and AMD paths that are actually boot-tested | NVIDIA until the post-install driver policy is implemented |
| Network | Default profile should install offline; custom packages require network | Claiming offline custom-package convergence |

If a modeled option is outside this table, validation should reject it before
the destructive confirmation screen. A field existing in `SystemManifest` is
not by itself a support promise.

## Progress since last review (fixed or largely fixed)

| Prior P0 | Status now | Evidence |
| --- | --- | --- |
| **1. Live root password on target** | **Fixed** | Install runs `passwd -l root`; profiles create a prompted wheel user with sudo |
| **2. Installer worker re-entry** | **Mostly fixed** | `install_in_progress()` blocks Esc/quit/restart while the channel is live; `start_install` refuses re-entry |
| **3. Disk preflight** | **Mostly fixed** | Holders (LVM/RAID/dm), swap, min 12 GiB, re-check immediately before wipe; system plan before wipe |
| **7. Managed system / CLI** | **Mostly fixed** | Seeds `/etc/oxys/current-manifest.toml` (+ optional `config.fe2o3`); Portage-owned `/usr/bin/oxys`; fsscript asserts VDB ownership |
| **Hostname / timezone** | **Fixed** | Written during install; OpenRC hostname conf included |
| **OpenRC service path escape** | **Fixed** | Rejects empty / `.` / `..` / `/` in service names; requires init scripts |
| **Installer tests** | **Improved** | Installer now has 7 unit tests (was 0) |

Other solid hardening since the July 13 pass:

- Pre-wipe **system plan validation** (session/graphics/source image) so an unsupported machine does not get wiped first
- Target layout verification after rsync (incomplete copy fails loud)
- resolv.conf repair in chroot for package emerge
- fsscript hard-fails on seatd server, zram-init, oxys CLI ownership, virgl-related expectations, etc.
- Workspace version already `0.1.0`

## Verification snapshot (2026-07-17)

| Check | Result |
| --- | --- |
| `cargo test --workspace --all-targets` | **Pass** (~310 tests; was ~179) |
| `bash -n` on key ISO scripts | **Pass** |
| `git diff --check` | **Pass** (clean worktree) |
| `cargo fmt --all -- --check` | **Fail** (~19 files of formatting drift) |
| `cargo clippy … -D warnings` | **Fail** (~50 diagnostics on `oxys` alone) |
| `cargo audit` | **Not installed** |
| Clean catalyst ISO + install/reboot | **Not demonstrated** (`.build` has stage specs/overlays; no ISO artifact found) |
| `oxys-login` tests | **0** |

Toolchain note: workspace uses `edition = "2024"` / version `0.1.0`.

## Remaining P0 / release blockers

### 1. Package install can still “succeed” while packages were skipped

`oxys/src/install/portage.rs` is still deliberately best-effort: missing Portage
tree, plan conflicts, config write errors, no network, emerge start failure, and
emerge failure all log a **Warning** and return **`Ok(())`**.

That means the TUI can still reach **Done / Installation complete** without the
selected package set. This was P0.6 before and remains the biggest correctness
hole for a public install story.

### 2. Portage fragment layout only half-fixed

`write_portage_plan_config` correctly writes `package.use/oxys` (directory form),
but still writes:

- `package.accept_keywords` as a **file**
- `package.license` as a **file**

The live ISO uses **directory-backed** `package.accept_keywords/` and
`package.license/`. On a real Gentoo layout that write path can fail (and then
gets swallowed as a warning by #1).

### 3. Stale `manifest.toml` reuse is still possible

`compile_config_file_in` still:

1. runs cargo successfully
2. accepts any existing `out_dir/manifest.toml`

It does **not** remove the old file first, does not require a freshly written
file, and has no regression test for “successful no-output config + pre-existing
valid manifest → fail.”

Installer compiles into the live cwd (`/root`), so profile switching / partial
runs can still silently reuse the previous profile’s manifest.

### 4. Custom remote configs remain an unsigned code execution path

Installer still accepts **`http://` and `https://`**, downloads into
`configs/custom.fe2o3`, then `cargo run`s it as root. No trust confirmation, no
pin/hash, no sandbox, no HTTP rejection.

For 0.1 this should either be disabled or require an explicit trust step +
HTTPS-only + size bound.

### 5. Destructive confirmation is still a single Enter

Preflight is much better, but Confirm is still one keypress—not typed device
path / `ERASE` as recommended. Easy mis-click remains a real footgun.

### 6. Install terminal state is still string-inferred

Worker re-entry is fixed, but success is still “channel closed **and** no log
line starts with `[error]`.” That is better than the old unconditional Done, but
still not a typed `Succeeded | Failed | Panicked` result. A panic/drop without
an `[error]` line can still look green.

### 7. Prompted passwords can land as plaintext in `current-manifest.toml`

Installer resolves `Password::Prompt` to `Password::Plain` in memory, then
`seed_oxys_config` serializes the full manifest. There is **no redaction step**
before persist. Plan render avoids logging secrets; on-disk applied state may
still store them.

### 8. `oxys-login` target/PAM story is still unfinished

Still true:

- Built against **host** glibc/PAM with an explicit ABI caveat in
  `build-installer-overlay.sh`
- Depends on `pam = "0.8"` (and `uzers`; prior RustSec issues were on the old
  `users` path—audit not re-run because `cargo audit` is missing)
- Password check via `pam` crate **and** a second raw `PamSession` open
  (double auth / policy fallback to `system-auth`)
- Successful `exec` means `Drop` / `pam_close_session` never run
- **Zero tests**

### 9. No completed RC install matrix

No ISO artifact, and the prior “unverified catalyst injection / no E2E” concern
is still open. Without:

boot ISO → desktop install → reboot disk-only → oxys-login → Niri → root locked /
sudo works / `oxys --version`

you cannot honestly call 0.1 shippable as an OS.

### 10. Support boundary is still soft

Much validation improved (OpenRC services, timezone existence, session/graphics
resolve before wipe), but the public installer still models more than the 0.1
contract should promise (ZFS planning paths, systemd service activation path,
custom profiles, sshd enabled by default with no project `sshd_config` hardening
/ root password auth disable). Locale is still parsed, not applied.

## What looks release-shaped already

- **Profiles**: desktop + base both prompt for a wheel admin user
- **Live credential isolation intent**: live `root:oxys` for recovery; target
  root locked after user setup
- **Disk safety baseline**: busy holders/swap/size + second preflight
- **Managed target**: `/etc/oxys/*` + Portage-owned CLI + fsscript ownership checks
- **Core library quality**: large, green unit/integration suite around resolver,
  packages, update, session, install planning
- **ISO gatekeeping**: fsscript refuses to ship a desktop image missing
  seatd/zram-init/oxys/etc.

## Recommended positioning

| Mode | Ready? |
| --- | --- |
| Public `0.1.0` “install OxysOS on real hardware” | **No** |
| Tagged **0.1.0-dev / developer preview** with explicit narrow contract | **Almost** — after E2E once and the safety items below |
| Internal dogfood ISO for QEMU/desktop iteration | **Yes, with eyes open** |

Match the support table above and put it in the README before any public claim.

## Minimum remaining work for a public 0.1 tag

Ordered by risk:

1. **Package failures must block Done** (or a real Partial screen that cannot say “complete”).
2. **Write only owned Portage fragments** (`package.accept_keywords/oxys`, `package.license/oxys`) and never clobber directory layouts.
3. **Delete/require fresh `manifest.toml`** on every compile; add the stale-reuse regression test.
4. **Redact credentials** before writing `current-manifest.toml` (store `None`/`Prompt`/hash markers, never Plain).
5. **Typed destructive confirmation** in the TUI.
6. **Disable or hard-gate remote custom configs** for 0.1.
7. **Typed install result** from the worker (not `[error]` string sniffing).
8. **One full QEMU RC**: ISO build → install → reboot → login → Niri → security checks → offline desktop profile.
9. **oxys-login**: target-matched build, single PAM service, supervised session (parent waits so PAM close runs).
10. **fmt + clippy green**, pin/record release inputs, checksum ISO, update README/support contract.

Nice-to-have for the same tag: disable `sshd` by default or ship an owned
hardened config; reject unsupported layouts in the public installer; CI gates;
`SECURITY.md` / changelog.

## Release checklist

### Scope and safety

- [ ] The 0.1 support contract is written in README and enforced in validation.
- [x] Installed root is locked (`passwd -l root`) after user setup.
- [x] Shipped desktop/base profiles create a usable wheel admin account.
- [ ] TUI uses typed destructive confirmation.
- [x] Disk graph (holders), swap, size floor, and last-moment preflight exist.
- [x] Destructive background work cannot be re-entered via Esc/quit while live.
- [ ] Custom executable configs have an explicit trust policy.
- [ ] Stale manifest regression test passes.

### Install correctness

- [ ] Portage fragment handling works with normal Gentoo directory layouts
      (`package.accept_keywords/oxys`, `package.license/oxys`).
- [ ] Requested package failures cannot produce a success screen.
- [ ] Default desktop install succeeds offline from baked contents.
- [x] Effective manifest is persisted under `/etc/oxys/`.
- [ ] Credential-redacted desired-state manifest (no Plain secrets on disk).
- [x] `oxys` CLI is Portage-owned and staged into the image.
- [ ] Unsupported manifest fields fail validation or are clearly documented.
- [ ] `oxys-login` is built against the target and uses one supervised PAM flow.

### Quality gates

- [ ] Formatting, strict Clippy, tests, doctests, package checks, ShellCheck,
      dependency audit, and license checks pass in CI.
- [x] Installer has focused automated tests (7 unit tests).
- [ ] Login has focused automated tests.
- [ ] A clean release candidate passes the QEMU install/reboot matrix.
- [ ] README, config reference, disk/install docs, and changelog agree with code.

### Build and artifacts

- [ ] Release inputs are pinned and captured in a build manifest.
- [x] Crate workspace version is `0.1.0`.
- [ ] Version is consistent across tag, crates, `/etc/os-release`, and ISO name.
- [ ] Root licensing, security policy, third-party notices, and SBOM exist.
- [ ] ISO, checksums, build manifest, logs, and optional signature are published.
- [ ] The release tag points to a clean worktree and the recorded source SHA.

## Suggested implementation order

1. Fix package failure semantics and Portage fragment directory layouts.
2. Fix stale manifest compile, credential redaction, and typed confirm.
3. Gate or disable remote custom configs; typed install worker result.
4. Rebuild `oxys-login` in the target environment and fix its PAM lifecycle.
5. Get one clean ISO through install, reboot, login, offline, and security tests.
6. Make formatting/Clippy/CI green, then update docs and release metadata.
7. Pin inputs, generate the versioned artifact/provenance bundle, and tag.

## Bottom line

Since the July 13 review, the project crossed from “promising resolver with a
dangerous installer” into **“installer that looks intentional and safer.”**
Several of the worst P0s (root inheritance, re-entrant wipe, blind wipe before
plan, missing managed state/CLI) are addressed in code.

What still blocks a public 0.1 is less “feature missing” and more
**truthfulness and proof**:

- installs can report success while packages were skipped
- credentials / remote code / confirmation UX still need hardening
- no demonstrated end-to-end ISO install

## Worktree note

Local `main` was eight commits ahead of `origin/main` at review time. That is
normal during preparation; the release candidate should eventually be a clean,
reviewed commit with a version-matching tag.
