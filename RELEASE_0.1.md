# OxysOS 0.1 Release Readiness Review

Reviewed: 2026-07-13  
Snapshot: `main` at `cf57906` (`changed to gcc`).

## Verdict

The Rust resolver/CLI core is in encouraging shape, but the installable OS is
not ready for a public `0.1.0` tag yet. The core test suite passes and the
release binaries build. The remaining blockers sit at the highest-risk
boundaries: destructive disk handling, inherited credentials, execution of
custom config code, package-install failure semantics, PAM/session handling,
and the lack of a completed install/reboot test.

The safest release positioning is **0.1 developer preview**, with a deliberately
narrow support contract. Do not advertise general hardware support,
reproducible builds, ZFS-root, encryption, or fully declarative system state in
0.1 unless the corresponding work below is completed and tested.

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

## P0: must fix before a public release

### 1. Remove the live image's known root credential from the installed target

`oxys-iso/fsscript/fsscript.sh:80-88` sets the live root password to `oxys`.
The system-copy exclusions in `oxys/src/install/exec.rs` do not exclude
`/etc/shadow`, and the install plan never resets or locks root. All bundled
profiles also enable `sshd`.

Required change:

- Require at least one prompted wheel/admin user for installable profiles.
- Lock the installed root account, or provision it through a separate explicit
  root-credential flow.
- Explicitly disable SSH password login for root.
- Keep the throwaway live credential documented as live-media-only.

Acceptance:

- The installed `/etc/shadow` does not contain the live root hash.
- `passwd -S root` reports locked unless the operator explicitly configured it.
- A wheel user can log in locally and use `sudo`.
- Root password authentication over SSH is denied.

### 2. Make the destructive installer worker single-owner and non-reentrant

The install runs in `spawn_blocking` (`oxys-installer/src/app/tasks.rs:57-83`),
but Escape moves from `Installing` back to `Confirm`
(`oxys-installer/src/app/input.rs:182-232`). Calling `abort()` on a running
blocking task does not stop it. The current UI can therefore start a second
disk operation while the first is still running. Global quit has the same
unclear lifecycle, and a disconnected worker is inferred from strings instead
of an explicit result.

Required change:

- Disable back, quit, retry, and reboot while destructive work is active, or
  implement real cooperative cancellation at safe phase boundaries.
- Never start an install while any previous install worker is alive.
- Return a typed terminal result: `Succeeded`, `Failed`, `Cancelled`, or
  `Panicked`; do not infer success from channel closure or log prefixes.
- Await worker completion and show recovery instructions after failure.
- Apply the same ownership rule to overlapping compile/fetch jobs, whose
  `spawn_blocking` handles also cannot be cancelled by `abort()` once running.

Acceptance:

- Repeated Escape/Enter/quit input during provisioning cannot start another
  worker or leave commands running invisibly.
- A panic, dropped channel, or failed final cleanup can never reach `Done`.

### 3. Harden disk selection, preflight, and destructive confirmation

`oxys/src/disk/mod.rs:221-250` checks only direct `/proc/mounts` sources. It can
miss a physical disk used beneath LUKS/LVM/device-mapper, active swap, MD RAID,
ZFS, or another sysfs holder. The check is not repeated immediately before
`wipefs`, leaving a time-of-check/time-of-use gap. The TUI confirmation is a
single Enter press, unlike the CLI's typed device confirmation.

Required change:

- Resolve the complete block-device graph by major/minor number using sysfs or
  `lsblk`, including holders and slaves.
- Check mount descendants, `/proc/swaps`, device-mapper, MD, multipath, and ZFS
  membership.
- Verify minimum capacity for the planned partitions.
- Re-run preflight immediately before the first destructive command.
- Require the operator to type the exact device path (and ideally `ERASE`) in
  the TUI.
- Display stable identifiers: model, size, serial/WWN, transport, and whether
  the disk is removable.

Acceptance:

- Tests prove that a disk backing mounted LVM/LUKS, active swap, MD, and ZFS is
  rejected.
- Swapping or mounting a selected device after the review screen is caught by
  the final pre-wipe check.

### 4. Treat custom Rust configs as executable code, because they are

The custom picker accepts local paths plus unsigned `http://` and `https://`
URLs (`oxys-installer/src/app/tasks.rs:142-189`). The downloaded Rust is then
executed via `cargo run` (`oxys/src/compile.rs:153-180`) in a root installer.
The UI currently presents this as ordinary config compilation without a trust
boundary.

