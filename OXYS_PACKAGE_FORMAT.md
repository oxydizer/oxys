# `.oxys` Package Format and Fast ISO Composition Plan

Status: design proposal, not yet implemented  
Primary use case: build the OxysOS live ISO without emerging the desktop and
installer package closure on every ISO build  
Secondary use case: install and update Oxys-native prebuilt packages while
retaining Portage as the Gentoo compatibility and source-build backend

## Executive recommendation

Add `.oxys` as an **immutable binary package format**, not as a replacement for
ebuilds or Portage.

The first useful release should do the following:

1. Compile the existing Rust system configuration into `manifest.toml` as it
   does today.
2. Resolve the requested live-image package set against a signed Oxys repository
   index and write an `oxys.lock` containing exact package builds and hashes.
3. Fetch prebuilt `.oxys` artifacts and compose them into a stage3-derived root.
4. Preserve file ownership and, for artifacts converted from Gentoo packages,
   preserve enough Portage VDB metadata that Portage still understands what is
   installed.
5. Hand that already-composed root to catalyst for the ISO-only work: kernel
   injection, live initramfs, squashfs and ISO generation.
6. Continue to delegate packages absent from the Oxys repository, packages with
   custom USE flags, and explicitly source-built packages to the existing
   Portage planner and `emerge` runner.

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
For the first releases, Portage remains authoritative for the Gentoo graph;
Resolvo is authoritative only for published `.oxys` artifacts.

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
                         +-----------------------------+
Rust config              | Gentoo package not present, |
  -> manifest.toml       | source forced, or custom USE|
  -> unified planner ----+----------> PortagePlan -----> emerge
          |
          v
    Oxys repository index
          |
          v
      Resolvo solve
          |
          v
       oxys.lock
          |
          v
 fetch + verify `.oxys` artifacts
          |
          v
 transactional root composition
          |
          +----> live ISO root ----> catalyst stage2
          |
          +----> installed system (later phase)
```

## Goals

- Make unchanged ISO builds spend time downloading/verifying/extracting
  artifacts rather than compiling the same packages again.
- Keep the Rust DSL as the user-facing declaration of the desired system.
- Produce a deterministic lockfile so an ISO can be recreated from the same
  repository snapshot and artifact hashes.
- Support atomic-enough install, upgrade and rollback behavior: a failed
  package must not leave an untracked half-install behind.
- Preserve package/file ownership and detect collisions before mutating a root.
- Preserve Portage as a supported backend for arbitrary Gentoo packages,
  custom USE configurations and source builds.
- Make architecture, CPU baseline, libc, init system, ABI and kernel coupling
  explicit compatibility dimensions.
- Support offline ISO builds after repository metadata and artifacts have been
  prefetched.
- Reuse the kernel/ZFS build-ID discipline already present in `oxys-build`.

## Non-goals for version 1

- Reimplementing all of Portage's dependency solver.
- Replacing ebuilds as the way Gentoo-derived packages are built.
- Executing arbitrary package scripts as root from downloaded archives.
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
- UID/GID are numeric. The package may also declare required system users/groups
  separately so IDs can be allocated consistently before extraction.
- mtime is normalized for reproducibility; runtime-mutated state does not belong
  in a package payload.

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

[build]
builder = "oxys-build"
source = "portage"
repo = "gentoo"
repo_commit = "..."
ebuild = "gui-wm/niri-25.11-r1.ebuild"
profile = "default/linux/amd64/23.0/no-multilib"
use = ["elogind", "pipewire", "screencast", "wayland", "-systemd"]
compiler = "gcc"
cflags = "-O2 -pipe -march=x86-64-v3"

[portage]
compatible = true
category = "gui-wm"
pf = "niri-25.11-r1"
repository = "gentoo"
vdb_payload = true
```

Metadata should include license identifiers, source URLs/checksums, build tool
versions and optional SBOM/provenance references. They are valuable for audits,
but only the fields that affect selection belong in the solver's hot index.

### File table

The file table is canonical, sorted bytewise by normalized path and records:

```text
type | mode | uid | gid | size | sha256 | path | symlink-target
```

It serves four purposes:

- validate the payload before commit;
- preflight collisions without extracting into the live root;
- populate the ownership database;
- verify and repair installed files later.

Do not infer ownership by scanning the final root after installation. Generated
runtime files should be handled by typed triggers and recorded separately.

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
source = "oxys"
reason = ["root:live-desktop"]

