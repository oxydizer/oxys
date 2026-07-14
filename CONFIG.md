# Oxys OS Configuration Wiki

This document serves as the comprehensive reference guide and wiki for the declarative configuration options in Oxys OS.

In Oxys OS, the system configuration is defined as a pure Rust DSL using the [SystemManifest](oxys/src/manifest.rs#L14-L45) struct (aliased as `Oxys`). Running `oxys compile` (or compiling the user config crate) generates a verified, checksum-locked `manifest.toml` which is then consumed by the installer and package-management engines.

---

## Configuration Overview

The root of any Oxys configuration is [SystemManifest](oxys/src/manifest.rs#L14-L45) (alias [Oxys](oxys/src/manifest.rs#L47)).

```rust
pub struct SystemManifest {
    pub os: Os,
    pub disk: Disk,
    pub hardware: Hardware,
    pub kernel: Kernel,
    pub journal: Journal,
    pub environment: Vec<String>,
    pub packages: Vec<Package>,
    pub compiler: Compiler,
    pub init_system: InitSystem,
    pub bootloader: Option<Bootloader>,
    pub display_stack: Option<DisplayStack>,
    pub audio_stack: Option<AudioStack>,
    pub session: Session,
    pub prefer_binary: bool,
    pub services: Services,
    pub users: Vec<User>,
}
```

Below is the detailed reference for each block.

---

## 1. Operating System (`os`)
The `os` block specifies high-level system parameters like hostname, locales, shell preferences, and the standard C library.

* **Struct:** [Os](oxys/src/manifest.rs#L110-L121)

| Field | Type | Default | Description | Status |
| :--- | :--- | :--- | :--- | :--- |
| `hostname` | `String` | `""` | The system hostname. | 🚧 **Coming Soon** (Parsed/validated but not written to target `/etc/hostname`) |
| `timezone` | `String` | `""` | Timezone (e.g., `"Europe/London"`). | 🚧 **Coming Soon** (Parsed/validated but not written to `/etc/localtime`) |
| `locale` | `String` | `""` | System locale (e.g., `"en_US.UTF-8"`). | 🚧 **Coming Soon** (Parsed/validated but not written to `/etc/locale.gen`) |
| `shell` | [Shell](oxys/src/manifest.rs#L714-L718) | `Shell::Bash` | Default system shell (`Bash`, `Zsh`, `Fish`). | 🚧 **Coming Soon** (Parsed/validated but not yet provisioned/configured) |
| `libc` | [Libc](oxys/src/manifest.rs#L676-L678) | `Libc::Glibc` | The system C library (`Glibc`). | 🟢 **Fully Implemented** |

---

## 2. Disk and Partitioning (`disk`)
The `disk` block defines storage devices, filesystems, subvolumes, swap configurations, and volume management rules.

* **Struct:** [Disk](oxys/src/manifest.rs#L124-L141)

| Field | Type | Default | Description | Status |
| :--- | :--- | :--- | :--- | :--- |
| `device` | `String` | `""` | Target block device (e.g., `"/dev/nvme0n1"`). | 🟢 **Fully Implemented** |
| `layout` | [DiskLayout](oxys/src/manifest.rs#L728-L733) | `DiskLayout::Btrfs` | Filesystem layout (`Btrfs`, `LuksBtrfs`, `Zfs`, `Ext4`). | 🟢 **Zfs / Ext4 Fully Implemented**<br>⚠️ **Btrfs / LuksBtrfs: Coming Soon** (modeled, but rejected by the executor for now) |
| `encryption` | [Encryption](oxys/src/manifest.rs#L743-L750) | `Encryption::None` | Disk encryption strategy (`None`, `Password`, `Tpm`). | 🟢 **None: Fully Implemented**<br>⚠️ **Password / Tpm: Coming Soon** (modeled, but installer fails early if set, to prevent plain text leaks) |
| `subvolumes` | `Vec<Subvolume>` | Standard subvolumes | Btrfs-style subvolumes or ZFS datasets. | 🟢 **Fully Implemented** (automatically maps `@` to datasets/mounts) |
| `partitions` | [DiskPartitions](oxys/src/manifest.rs#L237-L242) | EFI (512MB) + Swap (zram) | Boot and swap partitions details. | 🟢 **Fully Implemented** |
| `snapshots` | `bool` | `true` | Enable automated system snapshots. | 🚧 **Coming Soon** (Parsed/validated but does not trigger snapshotting yet) |
| `zfs` | [ZfsOptions](oxys/src/manifest.rs#L254-L269) | Standard ZFS defaults | Specific options for ZFS pool and datasets. | 🟢 **Fully Implemented** (when layout is ZFS) |
| `ext4` | [Ext4Options](oxys/src/manifest.rs#L417-L422) | separate home + 50GB root | Specific options for Ext4 partitions. | 🟢 **Fully Implemented** (when layout is Ext4) |

### Subvolume Configuration
* **Struct:** [Subvolume](oxys/src/manifest.rs#L191-L196)
Used to define Btrfs-style subvolumes. Defaults are:
- `name: "@", mount: "/"`
- `name: "@home", mount: "/home"`
- `name: "@snapshots", mount: "/.snapshots"`
- `name: "@log", mount: "/var/log"`
- `name: "@pkg", mount: "/var/cache/portage"`

### Partition Details (`disk.partitions`)
* **Struct:** [DiskPartitions](oxys/src/manifest.rs#L237-L242)

- **`efi`** ([EfiPartition](oxys/src/manifest.rs#L199-L204)):
  - `size`: `u64` (Default: `512 * MB` / `536870912` bytes)
  - `mount`: `String` (Default: `"/boot/efi"`)
- **`swap`** ([SwapConfig](oxys/src/manifest.rs#L217-L228)):
  - `Partition { size: u64 }`: Dedicated swap partition. 🟢 **Fully Implemented**
  - `File { size: u64 }`: Swap file. 🚧 **Coming Soon** (Ignored by partition planner / fstab)
  - `Zram { size: u64 }`: Compressed swap-in-RAM. 🚧 **Coming Soon** (Default layout fallback, but configuration on target not yet provisioned)
  - `None`: No swap configured. 🟢 **Fully Implemented**

### ZFS Custom Configuration (`disk.zfs`)
* **Struct:** [ZfsOptions](oxys/src/manifest.rs#L254-L269)

- `pool`: `String` (Default: `"rpool"`)
- `boot_pool`: `String` (Default: `"bpool"`)
- `boot_pool_size`: `u64` (Default: `2 * GB`)
- `ashift`: `u8` (Default: `12`)
- `compression`: `String` (Default: `"zstd"`)
- `boot_compression`: `String` (Default: `"lz4"`)
- `datasets`: `Vec<ZfsDataset>` (Default defines `ROOT`, `ROOT/os`, `BOOT`, `BOOT/os`, `home`, `var/log`, `var/cache`, etc.)

---

## 3. Hardware (`hardware`)
The `hardware` block controls graphics and power optimization policies.

* **Struct:** [Hardware](oxys/src/manifest.rs#L442-L447)

| Field | Type | Default | Description | Status |
| :--- | :--- | :--- | :--- | :--- |
| `graphics` | `Graphics` | `Graphics::default()` | Mesa, DRM, NVIDIA, PRIME, and VM graphics policy. | 🟢 **Resolved policy and source-image capability validation implemented** |
| `power` | [Power](oxys/src/manifest.rs#L892-L897) | `Power::Auto` | Power daemon selector: `Auto`, `None`, `Tlp`, `AsusCtl`. | 🟢 **Fully Implemented** |

> [!NOTE]
> The retired TOML field `hardware.gpu` remains readable for one compatibility
> release and is converted to `Graphics`; newly generated manifests write only
> `hardware.graphics`.

`Graphics` contains:

- `mesa: MesaGraphics`, with `VideoCards::Auto` or `Explicit(Vec<VideoCard>)`
  and a `SoftwareRenderer` policy;
- `drm: Drm`, with `DrmDrivers::Auto` or `Explicit(Vec<DrmDriver>)`;
- `nvidia: Option<Nvidia>`, selecting `Proprietary`/`Nouveau`, modesetting,
  and `PrimeMode::{Disabled, Primary, Offload}`;
- `vm_support: VmGraphics::{None, Virgl, Vmware}`.

Before an install plan is returned, Oxys resolves `VideoCards::Auto` and
`DrmDrivers::Auto` from the graphics value captured in the manifest (bundled
configs use `detect_graphics()`), then validates immutable source-image
capabilities:

- each required Mesa `VIDEO_CARDS` capability must have its expected DRI or
  Vulkan artifact under the source root;
- each resolved DRM driver must have its required enabled/built-in kernel
  options in `boot/config-*` or `usr/src/linux/.config`;
- proprietary NVIDIA requires an installed `nvidia-drivers` capability and an
  `nvidia_drm` module matching the installed kernel ABI;
- proprietary NVIDIA and Nouveau are mutually exclusive;
- Virgl and VMware VM policies add their Mesa and kernel requirements.

Newly built ISO images record these facts in
`/usr/share/oxys/image-capabilities.toml`, generated from installed Portage
metadata, driver artifacts, and the injected kernel configuration. A SHA-256
sidecar is verified before the contract is trusted. VM entries also record the
required launch device (`virtio-vga-gl` for Virgl or `vmware-svga` for VMware),
so renderer support is not confused with a complete VM contract. Older images
without the contract remain supported through direct artifact and
kernel-config probing; a present but invalid or tampered contract is an
installation error.

Pass a compiled, checksummed manifest to both image builders with
`OXYS_GRAPHICS_MANIFEST=/path/to/manifest.toml`. They call
`oxys graphics-build-policy`, derive matching `OXYS_VIDEO_CARDS` and
`OXYS_DRM_DRIVERS` values from the resolved policy, and reject a mixture of
manifest-derived and manually supplied values. The two explicit variables
remain available as a lower-level override when no manifest is supplied.

Mesa is excluded from binary-package reuse and its installed USE
flags/artifacts are verified; requested DRM symbols are appended to the kernel
configuration, checked after `olddefconfig`, and recorded in artifact
metadata. This prevents an old cache entry or mismatched prebuilt kernel from
weakening the contract.

The rendered install plan includes the graphics decisions and capability
evidence before the first copy/mutation step. Missing capabilities stop
planning with the exact artifact or kernel option that is absent.

`PrimeMode::Primary` writes the NVIDIA variables globally.
`PrimeMode::Offload` keeps those variables exclusively in `prime-run`, detects
the integrated and NVIDIA render nodes, and pins Niri to the integrated node.
Oxys also installs `oxys-graphics-diagnostics` to report libseat selection,
DRM nodes/modules, and the active Mesa renderer.

Proprietary NVIDIA modules must match the recorded kernel ABI. Secure Boot
module signing/enrolment is not currently provisioned by Oxys; keep Secure
Boot disabled for proprietary NVIDIA images unless those modules are signed
and their key is enrolled separately.

## 4. Session (`session`)

The session block models how a user enters and runs a desktop session. The
installer resolves it before producing any target-mutating steps, records the
decisions and requirements in the rendered plan, and merges derived packages,
services, and groups into the effective install manifest.

| Field | Type | Default |
| :--- | :--- | :--- |
| `mode` | `SessionMode::{Auto, Text, Graphical}` | `Text` |
| `user` | `SessionUser::{FirstConfigured, Named, Index}` | `FirstConfigured` |
| `login` | `LoginFrontend::{Tty, OxysLogin}` | `OxysLogin { tty: 1, fallback_tty_login: true }` |
| `compositor` | `Compositor` | `Niri` |
| `desktop_shell` | `Option<DesktopShell>` | `None` |
| `seat` | `SeatBackend::{Auto, Seatd, Logind, Direct}` | `Auto` |
| `session_tracker` | `SessionTracker::{Auto, Elogind, Systemd, Pam, None}` | `Auto` |

The default is `SessionMode::Text`; graphical login is never inferred for a
new or omitted session block. Explicit `SessionMode::Auto` remains accepted
for older/third-party configurations, infers a graphical session from a
declared `gui-wm/niri` package, and emits a deprecation warning with the
migration path. An explicit `Text` selection always overrides package
inference. The initial
graphical implementation supports tty1, Niri/Wayland, and the conservative
Seatd/Elogind or Logind/Systemd compatibility combinations.

Migration is mechanical: replace `mode: SessionMode::Auto` with `Text` for a
console-only system, or declare `Graphical` plus the intended user, login,
compositor, shell, seat, and tracker as shown in
`docs/examples/desktop-session-proposed.fe2o3`.

For the OpenRC Seatd/Elogind desktop policy, resolution derives:

- Niri, D-Bus, Seatd, elogind, and selected shell/audio packages;
- `dbus`, `seatd`, and `elogind` services;
- `video`, `input`, and `audio` access for the selected session user;
- `LIBSEAT_BACKEND=seatd` and the Wayland XDG session environment;
- PAM → D-Bus → Seatd → Niri → PipeWire/WirePlumber → Noctalia startup order.

The generated `/etc/oxys/session.env` is consumed by `oxys-login`. The
`fallback_tty_login` setting controls whether Ctrl+Q can replace the greeter
with `/bin/login`. Before returning an install plan, graphical sessions also
preflight the immutable source-image requirements: executable `oxys-login`,
`agetty`, and (when fallback is enabled) `login` binaries, plus PAM login
configuration. An unsupported OxysLogin TTY is rejected even in text mode;
text mode does not silently normalize it to tty1.

---

## 5. Kernel Command-line (`kernel`)
The `kernel` block controls kernel-specific options.

* **Struct:** [Kernel](oxys/src/manifest.rs#L450-L453)

| Field | Type | Default | Description | Status |
| :--- | :--- | :--- | :--- | :--- |
| `cmdline` | `Vec<String>` | `vec![]` | Bootloader command-line arguments (e.g. `["quiet", "splash"]`). | 🟢 **Fully Implemented** |

> [!TIP]
> Extended kernel configuration fields (e.g., custom source packages, pinned kernel versions, and external module lists) are planned to prevent kernel-vs-driver mismatch issues.

---

## 5. Systemd Journal (`journal`)
The `journal` block is used to customize the behavior of the system logging journal.

* **Struct:** [Journal](oxys/src/manifest.rs#L455-L461)

| Field | Type | Default | Description | Status |
| :--- | :--- | :--- | :--- | :--- |
| `storage` | `JournalStorage` | `JournalStorage::Persistent` | Log storage type (`Auto`, `Persistent`, `Volatile`). | 🚧 **Coming Soon** (Parsed/validated but not written to target `journald.conf`) |
| `max_use` | `String` | `""` | Maximum disk space used by logs (e.g., `"2G"`). | 🚧 **Coming Soon** (Parsed/validated but not written to target `journald.conf`) |

---

## 6. Portage Compiler/Build Options (`compiler`)
Maps to `/etc/portage/make.conf` compilation optimization variables.

* **Struct:** [Compiler](oxys/src/manifest.rs#76-92)

| Field | Type | Default | Description | Status |
| :--- | :--- | :--- | :--- | :--- |
| `cflags` | `String` | `"-O2 -pipe"` | Base C compiler flags (`-march` is appended from `march`). | 🟢 **Fully Implemented** |
| `cxxflags` | `String` | `"-O2 -pipe"` | Base C++ compiler flags (`-march` is appended from `march`). | 🟢 **Fully Implemented** |
| `march` | [March](oxys/src/manifest.rs#L76-L91) | `March::Native` | Target CPU microarchitecture level, appended as `-march=` to CFLAGS/CXXFLAGS (`Native`, `X86_64`, `X86_64V2`, `X86_64V3`, `X86_64V4`). Any `-march` in `cflags`/`cxxflags` is overridden. | 🟢 **Fully Implemented** |
| `ldflags` | `String` | `"-fuse-ld=mold"` | Linker flags. | 🟢 **Fully Implemented** |
| `makeopts_jobs` | `usize` | Detect CPU count | Number of parallel make jobs. | 🟢 **Fully Implemented** |
| `emerge_jobs` | `usize` | `2` | Number of parallel emerge packages. | 🟢 **Fully Implemented** |
| `ccache` | `bool` | `true` | Enable/disable compiler cache (`ccache`). | 🟢 **Fully Implemented** |
| `optimisation` | [BuildOptimisation](oxys/src/manifest.rs#L60-L67) | `BuildOptimisation::Balanced` | Performance strategy (`Fast`, `Balanced`, `Performance`). | 🟢 **Fully Implemented** |

---

## 7. Package List (`packages`)
Defines the list of packages to manage in the system.

* **Struct:** [Package](oxys/src/manifest.rs#L484-L498)

| Field | Type | Default | Description |
| :--- | :--- | :--- | :--- |
| `package` | `String` | *(Required)* | Atom path (e.g., `"gui-wm/niri"`). |
| `version` | `Option<String>`| `None` | Restrict installation to a specific version. |
| `use_flags` | `Vec<String>` | `vec![]` | Custom USE flag modifiers (e.g., `["screencast", "-debug"]`). Forces building from source. |
| `keywords` | `Vec<String>` | `vec![]` | Accepted package keywords (e.g., `["~amd64", "**"]`). |
| `accept_licenses` | `Vec<String>` | `vec![]` | Accept specific package licenses (e.g. `["Mozilla"]`). |
| `binary` | `bool` | `false` | Prefer binary packages for this dependency. |
| `from_source` | `bool` | `false` | Force building this package from source. |

---

## 8. System Services (`services`)
Declares enabled or disabled daemons for the init system.

* **Struct:** [Services](oxys/src/manifest.rs#L464-L469)

| Field | Type | Default | Description | Status |
| :--- | :--- | :--- | :--- | :--- |
| `enabled` | `Vec<String>` | `vec![]` | Services to enable (e.g., NetworkManager, sshd). | 🟢 **Fully Implemented** |
| `disabled` | `Vec<String>` | `vec![]` | Services to disable. | 🟢 **Fully Implemented** |

---

## 9. Users and Authentication (`users`)
Models user accounts, group memberships, shells, and passwords. At install time
each user is created inside the target chroot (`useradd -m`) and its password is
applied via `chpasswd` over stdin (so secrets never appear in the plan, the
install log, or `manifest.toml`). Any user in the `wheel` group additionally
gets a `/etc/sudoers.d/wheel` drop-in (`sudo` must be present in `packages`).

* **Struct:** [User](oxys/src/manifest.rs#L541-L599)

**Builder:** `User::new("alex").wheel().shell(Shell::Bash).password(Password::Prompt)`

| Field | Type | Default | Description | Status |
| :--- | :--- | :--- | :--- | :--- |
| `name` | `String` | *(Required)* | Username. | 🟢 **Fully Implemented** |
| `groups` | `Vec<String>` | `vec![]` | Extra groups (e.g., `wheel`, `video`). `.wheel()` is a shortcut for adding `wheel`. | 🟢 **Fully Implemented** |
| `shell` | [Shell](oxys/src/manifest.rs#L783-L787) | `Shell::Bash` | User login shell (mapped to `/bin/bash`, `/bin/zsh`, `/usr/bin/fish`). | 🟢 **Fully Implemented** |
| `password` | [Password](oxys/src/manifest.rs#L1071-L1086) | `Password::None` | How the password is provisioned (see below). | 🟢 **Fully Implemented** |

### Password modes (`Password`)

| Variant | Behaviour | Notes |
| :--- | :--- | :--- |
| `None` | Account created locked (`passwd -l`). | Default. |
| `Plain(String)` | Plaintext applied via `chpasswd`. | ⚠️ Stored verbatim in `manifest.toml`; emits a **compile-time warning**. Prefer `Hashed`/`Prompt`. |
| `Hashed(String)` | Pre-hashed value (e.g. `openssl passwd -6`) applied via `chpasswd -e`. | Safe to commit; no plaintext. |
| `Prompt` | Installer prompts (masked, with confirmation) at install time. | Secret lives only in memory during install — never in the config or `manifest.toml`. |

---

## 10. Global / Top-level Fields

| Field | Type | Default | Description | Status |
| :--- | :--- | :--- | :--- | :--- |
| `init_system` | [InitSystem](oxys/src/manifest.rs#L646-L649) | `InitSystem::Openrc` | Selects system init: `Openrc` or `Systemd`. | 🟢 **Fully Implemented** (drives USE flag inferences) |
| `bootloader` | `Option<Bootloader>` | `None` (Grub) | Bootloader to use: `Grub` or `SystemdBoot`. | 🟢 **Fully Implemented** |
| `display_stack` | `Option<DisplayStack>`| `None` | Display stack: `Wayland` or `X11` (inferred if None). | 🟢 **Fully Implemented** |
| `audio_stack` | `Option<AudioStack>` | `None` | Audio stack: `Pipewire` or `Pulseaudio` (inferred if None). | 🟢 **Fully Implemented** |
| `prefer_binary` | `bool` | `false` | Global preference for binary packages when available. | 🟢 **Fully Implemented** |
| `environment` | `Vec<String>` | `vec![]` | Global environment variables list. | 🚧 **Coming Soon** (Parsed/validated but not written or loaded) |