Minimum 0.1 change:

- Reject plain HTTP.
- State clearly that a `.fe2o3` file is executable Rust with the installer's
  privileges, and require a separate trust confirmation.
- Download to a bounded temporary file, validate size/hash, and atomically
  rename; never overwrite the built-in template in place.
- Prefer disabling remote configs for 0.1 unless a signature or pinned SHA-256
  is supplied.

Better design:

- Run config generation as an unprivileged user in a sandbox with no network,
  limited read-only hardware inputs, an empty temporary output directory, and
  a single validated manifest as output.

### 5. Prevent stale `manifest.toml` reuse

Both standalone config compilation (`oxys/src/compile.rs:163-190`) and the
current-directory CLI path (`oxys/src/cli/compile.rs:19-43`) accept an existing
valid `manifest.toml` if the newly executed program exits successfully without
writing one. In the installer, that can silently install a previously selected
profile.

Required change:

- Generate into a fresh temporary directory.
- Require a newly created output file from this invocation.
- Validate it, then atomically rename it into place.
- Add a regression test: existing valid manifest + successful no-output config
  must fail with `ManifestMissing`.

### 6. Make requested package convergence truthful and fatal when required

`oxys/src/install/portage.rs:11-115` deliberately converts missing metadata,
resolver conflicts, config-write errors, no network, emerge startup errors, and
emerge failures into warnings followed by `Ok(())`. The TUI can consequently
report a complete installation that does not match the selected manifest.

There is also a concrete Gentoo layout mismatch: the repository and live image
use directory-backed `package.accept_keywords` and `package.license`, while
`write_portage_plan_config` writes those paths as files
(`oxys/src/use_resolver/generate.rs:98-142`). That makes on-target package
configuration fail on the default image and then get downgraded to a warning.

Required change:

- Resolve and validate the complete package plan before disk confirmation.
- Distinguish packages already baked into the ISO from packages to fetch as
  binpkgs and packages to compile from source.
- Write only owned fragments such as `package.use/oxys`,
  `package.accept_keywords/oxys`, and `package.license/oxys` without deleting
  unmanaged state.
- Do not replace an unmanaged `make.conf` without an import/backup policy.
- Treat an absent requested package as install failure or an explicit
  `Partial` result that cannot display “Installation complete.”
- Make world-set registration required before persisting desired state; an
  oneshot package that is not registered can later be removed by depclean.

Acceptance:

- An intentionally unavailable custom package blocks completion before wipe
  where possible, and otherwise produces a clear partial/failure screen.
- Directory-backed Portage config is covered by integration tests.
- The package summary no longer equates “binary” with “already on the ISO” or
  “no download” unless that has been verified against the ISO package manifest.

### 7. Leave the installed machine as a managed Oxys system

The install plan ends with login setup and unmount/export
(`oxys/src/install/plan.rs:484-493`). It does not write
`/etc/oxys/current-manifest.toml`, even though subsequent package commands
require it (`oxys/src/cli/install.rs:160-165`). The ISO overlay also stages
`oxys-installer` and `oxys-login`, but not the `oxys` CLI binary.

Required change:

- Build and install the `oxys` CLI into the live image and target.
- Persist the effective post-install manifest before unmounting.
- Never persist prompted/plaintext credentials. Store a credential-redacted
  desired-state manifest with restrictive permissions and atomic writes.
- Run/apply the supported runtime configuration before persisting state.
- Verify `oxys --version`, `oxys diff`, `oxys apply`, `oxys update --dry-run`,
  and `oxys install <atom>` after first boot.

### 8. Add semantic/path-containment validation and enforce the real support boundary

The manifest models more than install/apply currently enact. Hostname,
timezone, locale, environment, journal policy, power policy, and several disk
options are parsed but not applied. Runtime `apply` mainly handles packages and
NVIDIA PRIME files, so a non-package change may be persisted as successful
without changing the machine. The OpenRC live image also cannot become a true
systemd base just because a custom manifest selects systemd.

The generated-manifest checksum is edit detection, not authentication or
semantic validation. Several values are later used as paths without containment
checks:

- OpenRC service names are joined beneath `etc/runlevels` and existing entries
  may be removed (`oxys/src/install/services.rs:41-75`). A service containing
  `../` can escape the intended directory.
- The EFI mount is only stripped of a leading slash before being joined to the
  target (`oxys/src/install/plan.rs:315-316` and
  `oxys/src/disk/mod.rs:145-161`).
- ZFS mountpoints are checked for an initial `/`, but parent traversal is still
  accepted before joining them beneath the target
  (`oxys/src/disk/zfs.rs:154-168,240-255`).

Required change:

- Add one semantic `SystemManifest::validate_for_install()` used before any
  planning or destructive operation.
- Reject parent/root path components, control characters/newlines, option-like
  values, slashes in service/user/group names, duplicate users, unsafe package
  atoms and pool/dataset names, and any target mount that escapes an explicit
  allowlist.
- Add containment tests proving that malicious EFI, ZFS, service, username,
  package, and kernel-command-line values cannot escape or become command
  options.
- For 0.1, either implement hostname/timezone/locale and other advertised
  fields, or reject/mark them unsupported rather than silently accepting them.
- Reject systemd, Btrfs, LUKS, ZFS-root, unsupported swap modes, and untested
  bootloader/layout combinations in the public installer.
- Make `CONFIG.md` status labels match execution, not just parsing/planning.

### 9. Build and operate `oxys-login` as part of the target system

`oxys-iso/scripts/build-installer-overlay.sh:86-102` builds `oxys-login`
dynamically against the host's glibc and PAM; the script itself notes that this
may be incompatible with the Gentoo target. PAM is then authenticated twice
using fallback services, and a successful `exec` means `PamSession::drop` and
`pam_close_session` never run (`oxys-login/src/main.rs:535-771`). Falling back
from a policy failure in `login` to `system-auth` can also bypass service-local
policy or double failure counters.

Required change:

- Build `oxys-login` inside the same pinned Gentoo userspace as the ISO.
- Remove or replace the `pam` dependency path that pulls `users 0.10.0`, which
  fails the current RustSec audit (RUSTSEC-2025-0040, plus unmaintained and
  unsound advisories). Reuse the existing raw PAM layer or select a maintained
  client without that dependency.
- Install one explicit `/etc/pam.d/oxys-login` policy and authenticate once.
- Distinguish PAM initialization failure from authentication/policy denial.
- Supervise the user session in a parent process so credentials and the PAM
  session are closed after Niri exits.
- Pass the intended initial username explicitly, rather than deriving `root`
  from the login manager's effective UID.

Acceptance:

- Wrong and correct password behavior, account expiry/lockout, logout, and a
  second login all work in the built ISO/target—not only on the build host.

### 10. Complete an end-to-end release candidate test

`oxys-iso/README.md:273-293` still states that the catalyst kernel-injection
path is unverified. In the current local catalyst output, a stage1 artifact is
present but no final ISO was found. `oxys-installer` and `oxys-login` currently
have zero unit tests.

Minimum release-candidate test:

1. Build the kernel/ZFS artifacts from a clean pinned input set.
2. Build a clean ISO and record every resolved input.
3. Boot the ISO in UEFI QEMU on the supported CPU baseline.
4. Install the desktop profile to a fresh qcow2 disk.
5. Shut down the live guest and boot from the installed disk only.
6. Log in through `oxys-login` and start Niri/Noctalia.
7. Verify networking, DNS, audio session startup, Portage repositories,
   `oxys --version`, current manifest state, and a dry-run update.
8. Verify root is locked, SSH root password auth is denied, and the configured
   admin can use `sudo`.
9. Repeat once offline for the baked default profile.
10. Save the commands, serial log, install log, artifact hash, and result with
    the release record.

## P1: release engineering and project polish

### Make CI boring and mandatory

Add CI gates for:

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-targets --locked
cargo test --workspace --doc --locked
cargo build --manifest-path oxys-installer/Cargo.toml \
  --release --target x86_64-unknown-linux-musl --locked