[[package]]
id = "gentoo/app-editors/neovim@0.11.1:0"
source = "portage"
atom = "=app-editors/neovim-0.11.1"
reason = ["root:user-config"]
```

The lockfile may contain Portage fallbacks, but those entries are only fully
reproducible if the Portage repository commit, profile, USE state and distfile
hashes are also pinned. `oxys lock --frozen` must fail instead of changing any
selection. ISO release builds always use frozen mode.

## Rust declarative configuration

### Preserve the existing package API

Existing configurations using `Package::new("cat/pkg")` must continue to work.
Extend `Package` with a backend preference rather than create a second package
list:

```rust
pub enum PackageBackend {
    Auto,               // Oxys if an exact compatible artifact exists, else Portage
    OxysOnly,           // fail if no compatible .oxys artifact exists
    PortageBinaryFirst, // existing emerge --getbinpkg behavior
    PortageSource,      // existing from_source behavior
}

Package::new("gui-wm/niri")
    .backend(PackageBackend::Auto)
    .features(["screencast"])
```

`from_source()` remains as a compatibility builder that selects
`PortageSource`. Existing `use_flags()` behavior needs a clear rule:

- an exact `.oxys` variant with those flags can satisfy the request;
- otherwise `Auto` falls back to Portage source;
- `OxysOnly` reports which requested variant was unavailable.

### Add package policy and repositories

Add a top-level package policy without changing ordinary defaults:

```rust
Oxys {
    package_policy: PackagePolicy {
        default_backend: PackageBackend::Auto,
        repositories: vec![
            OxysRepository::new("stable", "https://packages.oxysos.org/v1")
                .key("sha256:..."),
        ],
        allow_portage_fallback: true,
        require_signed: true,
        frozen_lock: true,
    },
    packages: vec![
        Package::new("gui-wm/niri"),
        Package::new("app-editors/neovim").backend(PackageBackend::PortageSource),
    ],
    ..Oxys::default()
}
```

Repository credentials or private keys never belong in `manifest.toml`.

### Separate live-image intent from installed-system intent

The current catalyst package list and the installer configuration can drift.
Introduce an `ImageManifest` (or a `SystemManifest` target/profile field) in
Rust for packages needed only by the live medium:

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

### Keep two resolvers with a coordinator

Rename the conceptual responsibilities even if module names change later:

```text
UnifiedPlanner
  |-- OxysArtifactResolver (Resolvo)
  |     |-- repository packages
  |     |-- installed .oxys packages
  |     `-- target/policy virtual packages
  |
  `-- PortagePolicyResolver (existing use_resolver)
        |-- md5-cache parsing
        |-- USE/keyword policy
        `-- emerge target generation
```

The coordinator follows this algorithm:

1. Normalize every manifest request and validate contradictory policy.
2. Load trusted repository snapshot/index metadata.
3. Add target facts as locked virtual packages: architecture, CPU baseline,
   libc, init system, kernel build ID and allowed licenses.
4. Query `.oxys` candidates matching each `Auto`/`OxysOnly` root request.
5. Exclude incompatible candidates before solving.
6. Resolve the `.oxys` graph with installed versions and the previous lockfile
   preferred to minimize churn.
7. Move unresolved `Auto` roots to the Portage set; fail unresolved `OxysOnly`
   roots.
8. Check the boundary: Portage roots must not overwrite files owned by selected
   native `.oxys` packages, and `.oxys` dependencies cannot silently assume an
   untracked Portage provider.
9. Run the existing Portage policy planner for the fallback roots.
10. Emit one plan with ordered fetch, unpack, trigger and emerge phases plus an
    explanation for each backend choice.

### Why Resolvo is the default recommendation

Resolvo's generic interface and SAT model are a good match for:

- mutually exclusive OpenRC/systemd variants;
- CPU/libc/kernel compatibility represented as constraints;
- `provides` and virtual capabilities;
- conflicts and replacements;
- selecting one build from several feature variants;
- lazy loading of repository metadata;
- eventually solving bundles and individual artifacts together.

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

### Portage is not merely the last resolver candidate

Do not put every ebuild version into Resolvo and pretend that a successful solve
means Portage will accept the result. Instead, the boundary is explicit:

- `.oxys` dependencies must be satisfiable from `.oxys` artifacts or declared
  external capabilities.
