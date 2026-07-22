# `.oxys` Package Format and Fast ISO Composition Plan

Status: design proposal, not yet implemented
Primary use case: build the OxysOS live ISO without emerging the desktop and
installer package closure on every ISO build
Secondary use case: install and update Oxys-native prebuilt packages while
retaining Portage as the Gentoo compatibility and source-build backend

## Executive recommendation

Add `.oxys` as an **immutable binary package format**, not as a replacement for
ebuilds or Portage.

Portage's VDB (`/var/db/pkg/`) is the **single source of truth** for every
installed package — both Gentoo-derived and native Oxys packages. The `.oxys`
composer writes real, complete VDB entries so the installed system is
indistinguishable from one built by `emerge`. Oxys keeps a lightweight
plain-file sidecar recording which packages it installed via `.oxys` artifacts,
but this is informational — VDB is authoritative.

The first useful release should do the following:

1. Compile the existing Rust system configuration into `manifest.toml` as it
   does today.
2. Resolve the requested live-image package set against a signed Oxys repository
   index and write an `oxys.lock` containing exact package builds and hashes.
3. Fetch prebuilt `.oxys` artifacts and compose them into a stage3-derived root.
4. Write complete Portage VDB entries captured from the reference build so that
   Portage fully understands what is installed.
5. Hand that already-composed root to catalyst for the ISO-only work: kernel
   injection, live initramfs, squashfs and ISO generation.
6. For packages absent from the Oxys repository, prompt the user and let them
   choose: check the binhost, build from source, or skip.

Use **Resolvo** for the `.oxys` artifact graph. It has a generic package-manager
interface, supports lazy metadata loading and is designed around a CDCL SAT
solver. Those properties suit variants, virtual capabilities, conflicts and a
large binary repository better than a name/version-only resolver. PubGrub is a
credible alternative and has excellent explanations and flexible package,
version and virtual-package types, but it is the simpler fit only if the Oxys
model remains close to ordinary name/version dependencies. The implementation
should hide either solver behind an internal trait and validate the choice with
the same fixture corpus before committing the lockfile schema to it.

This proposal deliberately does **not** use Resolvo to replace Portage's solver.
Portage dependency expressions include SLOT/sub-slot operators, blockers,
USE-conditional dependencies, repository/profile state and rebuild semantics.
Portage remains authoritative for any package that goes through `emerge`.

Relevant upstream material:

- [Resolvo repository and design summary](https://github.com/prefix-dev/resolvo)
- [Resolvo Rust API documentation](https://docs.rs/resolvo/latest/resolvo/)
- [PubGrub Rust implementation](https://github.com/pubgrub-rs/pubgrub)

## What changes compared with the current project

Today the package path is approximately:

```text
Rust config
  -> oxys compile
  -> checksum-locked manifest.toml
  -> Portage metadata + Oxys USE policy
  -> PortagePlan
  -> package.use / package.accept_keywords / make.conf
  -> emerge
```

The ISO path currently starts from a stage3, uses catalyst's
`livecd-stage1` package list to merge the live package set, then uses
`livecd-stage2` to inject the kernel/ZFS artifacts and make the ISO. Official
Gentoo binpackages help when their profile and USE choices match, but the custom
Wayland desktop and overlay packages still cost time. The installed system is
then an rsync copy of the live root, and any requested package additions are
planned and emerged inside the target chroot.

The relevant existing integration points are:

- `oxys/src/compile.rs`: compiles the Rust DSL to `manifest.toml`.
- `oxys/src/manifest/packages.rs`: describes package requests and Portage
  preferences.
- `oxys/src/use_resolver/`: produces Portage USE/keyword policy and emerge
  targets.
- `oxys/src/install/portage.rs`: performs the current target-chroot fallback.
- `oxys/src/install/plan.rs`: inserts `EmergePackages` after copying the live
  root.
- `oxys-iso/specs/installcd-stage1.spec`: owns the current live package list.
- `oxys-iso/specs/installcd-stage2.spec`: performs kernel injection and ISO
  production.
- `oxys-build/`: already establishes the useful pattern of immutable,
  build-ID-tagged, verified artifacts.

The proposed path becomes:

```text
Rust config
  -> manifest.toml
  -> resolve against .oxys repository
          |
          +---> all dependencies satisfied
          |       |
          |       v
          |     oxys.lock -> fetch -> compose root -> catalyst stage2
          |
          +---> missing dependency
                  |
                  v
                prompt user: binhost / source / skip
                  |
                  v
                emerge for that package
```

## Goals

- Make unchanged ISO builds spend time downloading/verifying/extracting
  artifacts rather than compiling the same packages again.
- Keep the Rust DSL as the user-facing declaration of the desired system.
- Produce a deterministic lockfile so an ISO can be recreated from the same
  repository snapshot and artifact hashes.
- Preserve Portage VDB as the single authoritative package database. A system
  composed via `.oxys` must be indistinguishable from one built by `emerge`.
- Users can run `emerge -uDN @world` at any time without breaking anything.
- Support atomic-enough install, upgrade and rollback behavior: a failed
  package must not leave an untracked half-install behind.
- Preserve package/file ownership and detect collisions before mutating a root.
- Make architecture, CPU baseline, libc, init system, ABI and kernel coupling
  explicit compatibility dimensions.
- Support offline ISO builds after repository metadata and artifacts have been
  prefetched.
- Reuse the kernel/ZFS build-ID discipline already present in `oxys-build`.

## Non-goals for version 1

- Reimplementing all of Portage's dependency solver.
- Replacing ebuilds as the way Gentoo-derived packages are built.
- Executing arbitrary package scripts as root from downloaded archives.
- Automatic fallback logic between resolvers. If a dependency is missing from
  the `.oxys` repo, the user decides what to do.
- Supporting multiple installed versions/slots of every native package on day
  one. The metadata can represent slots, but the first installer may reject
  multiple selected instances unless explicitly supported.
- Delta packages, peer-to-peer distribution, remote builds or transparent
  Nix-style source builds.
- Making every historical Gentoo binpackage directly installable as `.oxys`.
- Replacing catalyst in the first milestone. Removing catalyst can be considered
  after the root composer has proven reliable.

## Package model

### Two artifact kinds

The format should support two declared kinds while sharing one container:

1. `package`: one logical installable unit with dependencies, provided
   capabilities and owned files.
2. `bundle`: a convenience root/image layer containing an already-resolved set
   of package records. The ISO builder can use bundles for the base or desktop
   closure when maximum composition speed matters.

Bundles must not hide their members. Their metadata includes the exact package
records and file ownership map, so `oxys query`, upgrades and Portage
interoperability still operate on real package identities.

The initial implementation should prioritize `package` artifacts and optionally
publish a generated `oxys-live-desktop` bundle as a performance optimization.
The lockfile remains the source of truth either way.

### Identity

An artifact identity is more than `name-version`:

```text
namespace/name@version#revision:slot
  + target triple
  + CPU baseline
  + libc and ABI
  + init-system variant
  + feature set
  + build ID
```

Example:

```text
gentoo/gui-wm/niri@25.11-r1#2:0
target=x86_64-unknown-linux-gnu
cpu=x86-64-v3
libc=glibc-2.40
init=openrc
features=[pipewire,screencast,wayland,-systemd]
build_id=sha256:...
```

The filename is a display convenience, not identity or trust:

```text
niri-25.11-r1-r2-x86_64-v3.oxys
```

The repository index and internal metadata carry the canonical identity.

Use namespaces to prevent accidental equivalence:

- `oxys/*` for native Oxys packages.
- `gentoo/<category>/<package>` for packages converted from a Portage build.
- `bundle/*` for generated image bundles.
- `cap/*` for virtual capabilities used only by the solver, such as
  `cap/init`, `cap/libc`, `cap/ssl` or `cap/desktop-portal`.

### Versions and constraints

Do not force Gentoo versions into SemVer. The repository record declares a
version scheme:

```toml
version = "25.11-r1"
version_scheme = "gentoo"
```

Native packages can use `version_scheme = "semver"`. The resolver adapter must
compare versions through the declared scheme and reject constraints that cross
incompatible schemes. For the MVP, dependencies should normally identify an
exact namespace, so cross-scheme comparison is unnecessary.

Supported constraint forms should begin small:

- exact version/build;
- inclusive/exclusive version ranges within one version scheme;
- slot equality;
- capability requirements;
- explicit conflicts.

The lockfile always resolves these to an exact artifact digest.

### Compatibility dimensions

Compatibility is solver input, not an installer-time guess. At minimum every
artifact declares:

- target OS and architecture;
- CPU baseline (`x86-64`, `v2`, `v3`, `v4`, or a named native target);
- libc family and minimum ABI version;
- init system compatibility (`openrc`, `systemd`, or `any`);
- required kernel ABI/release for kernel modules;
- package slot and optional sub-slot;
- enabled build features/USE flags;
- required filesystem layout features, only when unavoidable.

For ordinary userland, avoid encoding a full host snapshot. Over-constraining
every artifact would destroy reuse. Kernel release and build ID should be
mandatory only for kernel-coupled packages such as `zfs-kmod`.

## `.oxys` version 1 container

### Framing

Use a small purpose-built framed container rather than relying on the filename
or treating an arbitrary tarball as trusted. A reader can inspect metadata and
limits before allocating or extracting the payload.

All integers are little-endian:

```text
offset  size  field
0       8     magic = "OXYS\0PKG"
8       2     format_major = 1
10      2     format_minor = 0
12      4     flags
16      8     metadata_length
24      8     file_table_length
32      8     payload_length
40      N     UTF-8 metadata.toml
...     M     canonical file table
...     P     zstd-compressed deterministic tar payload
```

The implemented package MVP assigns flag bit 0 to `hardlinks-present`. Readers
reject unknown flag bits and reject a mismatch between this bit and type `4`
file-table records.

Rules:

- Major-version mismatch is a hard error. Unknown optional minor fields can be
  ignored only when their feature bit permits it.
- Lengths have configured upper bounds before allocation.
- The decompressed payload size and file count are declared and bounded.
- The payload is a deterministic POSIX/pax tar stream compressed with zstd.
- Paths in the payload are relative root paths without a leading slash.
- Absolute paths, `..`, NUL bytes and extraction through symlink parents are
  rejected.
- Device nodes, setuid/setgid bits, capabilities and special files require
  explicit metadata permission and repository policy; the MVP can reject them.
- UID/GID in the payload are numeric. The metadata separately declares required
  system users/groups by name so IDs can be allocated or mapped consistently
  before extraction (see "UID/GID handling" below).
- mtime is normalized for reproducibility; runtime-mutated state does not belong
  in a package payload.
- Extended attributes, ACLs, Linux capabilities and hardlink relationships must
  be representable in the file table to accurately reproduce a real root.

A framed format is slightly more implementation work than `tar.zst`, but it
allows cheap metadata inspection, safe size checks and future compression
choices. A throwaway prototype may use `tar.zst`; artifacts published as format
v1 should use the stable framing above.

### Metadata

Illustrative `metadata.toml`:

```toml
format = 1
kind = "package"
namespace = "gentoo"
name = "gui-wm/niri"
version = "25.11-r1"
version_scheme = "gentoo"
revision = 2
slot = "0"
build_id = "sha256:4c..."

[target]
triple = "x86_64-unknown-linux-gnu"
cpu = "x86-64-v3"
libc = "glibc"
libc_min = "2.39"
init = "openrc"

[payload]
compression = "zstd"
uncompressed_size = 18300419
file_count = 127
sha256 = "..."
file_table_sha256 = "..."

[[dependencies]]
name = "gentoo/dev-libs/libinput"
constraint = ">=1.26,<2"
slot = "0"
kind = "runtime"

[[requires]]
capability = "cap/wayland"
constraint = ">=1"

[[provides]]
capability = "cap/wayland-compositor"
version = "1"

[[conflicts]]
name = "gentoo/gui-wm/niri"
constraint = "<25.11"

[[system_users]]
name = "niri"
group = "niri"
home = "/var/empty"
shell = "/sbin/nologin"

[build]
builder = "oxys-build"
method = "portage-source"
repo = "gentoo"
repo_commit = "..."
ebuild = "gui-wm/niri-25.11-r1.ebuild"
profile = "default/linux/amd64/23.0/no-multilib"
use = ["elogind", "pipewire", "screencast", "wayland", "-systemd"]
compiler = "gcc"
cflags = "-O2 -pipe -march=x86-64-v3"

[portage]
category = "gui-wm"
pf = "niri-25.11-r1"
repository = "gentoo"
vdb_payload = true
```

Metadata should include license identifiers, source URLs/checksums, build tool
versions and optional SBOM/provenance references. They are valuable for audits,
but only the fields that affect selection belong in the solver's hot index.

### File table

The file table is canonical, sorted by raw UTF-8 path bytes (never locale-aware)
and records:

```text
type | mode | uid | gid | size | sha256 | path | symlink-target | xattrs | hardlink-group
```

In the implemented MVP binary table, types are `1=file`, `2=directory`,
`3=symlink`, and `4=hardlink`; the variable target field stores either symlink
text or the package-relative canonical path for a hardlink. The bytewise-lowest
path in an inode group is the canonical regular file, and every alias points
directly to it. Hardlink records carry the canonical content size and SHA-256
but no inline tar data; the tar entry uses POSIX type `1`.

It serves four purposes:

- validate the payload before commit;
- preflight collisions without extracting into the live root;
- populate the Oxys receipt and verify against VDB CONTENTS;
- verify and repair installed files later.

Do not infer ownership by scanning the final root after installation. Generated
runtime files should be handled by typed triggers and recorded separately.

### UID/GID handling

Numeric UID/GID baked into artifacts from a build environment will not
necessarily match the target system's allocations. The metadata declares
required system users/groups by name in `[[system_users]]`. During composition:

1. Create or verify required system users/groups before extraction.
2. Map numeric IDs in the payload to the target system's allocations.
3. Record the mapping in the transaction for reproducibility.

For the ISO composition case where the entire root is being built fresh, the
composer controls user allocation and can use a fixed Oxys system-ID registry
to keep IDs stable across builds.

### Trust and signatures

The security boundary is the signed repository snapshot plus content hashes:

1. A repository root key signs a small snapshot/targets document.
2. The snapshot pins an index digest and expiry.
3. The index pins each `.oxys` artifact by SHA-256, size and identity.
4. `oxys.lock` repeats the artifact digest.
5. The installer verifies snapshot, index, artifact, internal payload and file
   table before writing the target root.

This is a simplified TUF-like starting point. Key rotation, threshold signing
and delegated targets should be designed before a public repository is treated
as production. Direct local-file installs require an explicit `--allow-unsigned`
or a detached signature from a trusted key; unsigned network installs are
never implicit.

## Repository and lockfile

### Repository layout

An HTTP/static-file repository is sufficient:

```text
repo/
  root.json
  snapshot.json
  index-v1.cbor.zst
  artifacts/sha256/ab/abcdef....oxys
  provenance/sha256/...
```

Artifact paths are content-addressed so mirrors and caches are safe. The index
contains the compact solver records and artifact URL/digest; full metadata stays
inside the artifact. Start with one index per supported target/CPU baseline if a
single index becomes large.

### `oxys.lock`

The Rust configuration expresses intent; the lockfile expresses an exact build.
An illustrative lock entry:

```toml
lock_version = 1
manifest_sha256 = "..."
repository_snapshot = "sha256:..."
target = "x86_64-unknown-linux-gnu"
cpu = "x86-64-v3"

[[package]]
id = "gentoo/gui-wm/niri@25.11-r1#2:0"
artifact = "sha256:..."
payload = "sha256:..."
reason = ["root:live-desktop"]

[[package]]
id = "gentoo/app-editors/neovim@0.11.1:0"
artifact = "sha256:..."
payload = "sha256:..."
reason = ["root:user-config"]
```

The lockfile contains only `.oxys` artifact references. Packages installed
via Portage (because the user chose binhost or source when prompted) are not
in the lockfile — they are tracked by VDB alone. This keeps the lockfile
fully reproducible: every entry has an exact artifact digest.

`oxys lock --frozen` must fail instead of changing any selection. ISO release
builds always use frozen mode. A frozen build with missing dependencies is a
build error, not a prompt — the repo must be complete for the declared closure.

## Rust declarative configuration

### Preserve the existing package API

Existing configurations using `Package::new("cat/pkg")` must continue to work.
The `prefer_binary` and `from_source()` mechanisms remain. When an `.oxys`
repository is configured, the planner checks it first. No new `PackageBackend`
enum is needed — the behavior is:

1. If an `.oxys` artifact exists with matching version and features, use it.
2. If not, prompt the user: check binhost, build from source, or skip.
3. `from_source()` always goes to Portage, bypassing the `.oxys` repo.

### Add repository configuration

Add repository policy to the top-level config:

```rust
Oxys {
    package_policy: PackagePolicy {
        repositories: vec![
            OxysRepository::new("stable", "https://packages.oxysos.org/v1")
                .key("sha256:..."),
        ],
        require_signed: true,
        frozen_lock: true,  // for ISO builds
    },
    packages: vec![
        Package::new("gui-wm/niri").use_flags(vec!["screencast"]),
        Package::new("app-editors/neovim").from_source(),
    ],
    ..Oxys::default()
}
```

Repository credentials or private keys never belong in `manifest.toml`.

### Separate live-image intent from installed-system intent

The current catalyst package list and the installer configuration can drift.
Introduce an `ImageManifest` in Rust for packages needed only by the live
medium:

```rust
ImageManifest {
    base: "gentoo-stage3-amd64-openrc",
    packages: vec![
        Package::new("sys-block/parted"),
        Package::new("sys-fs/dracut"),
        Package::new("bundle/oxys-live-desktop"),
    ],
    kernel_build: KernelBuild::OxysBuild { arch: "v3".into() },
}
```

Keep live-only tools separate from packages the installed user's config owns.
Both compile through the same typed Rust DSL and resolver, but generate distinct
lockfiles such as `oxys-iso.lock` and `oxys.lock`.

## Resolver architecture

### One resolver for `.oxys`, Portage for everything else

The resolver is simple: it walks the `.oxys` dependency graph and checks
whether each dependency is satisfied by another `.oxys` artifact in the repo
or by a package already present in VDB. There is no automatic fallback
coordinator or unified planner merging two solver results.

```text
OxysResolver
  |-- walks .oxys dependency tree
  |-- checks VDB for already-installed packages
  |-- reports missing dependencies to the user
  |
  User decides: binhost / source / skip
  |
  v
  emerge (for missing packages only)
```

For ISO builds in frozen mode, all dependencies must be satisfiable from the
`.oxys` repo. Missing dependencies are a build error — the repo must be
complete.

For installed-system operations, the resolver reports gaps and the user
chooses. This keeps the resolver small and avoids trying to understand
Portage's dependency semantics.

### Why Resolvo

Resolvo's generic interface and SAT model are a good match for:

- mutually exclusive OpenRC/systemd variants;
- CPU/libc/kernel compatibility represented as constraints;
- `provides` and virtual capabilities;
- conflicts and replacements;
- selecting one build from several feature variants;
- lazy loading of repository metadata.

PubGrub remains attractive for its mature human-readable derivations and
flexible package/version types. Before the end of the resolver milestone, run
the same test corpus through a thin prototype of both. Choose PubGrub instead if
the final package model deliberately excludes general capability/conflict SAT
constraints and its diagnostics are materially easier to integrate.

Do not expose solver-specific IDs, clauses or error types in public APIs or the
lockfile. Define an internal interface roughly like:

```rust
trait ArtifactSolver {
    fn solve(&self, problem: SolveProblem) -> Result<Solution, SolveFailure>;
}
```

`SolveFailure` should contain a solver-neutral explanation tree so CLI/TUI
output remains stable.

## Portage coexistence

### VDB is the single source of truth

Every package installed by `.oxys` composition gets a complete, real VDB entry
in `/var/db/pkg/<category>/<PF>/`. This entry is captured from the reference
build — not synthesized, not minimal. It includes USE, SLOT, RDEPEND, CONTENTS,
repository, and all other metadata Portage expects.

Consequences:

- `emerge -uDN @world` works at any time. Portage sees every package.
- `emerge --depclean` works. Portage understands the full dependency graph.
- `qcheck` works. CONTENTS matches the installed files.
- Users can freely mix `emerge` and `oxys` operations.

When Oxys runs after the user has done a manual `emerge`, it reads VDB to see
what changed, updates its own receipts to match reality, and moves on. It does
not try to downgrade or fight Portage's decisions.

### Version holds, not package.provided

A real VDB entry makes Portage treat the package as installed, but when the
tree carries a newer version, `emerge -uDN @world` would upgrade over the
Oxys-managed files and strand the receipt. To pin the ceiling, installation
registers a version hold `>category/PF` in the Oxys-owned fragment
`/etc/portage/package.mask/oxys`; removal prunes only the lines Oxys added
(tracked by a `hold_added` receipt field mirroring the world-entry model), and
deletes the fragment once no holds remain. A pre-existing identical line stays
user-owned.

`package.provided` was considered and rejected as the registration mechanism.
It removes the package from dependency calculation entirely: reverse
dependencies with slot/subslot operators (`:0/1.2=`) or USE-conditional deps
(`[ssl]`) cannot be satisfied; there is no CONTENTS, so `collision-protect`
and `qcheck` lose coverage; the package's own RDEPENDs become invisible, so
`--depclean` may remove libraries it needs at runtime; and it is a static
config line that goes stale silently. A real VDB entry plus a `>` mask keeps
every Portage subsystem working and merely pins the version ceiling.

A future upgrade flow must *replace* a package's hold line, never accumulate
holds: a stale `>category/pkg-old` line would mask a newer Oxys-installed
version and invite emerge to propose a downgrade.

### Native Oxys packages

Native `oxys/*` packages also get VDB entries — synthetic but well-formed
enough for Portage to understand them. This means `emerge --depclean` won't
try to remove their dependencies. The VDB entries need:

- `CATEGORY`, `PF`, `SLOT`, `EAPI`
- `CONTENTS` (file listing)
- `RDEPEND` (so Portage knows what they need)
- `KEYWORDS`, `USE`, `IUSE` (can be minimal)
- `repository` (use `oxys` as the repo name)

Test early that Portage handles these gracefully across `--depclean`,
`--pretend`, `qcheck` and `emerge -uDN @world`. The VDB entries should not
cause Portage to attempt re-emerging or updating these packages.

### Stage3 VDB import

The stage3 base already has a complete VDB. When composing onto a stage3, Oxys
must import awareness of its VDB CONTENTS into its collision detection. Without
this, the composer doesn't know what files stage3 owns and cannot detect
overlaps.

Import the stage3 VDB as read-only baseline state. Do not modify it. Oxys's
collision preflight reads both VDB CONTENTS from the stage3 and file tables
from `.oxys` artifacts being composed to detect conflicts before extraction.

### Centralized triggers

Avoid arbitrary `pre_install.sh` and `post_install.sh` scripts in v1. Package
metadata requests typed, idempotent triggers that Oxys owns, such as:

- `ldconfig`;
- `env-update`;
- desktop MIME database update;
- GSettings schema compilation;
- icon cache update;
- tmpfiles/sysusers application;
- initramfs rebuild;
- bootloader refresh.

Deduplicate triggers and run them after all files are staged. A trigger failure
fails the transaction or leaves a clearly recoverable `needs_repair` state.
Portage-derived packages whose correctness depends on arbitrary pkg_postinst
logic are not eligible for direct-install `.oxys` until that behavior has a
typed equivalent; they fall back to Portage merge.

## Oxys state records

### What Oxys tracks (and what it doesn't)

VDB is authoritative for what's installed. Oxys keeps a small sidecar that
records:

- which packages were installed via `.oxys` artifacts (so `oxys update` knows
  which packages it can fast-path);
- the artifact hash and build ID for each (so it can detect when a newer
  artifact is available);
- transaction history for rollback diagnostics.

This is not an ownership database. It is not authoritative for file lists or
dependencies. It is a record of provenance: "I installed this package from
this artifact at this time."

### State paths

```text
/var/lib/oxys/installed/
  <category>/<pf>.toml           one receipt per .oxys-installed package
/var/lib/oxys/transactions/      durable journals and recovery records
/var/cache/oxys/artifacts/       content-addressed downloaded .oxys files
/var/cache/oxys/indexes/         verified repository indexes
/var/log/oxys/                   human-readable transaction logs
```

An installed receipt is minimal:

```toml
package = "gentoo/gui-wm/niri@25.11-r1#2:0"
artifact = "sha256:..."
build_id = "sha256:..."
installed_at = "2026-07-15T03:15:29Z"
transaction = "20260715T031522Z-7e4b..."
```

When `oxys update` runs, it checks each receipt against the repo for newer
artifacts, and checks VDB to see if Portage has already updated the package.
If VDB shows a newer version than the receipt, Oxys updates the receipt (or
removes it if the package was updated via `emerge`).

### Reconciliation with manual emerge

When `oxys` detects that VDB differs from its receipts:

- Package was upgraded by `emerge`: update or remove the Oxys receipt. The
  package is no longer on the `.oxys` fast path unless a matching artifact
  exists for the new version. Also prune the now-stale version hold the
  receipt owned (`hold_added`), since its `>` line refers to the replaced
  version. (Such an upgrade requires the user to have removed or overridden
  the hold; reconciliation still cleans up after it.)
- Package was removed by `emerge --depclean`: remove the Oxys receipt and any
  version hold it owned.
- Package was added by `emerge`: ignore it. Oxys only tracks packages it
  installed.

This is a read-only reconciliation. Oxys never modifies VDB based on its own
records — VDB is always right.

## Transaction and install model

### Install algorithm

1. Acquire a global Oxys package lock and separately check Portage is not
   running.
2. Verify the plan against the repository snapshot.
3. Fetch and fully verify all artifacts before modifying the root.
4. Read file tables and preflight every collision against VDB CONTENTS
   (including stage3 baseline), disk-space requirements, and protected paths.
5. Create a durable transaction journal.
6. Allocate or verify required system users/groups and map UID/GID.
7. Extract into a staging directory on the target filesystem using safe
   `openat`-style traversal; never follow untrusted symlinks.
8. Snapshot or back up every path that will be replaced. On ZFS/Btrfs, a
   filesystem snapshot can optimize this, but correctness cannot require it.
9. Rename/copy staged paths into place in deterministic dependency order.
10. Write complete VDB entries for each package.
11. Write Oxys receipts.
12. Run deduplicated typed triggers.
13. Commit the transaction marker.
14. Delete backups asynchronously only after commit.

If a crash occurs, the next invocation reads the journal and either completes
the commit or restores the pre-transaction paths.

### Upgrade and removal

- Compute the full old-to-new transaction before changing files.
- Preserve user configuration using an explicit config-file policy similar in
  spirit to Portage CONFIG_PROTECT; never overwrite locally modified config
  without a merge artifact.
- Do not delete a path if its current hash differs from the recorded installed
  hash; report it as locally modified.
- Shared directories are not files and are removed only when empty and no owner
  remains.
- Shared identical files are allowed only when explicitly represented; differing
  hashes are a conflict.
- `replaces` authorizes ownership transfer only for named versions and paths.
- Keep the previous lockfile for rollback diagnostics.

## ISO pipeline design

### Target pipeline

```text
oxys-iso/config.rs
  -> oxys compile-image
  -> image-manifest.toml
  -> oxys resolve --lock oxys-iso.lock  (frozen: all deps must be in repo)
  -> oxys fetch --locked --offline-capable
  -> unpack Gentoo stage3 into composition root
  -> import stage3 VDB for collision awareness
  -> oxys compose --root <composition-root> --locked
  -> write VDB entries for all composed packages
  -> inject oxys-build kernel + zfs-kmod build pair
  -> validate root: VDB coherence, qcheck, emerge --pretend
  -> catalyst livecd-stage2
  -> squashfs + ISO
```

Stage1 package compilation disappears from routine ISO builds. Rebuilding
changed `.oxys` artifacts happens in the package build/publish pipeline, not
inside the ISO assembly pipeline.

### Incremental adoption with catalyst

Milestone 1 should keep catalyst stage2 because it already handles live-media
details. Add a composer that creates the exact stage1 tarball path catalyst
expects:

```text
<storedir>/builds/23.0-default/livecd-stage1-amd64-<stamp>.tar.xz
```

Then render only the stage2 spec against that root. This is less risky than
rewriting bootloader/initramfs/squashfs behavior at the same time as introducing
the package format.

The current kernel override remains unchanged initially. It already consumes a
verified `oxys-build` kernel/ZFS pair without emerge. Later, those artifacts may
be represented by `.oxys` metadata or a bundle, but the kernel build ID and
vermagic checks must remain authoritative.

### Base system

For the MVP, continue using an official, checksum-verified Gentoo stage3 as the
base. Record its URL, timestamp and digest in `oxys-iso.lock`. Do not immediately
split stage3 into hundreds of `.oxys` packages; compose the desktop/live additions
onto it first.

Longer term, publish a base bundle generated from the stage3 and its VDB. That
can make the entire root content-addressed while retaining an upstream bootstrap
story.

### Cache behavior

Cache keys must include:

- artifact digest, not filename;
- repository snapshot digest;
- image manifest and lockfile digest;
- stage3 digest;
- kernel/ZFS build ID;
- composer format/tool version.

An unchanged lockfile should cause zero dependency solving in frozen mode and
zero artifact downloads when the cache is warm. Extraction/compression remains,
but compilation does not.

## Build and publish pipeline

### Package builder

Add an isolated builder, preferably under `oxys-build/packages/`, that can:

1. Accept a Rust image/package-set declaration and pinned repository snapshot.
2. Create a clean reference root/container for each compatibility target.
3. Ask Portage to resolve and build packages, using official and local binpkg
   caches where possible.
4. Capture each installed package's owned files and complete VDB metadata.
5. Normalize metadata and payload deterministically.
6. Emit `.oxys`, provenance and optional SBOM.
7. Reinstall the artifact into an empty test root, write VDB, and verify
   Portage behavior: `qcheck`, `emerge --pretend`, `--depclean`.
8. Publish content-addressed artifacts, then atomically publish a signed index
   snapshot.

Never build packages during index publication. Publication is a metadata-only
operation over already-verified artifacts.

### Reproducibility levels

Record a useful distinction:

- `content_reproducible`: rebuilding produced the same artifact digest;
- `input_locked`: all known source inputs and tools are pinned, but output was
  not independently reproduced;
- `best_effort`: development artifact, excluded from release channels.

Release ISOs should consume only the first two levels, with policy configurable
for local development.

## Proposed crate/module and CLI shape

Start inside the existing `oxys` crate to avoid premature workspace splitting,
but maintain clear module boundaries:

```text
oxys/src/packages/
  model.rs          package IDs, versions, constraints, capabilities
  format.rs         framed .oxys reader/writer
  index.rs          signed repository metadata and cache
  lockfile.rs       stable lock schema
  solve.rs          solver-neutral interface
  resolvo.rs        Resolvo adapter
  compose.rs        alternate-root/image installation
  transaction.rs    journal and filesystem commit/recovery
  triggers.rs       typed post-install actions
  vdb.rs            VDB reading, writing and validation
  receipts.rs       lightweight Oxys provenance records
```

Potential CLI:

```text
oxys package inspect foo.oxys
oxys package verify foo.oxys
oxys package build --root <reference-root> cat/pkg
oxys repo index <artifact-dir>
oxys repo sign <snapshot>

oxys resolve --config config.rs --lock oxys.lock
oxys fetch --locked
oxys install --locked [--root /]
oxys remove <package>
oxys update [<package>]
oxys verify [<package>]
oxys repair

oxys image resolve --config oxys-iso/config.rs --lock oxys-iso.lock
oxys image compose --locked --root <dir>
```

`oxys apply` can eventually wrap resolve/fetch/install plus declarative system
actions. Initially, explicit package/image commands make failure boundaries and
testing easier.

## Detailed implementation plan

### Phase 0: decisions and compatibility spike

Deliverables:

- Write small fixtures for native packages, Portage-derived packages, variants,
  capabilities, conflicts and one kernel-coupled package.
- Prototype the solver-neutral model with both Resolvo and PubGrub.
- Build two or three packages in a clean Gentoo root, capture their complete VDB
  entries, install files and VDB into another root, and test Portage behavior.
- Verify `qcheck`, `emerge --pretend`, `emerge -uDN @world`, `--depclean` and
  `emerge --unmerge` all work correctly against restored VDB entries.
- Create synthetic VDB entries for two `oxys/*` native packages and verify
  Portage handles them gracefully.
- Define which typed triggers are required by the current live package set.
- Freeze package ID, version-scheme and target-compatibility rules.

Exit criteria:

- Resolver can explain a successful variant choice and an unsatisfiable case.
- A restored test root passes all Portage operations listed above.
- Synthetic `oxys/*` VDB entries survive `--depclean` and `@world` updates.
- The project documents which Portage operations are tested and passing.

### Phase 1: format, verification and lockfile

Deliverables:

- Implement bounded framed reader/writer and deterministic payload creation.
- Implement path-safe extraction into an alternate root.
- Implement canonical file tables with xattrs, capabilities and hardlink
  support.
- Implement UID/GID mapping from symbolic user/group declarations.
- Define repository index and `oxys.lock` v1 schemas.
- Implement local filesystem repositories before HTTP repositories.
- Add `inspect`, `verify`, `resolve` and `fetch` commands.

Exit criteria:

- The same input tree produces byte-identical artifacts.
- Corrupt header, metadata, table or payload is rejected.
- Traversal, symlink escape, special-file and decompression-bomb fixtures are
  rejected without modifying the destination.
- Frozen resolution is deterministic across repeated runs.

### Phase 2: resolver and Rust DSL integration

Deliverables:

- Add repository policy fields to the Rust DSL.
- Compile them into the generated TOML without breaking old configs.
- Implement the chosen resolver adapter and solver-neutral explanations.
- Implement user prompting for missing dependencies.
- Preserve the current `use_resolver` behavior for Portage fallback.

Exit criteria:

- Old manifests behave identically when no Oxys repository is configured.
- Custom USE requests select an exact artifact variant or prompt the user.
- Conflicts identify the root request and dependency chain in CLI/TUI output.
- Frozen mode fails on any missing dependency without prompting.

### Phase 3: VDB integration and transactional alternate-root install

Deliverables:

- Implement VDB writing from captured reference-build metadata.
- Implement VDB reading for collision preflight (including stage3 baseline).
- Implement synthetic VDB entries for `oxys/*` native packages.
- Implement Oxys receipt writing and reconciliation with VDB.
- Implement collision preflight and config-file policy.
- Implement durable transaction journals and crash recovery.
- Implement typed triggers required by the live desktop.
- Implement `install`, `remove`, `verify` and `repair` against `--root <dir>`.

Exit criteria:

- Composed root passes `qcheck` for every package.
- `emerge --pretend -uDN @world` reports no changes needed.
- `emerge --depclean` does not remove composed packages or their dependencies.
- Injected failures at every mutation step recover to either the old or new
  complete state.
- Conflicting files fail before mutation.
- Upgrade/removal preserves locally modified configuration.
- Repeated install is idempotent.

### Phase 4: package conversion/build pipeline

Deliverables:

- Build Gentoo-derived `.oxys` packages from a pinned clean root.
- Capture build provenance, USE/profile/repository data and complete VDB.
- Add package reinstall/reference-root comparison tests.
- Generate and sign a development repository index.
- Publish the current ISO stage1 package set for one target first:
  `amd64`, `x86-64-v3`, glibc, OpenRC.

Exit criteria:

- Every artifact can reconstruct its owned files and VDB from an empty root.
- Repository publication is atomic and old lockfiles remain fetchable.
- Gentoo-derived artifacts pass all Phase 0 Portage interoperability tests.
- Kernel/ZFS coupled artifacts cannot be selected against a mismatched build ID.

### Phase 5: ISO composer integration

Deliverables:

- Replace the hand-maintained stage1 package block with `oxys-iso/config.rs`.
- Commit or release-manage `oxys-iso.lock`.
- Add stage3 unpack + VDB import + `.oxys` composition + validation.
- Emit catalyst's expected stage1 tarball and invoke only stage2.
- Add fully offline mode and cache prefetch tooling.
- Record lockfile, stage3 digest and kernel build ID in ISO metadata.

Exit criteria:

- Warm-cache ISO build performs no `emerge` and no network access.
- Cold-cache ISO build downloads artifacts but performs no package compilation.
- The produced ISO boots in QEMU, launches the installer and completes ext4
  installation.
- The installed system has coherent VDB and passes all Portage operations.
- A package can subsequently be installed through `emerge`.
- `emerge -uDN @world` on the installed system works without errors.
- Output contents are equivalent to the current catalyst stage1 package set,
  modulo documented normalization.

### Phase 6: installed-system package operations

Deliverables:

- Enable `.oxys` install on live systems (not just alternate roots).
- Add online repository update, package upgrade and rollback behavior.
- Implement VDB reconciliation after manual `emerge` operations.
- Coordinate global locking with Portage.
- Surface transaction progress through the existing structured installer event
  stream.

Exit criteria:

- Mixed `.oxys` plus Portage installs work across install, upgrade, remove and
  reboot.
- Manual `emerge -uDN @world` after `.oxys` installs works without errors.
- `oxys update` after manual `emerge` upgrades handles receipt reconciliation.
- Preserved libraries, initramfs-relevant packages and boot-critical upgrades
  have dedicated integration tests.

### Phase 7: hardening and optional catalyst removal

Deliverables:

- Production key rotation/delegation and expiry handling.
- Repository mirror support and garbage-collection policy.
- SBOM/provenance policy and vulnerability/advisory metadata.
- Performance benchmarks and memory limits for large repository indexes.
- Evaluate replacing catalyst stage2 with direct squashfs/xorriso tooling only
  after output parity tests exist.

## Test strategy

### Unit and property tests

- Package ID/version parsing for SemVer and Gentoo versions.
- Constraint intersection and compatibility filtering.
- Deterministic metadata, file-table and archive encoding.
- Arbitrary malformed container input must never panic or allocate without
  bound.
- Path normalization and symlink-safe extraction.
- UID/GID mapping from symbolic declarations.
- Solver determinism independent of index iteration order.
- Solver explanation snapshots for conflicts and missing variants.
- Lockfile forward/backward compatibility rules.

### Integration tests

- Install/upgrade/remove packages in a temporary root with VDB.
- VDB entries pass `qcheck`, `emerge --pretend`, `--depclean`, `--unmerge`.
- Synthetic `oxys/*` VDB entries survive all Portage operations.
- File collision cases detected via VDB CONTENTS preflight.
- Crash/failure injection before and after every journal checkpoint.
- Config file modified/unmodified upgrade cases.
- Receipt reconciliation after simulated manual `emerge` changes.
- Mixed plan where some packages are `.oxys` and others are emerged.
- Offline resolve from a lockfile and offline compose from a warm cache.
- Kernel/ZFS build-ID mismatch rejection.
- Stage3 VDB import and collision detection against base packages.

### End-to-end tests

- Build the current live package closure once, publish to a local repository,
  compose a stage1 and run catalyst stage2.
- Boot ISO under QEMU for BIOS and UEFI paths used by the project.
- Install to ext4, boot the installed disk and run Portage checks.
- Run `emerge -uDN @world` on the installed system.
- Run a subsequent `emerge` install and an `oxys update`.
- Compare important executables, libraries, services, VDB entries and file modes
  with the current catalyst-emerged image.

### Performance gates

Track separately:

- package build/publish time;
- cold-cache compose time;
- warm-cache compose time;
- squashfs/ISO time;
- index load and solver time;
- artifact cache size and final ISO size.

The success metric is not merely a fast resolver. A warm ISO build should be
dominated by decompression, root validation and squashfs creation, with zero
package compilation.

## Risks and mitigations

### Portage VDB compatibility

Risk: VDB entries captured from a reference build are subtly wrong or
incomplete, causing `emerge` to misbehave.

Mitigation: capture complete VDB from real Portage builds, never synthesize
Gentoo-derived entries. Test every Portage operation (pretend, depclean,
unmerge, world update, qcheck) in Phase 0 and in CI. For native `oxys/*`
packages, test synthetic entries separately and conservatively.

### Synthetic VDB for native packages

Risk: Portage chokes on `oxys/*` VDB entries — tries to update them, can't
find an ebuild, fails during `@world` updates.

Mitigation: test this thoroughly in Phase 0. If Portage cannot handle them
gracefully, fall back to keeping native packages out of VDB and accepting
that `--depclean` won't understand their dependency contribution. This is
strictly worse but safe.

### Package hooks contain hidden correctness

Risk: direct extraction skips pkg_preinst/pkg_postinst behavior.

Mitigation: eligibility audit, typed triggers, reference-root comparisons and
Portage binpkg fallback for packages needing arbitrary hooks.

### Variant explosion

Risk: every USE/CPU/init combination creates an unmaintainable artifact matrix.

Mitigation: publish a small supported platform matrix, use package-local
features only where they affect output, choose a general x86-64-v3 release
baseline, and fall back to Portage for uncommon variants.

### Two sources of truth

Risk: catalyst spec package lists, Rust manifests and repository bundles drift.

Mitigation: move the live package declaration to one Rust `ImageManifest`,
generate the lockfile and stage1 input from it, and test that hand-written
catalyst package blocks are absent.

### Unsafe archive extraction

Risk: traversal, symlink races, device nodes or decompression bombs modify the
host/root unexpectedly.

Mitigation: bounded framed parsing, verified file tables, alternate-root safe
file-descriptor traversal, staging on the destination filesystem, and fuzzing.

### Repository compromise or rollback

Risk: a mirror serves a malicious artifact or an old vulnerable snapshot.

Mitigation: signed expiring snapshots, content-addressed artifacts, trusted
root/key rotation and monotonic snapshot/version checks. Lockfiles provide
reproducibility but must not silently disable expiry/security policy.

### Stage3 blind spot

Risk: collision detection misses files owned by stage3 packages because Oxys
doesn't know about them.

Mitigation: import stage3 VDB CONTENTS as read-only baseline state during
composition. Preflight all `.oxys` file tables against this baseline.

## Open decisions to settle during Phase 0

1. Can synthetic VDB entries for `oxys/*` packages survive all Portage
   operations, or should native packages stay outside VDB?
2. Is Resolvo's richer SAT model needed by the final v1 metadata, or does the
   fixture corpus show PubGrub provides a simpler implementation with better
   explanations?
3. Which exact Gentoo version parser/comparator will be treated as canonical?
4. Which live packages require post-install behavior beyond the proposed typed
   triggers?
5. Should the repository use CBOR or another canonical binary index encoding?
6. Is a generated desktop bundle worth the additional publishing complexity,
   or are individual artifact extraction times already small compared with
   squashfs creation?
7. What is the supported release matrix beyond the first
   `amd64/x86-64-v3/glibc/OpenRC` target?
8. What is the repository key ownership, offline backup and rotation process?
9. What is the fixed Oxys system-ID registry for deterministic UID/GID in
   ISO composition?

## Suggested first vertical slice

The smallest slice that proves the idea without risking installed systems is:

1. Implement local unsigned development repositories and the `.oxys` reader,
   writer, file table and safe alternate-root extraction.
2. Package three independent live tools and one small dependency chain from a
   clean reference root, capturing complete VDB.
3. Resolve them with the solver-neutral interface and write a lockfile.
4. Compose them onto a stage3 with VDB entries written.
5. Import stage3 VDB and verify collision-free composition.
6. Validate Portage queries in that root: `qcheck`, `emerge --pretend`,
   `emerge -uDN @world`, `--depclean`.
7. Generate the stage1 tarball catalyst expects and complete stage2.
8. Boot the ISO in QEMU.

After that works, expand the repository to the desktop closure and measure the
real build-time reduction. Only then enable `.oxys` writes to a normal installed
root. This ordering proves the core value — removing repeated ISO compilation —
while keeping the existing emerge path intact throughout development.