bash -n <every tracked shell script>
shellcheck <every tracked shell script>
cargo audit
cargo deny check
```

Add tests around installer/login state machines, disk guards, stale manifests,
directory-backed Portage config, credential redaction, package failure, and PAM
session behavior. These components are riskier than the resolver and currently
have no tests of their own.

### Pin or record every release input

The current build is kernel/ZFS-paired, but it is not byte-for-byte
reproducible. Floating inputs include:

- Rust `stable` in `rust-toolchain.toml`.
- Mutable Gentoo container tags and the current Catalyst package.
- Automatically selected newest Portage snapshot and current stage3 seed.
- Noctalia `HEAD` in `oxys-iso/git-sources.conf`.
- A shallow GURU clone from current HEAD.
- A freshly generated staged Cargo lockfile.
- Archive timestamps and gzip metadata.

For 0.1, pin Rust, container digests, Catalyst, stage3 path/hash, Portage
treeish, GURU, and live-Git commits; use `--locked`; and emit a build manifest.
Verify Gentoo's signed pointer and payload digest instead of merely parsing the
signed text, and record/verify SHA-256 for every kernel/ZFS archive before root
extraction.
For deterministic archives, also use `SOURCE_DATE_EPOCH`, fixed kernel build
metadata, sorted tar input, normalized owners/modes/mtimes, and `gzip -n`. Build
twice from clean directories and compare hashes before using “reproducible.”
Until then, use the more accurate phrase **kernel/ZFS-paired build pipeline**.

### Fix crate/workspace metadata and licensing

- Put MIT/Apache license files at repository root, covering the installer,
  login manager, scripts, ISO material, and build tooling—not only `oxys/`.
- Add inherited description, license, repository, readme, authors (if wanted),
  and `rust-version` metadata.
- Add `version = "0.1.0"` to the workspace `oxys` path dependency so
  `cargo package -p oxys-installer` can succeed, or mark installer/login
  `publish = false` if they are not crates.io products.
- Add third-party notices/licensing provenance for the distributed OS image.

### Give release artifacts stable identity and provenance

Produce at least:

- `oxysos-0.1.0-amd64-v3.iso`
- `SHA256SUMS` and, preferably, a signature/attestation
- a JSON build manifest containing source Git SHA, dirty flag, Rust version,
  container digests, stage3 hash, Portage treeish, external Git commits,
  kernel release/build ID, and ISO hash
- an SBOM for the Rust binaries and ideally the ISO package set
- Oxys-specific `/etc/os-release` data in both live and installed systems

Archive metadata should include and verify source SHA plus archive SHA-256, not
only matching build IDs and vermagic.

### Repair user-facing documentation before publishing

- Replace or remove `doc.md`; it says the TUI only formats disks, which is no
  longer true.
- Update `oxys/INSTALL_PIPELINE.md` and `oxys/DISK_CONFIG.md` from obsolete
  `oxys install --device ...` examples to `oxys install system ...`.
- Correct `CONFIG.md` defaults and stale source links. Examples include ext4
  now being the default layout and x86-64-v3 being the default `march`.
- Remove the stale statement that the copy phase does not install packages or
  enable services.
- Add an alpha warning, supported-platform table, destructive-install warning,
  quick start, recovery path, and known limitations to the root README.
- Add `CHANGELOG.md`, `CONTRIBUTING.md`, `SECURITY.md`, and a maintained release
  checklist. Add a code of conduct if public contribution is expected.

### Make state changes transactional

Portage config, world membership, runtime files, and the current manifest
should converge as one reported operation. Back up or stage generated files,
apply package changes, require world registration, apply runtime/service state,
then atomically persist the manifest. On failure, retain a journal and precise
recovery instructions rather than claiming success with partially updated
state.

### Additional hardening

- Create `/var/log/oxys-install.log` as `0600` and remove/exclude the live
  install log from the copied target.
- Disable `sshd` by default unless remote access is an intentional 0.1 feature;
  otherwise ship a project-owned SSH policy rather than relying on upstream
  defaults.
- Reject `Password::Plain` in release install flows. Surface config warnings in
  the TUI/CLI instead of discarding successful generator output, and do not
  describe password hashes as safe to commit.
- Replace `eval` of resolver output in `oxys-iso/build.sh` with structured
  parsing and validate architecture/build identifiers.
- Require a pinned local patch plus digest instead of piping an optional URL
  directly into `patch` in the package builder.
- Use RAII terminal guards in both TUIs so errors and panics restore raw mode,
  the alternate screen, and cursor state.
- Document that Secure Boot is not implemented and that the manifest checksum
  detects edits but does not authenticate the author.

## Current verification snapshot

| Check | Result on this snapshot |
| --- | --- |
| `cargo check --workspace` | Pass |
| `cargo test --workspace --all-targets` | Pass: 179 tests |
| `cargo test --workspace --doc` | Pass: 0 doctests |
| Installer musl release build | Pass; static PIE |
| Login release build | Pass; dynamically linked to host glibc/PAM |
| `cargo package -p oxys --allow-dirty` | Pass: 68 files, 123.8 KiB compressed |
| `cargo package -p oxys-login --allow-dirty --no-verify` | Pass with missing-metadata warning |
| `cargo package -p oxys-installer --allow-dirty --no-verify` | Fail: `oxys` dependency has no version requirement |
| Bash syntax for tracked `*.sh` | Pass |
| `git diff --check` | Pass |
| `cargo fmt --all -- --check` | **Fail**: widespread formatting drift |
| Strict workspace Clippy | **Fail**: 41 `oxys` diagnostics plus 4 `oxys-login` diagnostics |
| RustSec dependency audit | **Fail**: `users 0.10.0` via `pam 0.8.0` (RUSTSEC-2025-0040, RUSTSEC-2023-0040, RUSTSEC-2023-0059) |
| `cargo deny` | Not run; tool/config is absent |
| ShellCheck | Not run; tool is absent |
| Clean catalyst ISO build | **Not demonstrated** |
| ISO install + reboot + login | **Not demonstrated** |

Toolchain used for this snapshot: `rustc 1.96.1`, `cargo 1.96.1`.

## Release checklist

### Scope and safety

- [ ] The 0.1 support contract is written in README and enforced in validation.
- [ ] Installed root never inherits the live password.
- [ ] Every shipped profile creates a usable admin account.
- [ ] TUI uses typed destructive confirmation.
- [ ] Disk graph, swap, holders, and last-moment preflight tests pass.
- [ ] Destructive background work cannot be re-entered or abandoned invisibly.
- [ ] Custom executable configs have an explicit trust policy.
- [ ] Stale manifest regression test passes.

### Install correctness

- [ ] Portage fragment handling works with normal Gentoo directory layouts.
- [ ] Requested package failures cannot produce a success screen.
- [ ] Default desktop install succeeds offline from baked contents.
- [ ] Effective, credential-redacted manifest is persisted atomically.
- [ ] `oxys` CLI is installed and usable after first boot.
- [ ] Unsupported manifest fields fail validation or are clearly documented.
- [ ] `oxys-login` is built against the target and uses one supervised PAM flow.

### Quality gates

- [ ] Formatting, strict Clippy, tests, doctests, package checks, ShellCheck,
      dependency audit, and license checks pass in CI.
- [ ] Installer and login have focused automated tests.
- [ ] A clean release candidate passes the QEMU install/reboot matrix.
- [ ] README, config reference, disk/install docs, and changelog agree with code.

### Build and artifacts

- [ ] Release inputs are pinned and captured in a build manifest.
- [ ] Version is consistent across tag, crates, `/etc/os-release`, and ISO name.
- [ ] Root licensing, security policy, third-party notices, and SBOM exist.
- [ ] ISO, checksums, build manifest, logs, and optional signature are published.
- [ ] The release tag points to a clean worktree and the recorded source SHA.

## Suggested implementation order

1. Fix root credentials, disk safety/confirmation, and worker re-entry.
2. Fix config execution/stale output and add the regression tests.
3. Fix Portage fragment layouts and package failure semantics.
4. Install the CLI, persist redacted state, and enforce the 0.1 manifest scope.
5. Rebuild `oxys-login` in the target environment and fix its PAM lifecycle.
6. Get one clean ISO through install, reboot, login, offline, and security tests.
7. Make formatting/Clippy/CI green, then update docs and release metadata.
8. Pin inputs, generate the versioned artifact/provenance bundle, and tag.

## Worktree note

The review began while the following user changes were still local; they were
committed as `cf57906` during the review and were included in the verification
snapshot:

- `oxys-build/podman/kernel/base.config`
- `oxys-build/podman/scripts/oxys-build-packages.sh`
- `oxys-iso/portage_confdir/make.conf`
- `oxys/src/use_resolver/generate.rs`

There were no existing tags, and local `main` was three commits ahead of
`origin/main`. That is normal during preparation; the release candidate should
eventually be a clean, reviewed commit with a version-matching tag.