- Portage roots and their transitive dependencies are resolved by Portage.
- The unified planner invokes `emerge --pretend` before mutation and performs a
  file/ownership conflict check against the selected `.oxys` state.
- A future Portage adapter can import more complete dependency expressions, but
  that is a separate project with its own conformance tests.

## Portage coexistence

This is the highest-risk part of the design and must be proven before `.oxys`
is used for the installed system.

### Native Oxys packages

Native `oxys/*` packages are owned only by the Oxys database. Portage does not
consider them installed. Therefore:

- native packages should initially install under collision-resistant locations
  such as `/usr/libexec/oxys`, `/usr/share/oxys` and declared `/usr/bin` links;
- the planner rejects overlaps with Portage-owned files unless an explicit,
  reviewed `replaces` relation exists;
- native packages must not claim to satisfy arbitrary Gentoo atoms;
- removing a native package only removes files still matching its recorded
  hash and not shared with another owner.

### Gentoo-derived `.oxys` packages

For a package built by Portage, merely copying its files is insufficient.
Portage uses `/var/db/pkg/<category>/<PF>/` to understand the installed package,
USE flags, SLOT, repository, dependency metadata and file contents.

The preferred build path is:

1. Build/install the package in an isolated reference root using the pinned
   Portage snapshot and profile.
2. Capture only files owned by that VDB entry.
3. Embed a validated VDB payload and provenance in the `.oxys` artifact.
4. During composition, install package files and the matching VDB entry in one
   transaction.
5. Run safe centralized triggers after the whole plan is present.
6. Validate the root with Portage queries and an `emerge --pretend` no-op check.

Do not synthesize minimal fake VDB entries. Either preserve the complete,
tested metadata contract from the reference root or mark the artifact
`portage.compatible = false` and restrict it to ephemeral image composition.

The spike must compare at least these approaches:

- importing complete VDB entries captured from a reference root;
- converting `.oxys` back into a local Gentoo binary package and asking Portage
  to install it;
- keeping `.oxys` image-only and using native Portage binpackages for all
  installed-system packages.

The second is safest for Portage semantics but still calls Portage's merge
path; it avoids compilation, however, which may already meet the installed
system requirement. The first gives the fastest ISO composer but needs strong
compatibility testing. A practical split is:

- ISO: direct `.oxys` extraction plus captured VDB, fully offline.
- normal installed system: Portage binpkg handoff for Gentoo-derived packages
  until direct extraction has passed upgrade/removal/preserved-libs testing.

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

## Transaction and ownership model

### State paths

Suggested paths:

```text
/var/lib/oxys/packages.db       SQLite installed package and ownership state
/var/lib/oxys/transactions/    durable journals and recovery records
/var/cache/oxys/artifacts/     content-addressed downloaded `.oxys` files
/var/cache/oxys/indexes/       verified repository indexes
/var/log/oxys/                 human-readable transaction logs
```

SQLite tables should cover packages, provided capabilities, dependencies,
files, shared ownership, transactions, operations, triggers and repository
snapshot provenance. Enable foreign keys, use WAL where appropriate, and
fsync the journal/commit boundary for real system installs.

### Install algorithm

1. Acquire a global Oxys package lock and separately check Portage is not
   running.
2. Verify the plan still matches the installed generation and repository
   snapshot.
3. Fetch and fully verify all artifacts before modifying the root.
4. Read file tables and preflight every collision, disk-space requirement,
   target constraint and protected path.
5. Create a durable transaction journal.
6. Extract into a staging directory on the target filesystem using safe
   `openat`-style traversal; never follow untrusted symlinks.
7. Snapshot or back up every path that will be replaced. On ZFS/Btrfs, a
   filesystem snapshot can optimize this, but correctness cannot require it.
8. Rename/copy staged paths into place in deterministic dependency order.
9. Install compatible VDB records and record ownership in SQLite.
10. Run deduplicated typed triggers.
11. Commit the database generation and transaction marker.
12. Delete backups asynchronously only after commit.

If a crash occurs, the next invocation reads the journal and either completes
the commit or restores the pre-transaction paths. A database transaction alone
is not sufficient because filesystem changes are outside SQLite.

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
- Keep the previous lockfile and package generation for rollback diagnostics.

## ISO pipeline design

### Target pipeline

```text
oxys-iso/config.rs
  -> oxys compile-image
  -> image-manifest.toml
  -> oxys resolve --target-root / --lock oxys-iso.lock
  -> oxys fetch --locked --offline-capable
  -> unpack Gentoo stage3 into composition root
  -> oxys compose --root <composition-root> --locked
  -> inject oxys-build kernel + zfs-kmod build pair
  -> validate root and VDB
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
3. Ask Portage to resolve and build missing packages once, using official and
   local binpkg caches where possible.
4. Capture each installed package's owned files and VDB metadata.
5. Normalize metadata and payload deterministically.
6. Emit `.oxys`, provenance and optional SBOM.
7. Reinstall the artifact into an empty test root and compare its owned paths
   with the reference root.
8. Run Portage interoperability tests.
9. Publish content-addressed artifacts, then atomically publish a signed index
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
  format.rs         framed `.oxys` reader/writer
  index.rs          signed repository metadata and cache
  lockfile.rs       stable lock schema
  solve.rs          solver-neutral interface
  resolvo.rs        Resolvo adapter
  plan.rs           unified Oxys/Portage coordinator
  transaction.rs    journal and filesystem commit/recovery
  ownership.rs      SQLite state and collision checks
  triggers.rs       typed post-install actions
  portage_bridge.rs VDB/binpkg handoff and interoperability checks
  compose.rs        alternate-root/image installation
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
  entries, install into another root and test Portage behavior.
- Compare direct VDB restoration with conversion/handoff to a local Gentoo
  binpkg.
- Define which typed triggers are required by the current live package set.
- Freeze package ID, version-scheme and target-compatibility rules.

Exit criteria:

- Resolver can explain a successful variant choice and an unsatisfiable case.
- A restored test root passes `qcheck`/equivalent ownership validation and
  sensible `emerge --pretend`, upgrade and unmerge scenarios.
- The project chooses and documents direct extraction versus Portage binpkg
  handoff separately for ISO and installed-system paths.

### Phase 1: format, verification and lockfile

Deliverables:

- Implement bounded framed reader/writer and deterministic payload creation.
- Implement path-safe extraction into an alternate root.
- Implement canonical file tables and payload/file hashing.
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

- Add `PackageBackend`, repository policy and compatibility target fields.
- Compile them into the generated TOML without breaking old configs.
- Implement the chosen resolver adapter and solver-neutral explanations.
- Add the unified coordinator and explicit Portage fallback reasons.
- Preserve the current `use_resolver` behavior for fallback requests.

Exit criteria:

- Old manifests choose the same Portage targets when no Oxys repository is
  configured.
- `Auto`, `OxysOnly`, `PortageBinaryFirst` and `PortageSource` have fixture
  coverage.
- Custom USE requests select an exact artifact variant or predictably fall back.
- Conflicts identify the root request and dependency chain in CLI/TUI output.

### Phase 3: ownership and transactional alternate-root install

Deliverables:

- Add SQLite ownership/state database with schema migrations.
- Implement collision preflight and config-file policy.
- Implement durable transaction journals and crash recovery.
- Implement typed triggers required by the live desktop.
- Implement `install`, `remove`, `verify` and `repair` against `--root <dir>`.

Exit criteria:

- Injected failures at every mutation step recover to either the old or new
  complete generation.
- Conflicting files fail before mutation.
- Upgrade/removal preserves locally modified configuration.
- Repeated install is idempotent.
- A composed reference root matches expected package file hashes and ownership.

### Phase 4: package conversion/build pipeline

Deliverables:

- Build Gentoo-derived `.oxys` packages from a pinned clean root.
- Capture build provenance, USE/profile/repository data and VDB payloads.
- Add package reinstall/reference-root comparison tests.
- Generate and sign a development repository index.
- Publish the current ISO stage1 package set for one target first:
  `amd64`, `x86-64-v3`, glibc, OpenRC.

Exit criteria:

- Every artifact can reconstruct its owned files from an empty staging root.
- Repository publication is atomic and old lockfiles remain fetchable.
- Gentoo-derived artifacts pass the Phase 0 interoperability decision.
- Kernel/ZFS coupled artifacts cannot be selected against a mismatched build ID.

### Phase 5: ISO composer integration

Deliverables:

- Replace the hand-maintained stage1 package block with `oxys-iso/config.rs`.
- Commit or release-manage `oxys-iso.lock`.
- Add stage3 unpack + `.oxys` composition + validation command/script.
- Emit catalyst's expected stage1 tarball and invoke only stage2.
- Add fully offline mode and cache prefetch tooling.
- Record lockfile, stage3 digest and kernel build ID in ISO metadata.

Exit criteria:

- Warm-cache ISO build performs no `emerge` and no network access.
- Cold-cache ISO build downloads artifacts but performs no package compilation.
- The produced ISO boots in QEMU, launches the installer and completes ext4
  installation.
- The installed copy contains coherent Oxys ownership state and Portage VDB.
- A package can subsequently be installed through the existing Portage fallback.
- Output contents are equivalent to the current catalyst stage1 package set,
  modulo documented normalization.

### Phase 6: installed-system package operations

Deliverables:

- Insert an `InstallResolvedPackages` step before the existing
  `EmergePackages` fallback, or replace both with one unified package-plan step.
- Add online repository update, package upgrade and rollback behavior.
- Coordinate global locking with Portage and detect external Portage mutations.
- Reconcile ownership after users run `emerge` directly.
- Surface transaction progress through the existing structured installer event
  stream.

Exit criteria:

- Mixed `.oxys` plus Portage installs work across install, upgrade, remove and
  reboot.
- Direct external emerge either safely coexists or causes a clear reconciliation
  requirement rather than silent database drift.
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
- Solver determinism independent of index iteration order.
- Solver explanation snapshots for conflicts and missing variants.
- Lockfile forward/backward compatibility rules.

### Integration tests

- Install/upgrade/remove native packages in a temporary root.
- File collision, shared file and explicit replacement cases.
- Crash/failure injection before and after every journal checkpoint.
- Config file modified/unmodified upgrade cases.
- Portage-derived package VDB query, pretend, upgrade and unmerge behavior.
- Mixed plan where one root is `.oxys` and another falls back to emerge.
- Offline resolve from a lockfile and offline compose from a warm cache.
- Kernel/ZFS build-ID mismatch rejection.

### End-to-end tests

- Build the current live package closure once, publish to a local repository,
  compose a stage1 and run catalyst stage2.
- Boot ISO under QEMU for BIOS and UEFI paths used by the project.
- Install to ext4, boot the installed disk and run ownership/Portage checks.
- Run a subsequent Portage package install and an `.oxys` update.
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

### Portage database drift

Risk: Portage does not know what `.oxys` installed, or an external emerge
changes files behind Oxys's database.

Mitigation: preserve/test full VDB records for compatible artifacts, maintain a
separate ownership DB, lock against concurrent Portage operations, and provide
`oxys verify/reconcile`. Restrict direct install to ISO roots until conformance
tests pass.

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

## Open decisions to settle during Phase 0

1. Should installed-system Gentoo-derived packages use direct VDB restoration,
   local Portage binpkg handoff, or remain Portage-only initially?
2. Is Resolvo's richer SAT model needed by the final v1 metadata, or does the
   fixture corpus show PubGrub provides a simpler implementation with better
   explanations?
3. Which exact Gentoo version parser/comparator will be treated as canonical?
4. Which live packages require post-install behavior beyond the proposed typed
   triggers?
5. Should the repository use CBOR or another canonical binary index encoding?
   This does not affect the human-readable metadata inside `.oxys`.
6. Is a generated desktop bundle worth the additional publishing complexity,
   or are individual artifact extraction times already small compared with
   squashfs creation?
7. What is the supported release matrix beyond the first
   `amd64/x86-64-v3/glibc/OpenRC` target?
8. What is the repository key ownership, offline backup and rotation process?

## Suggested first vertical slice

The smallest slice that proves the idea without risking installed systems is:

1. Implement local unsigned development repositories and the `.oxys` reader,
   writer, file table and safe alternate-root extraction.
2. Package three independent live tools and one small dependency chain from a
   clean reference root.
3. Resolve them with the solver-neutral interface and write a lockfile.
4. Compose them onto a stage3 in `/tmp` with ownership state and captured VDB.
5. Validate Portage queries in that root.
6. Generate the stage1 tarball catalyst expects and complete stage2.
7. Boot the ISO in QEMU.

After that works, expand the repository to the desktop closure and measure the
real build-time reduction. Only then enable `.oxys` writes to a normal installed
root. This ordering proves the core value—removing repeated ISO compilation—while
keeping the existing emerge path intact throughout development.
