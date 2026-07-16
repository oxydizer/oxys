# Oxys vs AerynOS (moss/boulder) — Comparison Notes

Notes from reading **oxys** (this repo) alongside the vendored AerynOS
**os-tools** tree (`os-tools-main/`: moss + boulder + stone crates). No code
changes — architecture comparison only.

**Sources:** local trees as of 2026-07-11; AerynOS public docs/README for
product framing. AerynOS was formerly Serpent OS (Ikey Doherty lineage).

---

## One-line summary

| | **Oxys** | **AerynOS (moss)** |
|---|---|---|
| **What it is** | Declarative *policy layer* on top of **Gentoo Portage** | Full **from-scratch** package + OS state manager |
| **Execution engine** | `emerge` / Portage | moss itself (transactions, blit, store) |
| **Package format** | Gentoo ebuilds + binpkgs (tbz2/xpak/etc.) | Custom binary **`.stone`** |
| **Build tool** | Portage + oxys-build (kernel/ZFS pipeline) | **boulder** (containerized, YAML recipes) |
| **Config model** | Rust DSL → checksummed `manifest.toml` | Emerging **system model** (KDL); still unfinished |
| **Atomic upgrades** | No (Portage mutates live tree) | Yes (`renameat2` swap of `/usr`) |
| **License** | MIT OR Apache-2.0 | MPL-2.0 |

---

## Scale (rough Rust LOC, source only)

| Component | ~LOC |
|---|---:|
| **oxys** crate (`src` + `tests`) | ~16.5k |
| oxys-installer | ~3.9k |
| oxys-login | ~2.1k |
| **moss** | ~14.4k |
| **boulder** | ~9.0k |
| os-tools shared crates + libstone | ~9.3k |
| **AerynOS tooling total** | ~33k |

Oxys the *library/CLI* is similar in size to moss alone; the full AerynOS
stack (manager + builder + format) is roughly 2× that. Oxys also owns
installer/ISO/build-pipeline concerns in the monorepo that moss does not
have to, because AerynOS splits OS image work elsewhere.

---

## Philosophical split

### Oxys: “make Gentoo declarative and honest”

Oxys does **not** replace Portage. It:

1. Accepts a typed `SystemManifest` (Rust DSL or compiled TOML).
2. Reads Gentoo **md5-cache** metadata offline.
3. Resolves USE flags, keywords, licenses, REQUIRED_USE, slots, blockers.
4. Writes generated Portage config (`package.use`, `package.accept_keywords`,
   `make.conf`, …).
5. Runs **`emerge`** and streams structured progress.
6. Diffs desired vs current manifest for apply/update flows.

The hard package-graph problem (version ranges, slots, blockers, binary
packages, build deps, circular deps, etc.) stays with Portage. Oxys owns
**policy**, **explainability**, and **system-level intent** (init, display,
audio, GPU, compiler march, disk, users, services).

Mental model:

```text
SystemManifest  →  md5-cache  →  PortagePlan  →  /etc/portage/*  →  emerge
```

### moss: “the package manager *is* the OS transaction engine”

moss does not wrap another package manager. It:

1. Fetches and indexes **`.stone`** packages.
2. Resolves deps via tagged **providers** (name, soname, pkgconfig, …).
3. Caches content into a content-addressable store under **`/.moss`**.
4. Builds an in-memory VFS from layout records.
5. **Blits** a full `/usr` tree into staging via `linkat`/`mkdirat`.
6. Runs triggers in a **namespace/container** over the staged tree.
7. Atomically **exchanges** staging `/usr` with live `/usr` via `renameat2`.
8. Records a **State** (package selections) and can roll back / activate
   prior states (including from initramfs via `moss.fstx`).

Mental model (from their DESIGN-NOTES, paraphrased):

```text
.stone → shard content into CAS → State(selections)
  → in-memory VFS → blit staging → triggers → renameat2(/usr)
```

Ikey’s framing in DESIGN-NOTES: packages are *not* tarballs with mixed
metadata; layout, meta, index, and content are separate strongly-typed
payloads. Installation is composition of layouts against a store, not
“extract archive into /”.

---

## Package format

### Oxys / Gentoo

- Source: ebuilds in trees (`gentoo` + overlays).
- Binary: Portage binpkgs + oxys-build’s own **tarball artifacts** for
  kernel/ZFS (build-id + vermagic tagged).
- Metadata: md5-cache fields (`IUSE`, `DEPEND`, `RDEPEND`, `KEYWORDS`, …).
- No custom on-disk package container in oxys itself.

### AerynOS `.stone`

Structured binary archive with versioned header and zstd payloads:

| Payload | Role |
|---|---|
| **Meta** | Strongly typed key/value package info + deps/providers |
| **Layout** | Paths, modes, types, optional content hashes |
| **Index** | Jump table into content blob (xxhash keys) |
| **Content** | Concatenated unique file bytes (single compressed blob) |
| **Attributes** | Extra attribute records |

Clever reuse: **repo indexes** and **build manifests** are also stones
(meta-only, different archive type flag). Uniform format for packages,
repos, and manifests.

Oxys has nothing equivalent — and does not need one while Portage remains
the backend. If oxys ever shipped a first-class “oxys package” format, stone
is the closest peer design in the Rust-from-scratch space.

---

## Dependency model

### Oxys / Portage world

Oxys **parses and reasons about** Portage’s model:

- Conditional deps (`flag? ( cat/pkg )`)
- Blockers (`!`, `!!`)
- Slots / subslots / slot operators (`:=`, `:*`)
- `REQUIRED_USE` (`||`, `^^`, `??`)
- Keywords, licenses, virtuals
- USE-driven mutual exclusions (wayland vs X, openrc vs systemd, …)

But the **solver that installs** is still Portage. Oxys’s job is to produce a
plan Portage will accept, with **decision records** explaining *why* each
flag was set (manifest policy vs explicit pin vs inference).

Notable oxys rule: **explicit per-package USE always wins** over manifest
policy; disagreement is a hard conflict with package + flag + field named,
not a silent override.

### moss

Dependencies are **simple tagged strings**, not version ranges:

```text
name(bash)
soname(libz.so.1(x86_64))
pkgconfig(zlib)
binary(git)
python(...)
cmake(...)
```

Providers are the inverse. Resolution walks a **plugin registry** (installed
set, local cobble, remote repos) ordered by priority.

Deliberate choice for a rolling binary OS: no rich constraint language yet.
Their own comment admits this and points at a future major stone format for
more expressive deps. Contrast with Portage, which is almost *all*
constraint language.

**Interesting implication:** moss can auto-emit soname/pkgconfig providers
from boulder analysis; oxys inherits Portage’s ebuild-authored DEPEND graph
and cannot invent providers the ebuild didn’t declare.

---

## Build systems

### boulder (AerynOS)

- Recipes: **YAML** (`stone.yaml`); KDL mentioned as future.
- Macros for autotools, cmake, meson, cargo, golang, python, qt-kde, PGO, …
- Architecture profiles (x86_64, v3, aarch64, riscv64, emul32).
- Containerized / rootless builds (subuid/subgid).
- Automatic **subpackage splitting**.
- Automatic **provider emission** from ELF/Python analysis.
- Emits `.stone` + binary manifests.

### Oxys / Gentoo

- Build unit = **ebuild** (bash + eclass ecosystem decades deep).
- oxys controls *compiler policy* via generated `make.conf`
  (`CFLAGS`/`march`, mold, ccache, emerge jobs, optimisation profile).
- **oxys-build**: Podman pipeline that builds kernel + zfs-kmod against one
  Portage snapshot, tags with shared `build_id`, verifies vermagic — so ISO
  and installed system never diverge on kernel modules.
- Binary preference: `prefer_binary` global + per-package `binary` /
  `from_source`; USE flags force source (with warnings if mixed with
  prefer_binary).

**Takeaway:** boulder owns the *package definition language*. Oxys deliberately
does not invent one; it steers Portage’s existing language. That is both
oxys’s biggest productivity win (entire Gentoo tree) and its biggest ceiling
(cannot fix Portage’s model).

---

## Transaction / filesystem model

This is where moss is most alien to Portage-shaped thinking.

### moss “stateless `/usr` + CAS”

- Content-addressable store under `/.moss` (hashed file bodies).
- Each transaction composes a **new** `/usr` tree in staging from hardlinks
  into the store (`linkat`).
- Atomic promote: `renameat2(..., RENAME_EXCHANGE)` swaps staging `/usr`
  with live `/usr`.
- Old tree becomes an archived **state**; boot can pin `moss.fstx=<id>` and
  activate from initramfs (`boot/moss-fstx.sh`).
- Full **USR merge**; OS vs local config separation; triggers in isolated
  roots.
- SQLite DBs (diesel): install meta, layout, state history.
- Benchmarks in-tree: ~230k layout entries blit in ~6s hot / ~12s cold on
  older SSD hardware — “install nano” rewrites the world view, not just
  nano’s files.

### Oxys / Portage

- Packages install into the live filesystem (or `--root` for target install).
- No content-addressable `/usr`, no atomic root swap, no first-class state
  history inside oxys.
- Rollback = whatever the user has (ZFS snapshots when layout is ZFS —
  modeled in manifest; not the same as moss states).
- World set bookkeeping: after apply, oxys runs select/deselect + depclean
  pretend to align Portage’s `@world` with the manifest.

**Interesting contrast:** moss pays a constant “rebuild the OS tree” cost so
every transaction is atomic and rollbackable. Portage/oxys pays per-package
mutation cost and inherits Portage’s partial-failure modes. Different failure
domains entirely.

---

## Declarative system config

| Concern | Oxys `SystemManifest` | moss `SystemModel` |
|---|---|---|
| Status | Core product feature | Checked off as **incomplete** in README |
| Format | Rust DSL → TOML + SHA-256 integrity | KDL (repos + package providers) |
| Packages | Full atoms + USE/keywords/binary policy | Set of providers (names) |
| Disk / install | Full disk layouts (ZFS, ext4, …) | Out of moss scope |
| Hardware | GPU (incl. hybrid PRIME), power | No |
| Compiler policy | CFLAGS, march tiers, ccache, jobs | No (boulder profiles handle builds) |
| Services / users | Yes (enable/disable; useradd/chpasswd) | No (OS-level elsewhere) |
| Init / display / audio | First-class enums driving USE | No |

Oxys is a **whole-system DSL** that happens to drive a package manager.
moss is a **package/OS-state manager** that is growing a system model for
desired packages/repos — closer to “declarative package selection” than
“declarative machine”.

If both finished their arcs:

- Oxys: NixOS-shaped *intent* with Gentoo *implementation*.
- moss: OSTree/Nix-store-shaped *implementation* with a thinner intent layer.

---

## CLI surface

### oxys

```text
compile   # Rust config → manifest.toml
check     # plan without touching disk
diff      # local vs /etc/oxys/current-manifest.toml
apply     # converge running system to local manifest
update    # sync + pretend + preflight + guarded world update
install   # add packages, or `install system` for OS install
```

Workflow is **manifest-centric**: edit config → compile → check/diff → apply.

### moss

```text
install / it    remove / rm    sync    fetch
list            search         search-file
info            inspect        extract
repo            index          cache
state           boot           version
```

Workflow is **transaction-centric**: install/remove/sync produce new states;
`state` manages history. `-D root` for alternate roots (including boulder
chroots).

Ephemeral clients blit into a foreign root without recording host state —
how boulder installs build deps without polluting the host OS.

---

## Monorepo scope

| Area | Oxys monorepo | os-tools |
|---|---|---|
| Package manager CLI | `oxys` | `moss` |
| Package builder | Portage + `oxys-build` | `boulder` |
| Package format lib | — | `crates/stone`, `libstone` (C FFI) |
| Installer TUI | `oxys-installer` | elsewhere (img-tests, etc.) |
| Live ISO | `oxys-iso` (catalyst) | elsewhere |
| Login manager | `oxys-login` | — |
| Kernel coherence | shared build-id pipeline | not in this tree |

Oxys is building a **distro product** in one repo. os-tools is the **OS
tooling core** for AerynOS; product packaging lives around it.

---

## Shared Rust ecosystem notes

Both:

- Rust **edition 2024**, clap 4, serde, thiserror, tokio where needed.
- Care about structured logging / progress (oxys: emerge event stream;
  moss: tracing + indicatif + tui crate).
- Think in “plans then apply” rather than fire-and-forget shell.

Divergences:

| | Oxys | moss/boulder |
|---|---|---|
| Error style | thiserror | thiserror + snafu in places |
| DB | none (JSON metadata cache) | SQLite + diesel migrations |
| Graphs | policy rules over packages | petgraph + custom dag crate; vfs tree |
| Containers | Podman for builds/ISO | Linux namespaces in-process for triggers/builds |
| Hashing | SHA-256 for manifest integrity | xxhash (store keys), SHA-2 elsewhere |
| Config languages | Rust + TOML | YAML recipes, KDL system model |

---

## What oxys can learn from moss (ideas, not mandates)

1. **State IDs / transaction history**  
   Even without CAS, recording “apply #N produced these Portage config
   hashes + emerge atoms” would make `oxys update`/`diff` more forensic.

2. **Strongly typed package metadata containers**  
   If oxys ever ships first-party binpkgs beyond Portage’s format, stone’s
   meta/layout/content split is a better blueprint than “tar + sidecars”.

3. **Provider-style auto-deps for custom packages**  
   Boulder’s ELF soname emission is something Gentoo only gets when
   ebuilds call the right eclasses. For oxys-overlay packages, automated
   analysis could reduce footguns.

4. **Ephemeral target roots**  
   moss’s ephemeral client (shared cache, no host state) maps cleanly to
   “install into `/mnt/oxys` with host binpkg cache” — something the oxys
   installer path still needs for real target Portage applies.

5. **Triggers in isolation**  
   Running post-install hooks against a staged tree (not live `/`) is how
   moss avoids half-upgraded systems. Harder under Portage, but target
   installs with `ROOT=` are the same problem space.

6. **Boot-time rollback hook**  
   `moss.fstx` + dracut module is a complete story. Oxys’s ZFS snapshots
   could grow a similar “boot previous known-good” path once snapshot
   apply is wired.

## What moss can learn from oxys (interesting inverses)

1. **Decision records**  
   Oxys’s `PortageDecision` trail (scope, package, action, source, reason)
   is excellent UX for “why is this flag on?”. moss resolution is more
   opaque from the outside.

2. **Whole-machine DSL**  
   Disk, users, services, GPU, compiler, init — one typed document. moss’s
   system model is intentionally thinner; AerynOS product will need
   something oxys-shaped above moss eventually.

3. **Guarded world updates**  
   `oxys update` with pretend-parse + preflight + force/dry-run is a
   careful rolling-release UX. moss `sync` is transactional/atomic, so the
   risk profile differs, but “explain before swap” still matters.

4. **Checksum-locked compiled config**  
   Oxys refuses silently edited `manifest.toml` via SHA-256 field. Useful
   for any declarative model that might be hand-edited.

5. **Explicit vs policy conflict reporting**  
   Prefer one precise conflict over two generic ones — good pattern for
   any policy engine layered on automatic resolution.

---

## Side-by-side: install “a desktop package”

### Oxys

```text
edit Rust config  →  oxys compile  →  oxys check
  → plan_portage (md5-cache, USE rules)
  → write /etc/portage/*
  → emerge =cat/pkg-ver
  → reconcile @world
  → persist /etc/oxys/current-manifest.toml
```

Portage may compile from source or fetch a binpkg. Tree mutates in place.
Failure mid-emerge is a classic Portage recovery situation.

### moss

```text
moss install pkg
  → resolve providers across plugins
  → fetch/cache .stones into CAS
  → new State(selections)
  → blit full /usr to staging
  → container triggers
  → renameat2 exchange /usr
  → archive previous state
```

Almost always binary. Failure before promote leaves live system untouched.
Promote is atomic at `/usr` granularity.

---

## Architecture sketches

### Oxys

```text
┌─────────────────────────────────────────────┐
│  SystemManifest (Rust DSL / TOML)           │
│  packages, USE, disk, users, GPU, init…     │
└──────────────────┬──────────────────────────┘
                   │ plan_portage
                   ▼
┌─────────────────────────────────────────────┐
│  use_resolver                               │
│  md5-cache → rules → PortagePlan            │
│  (decisions, conflicts, warnings)           │
└──────────────────┬──────────────────────────┘
                   │ write_portage_* / apply
                   ▼
┌─────────────────────────────────────────────┐
│  Portage (emerge)  +  binhost / source      │
│  ebuilds · slots · blockers · world set     │
└─────────────────────────────────────────────┘
```

### moss

```text
┌─────────────────────────────────────────────┐
│  CLI / SystemModel (KDL, WIP)               │
└──────────────────┬──────────────────────────┘
                   │
                   ▼
┌─────────────────────────────────────────────┐
│  Registry plugins → Transaction graph       │
│  stone fetch → CAS (/.moss)                 │
│  layout DB + meta DB + state DB             │
└──────────────────┬──────────────────────────┘
                   │ blit VFS → staging /usr
                   ▼
┌─────────────────────────────────────────────┐
│  Triggers (namespaces) → renameat2          │
│  State archive · boot (blsforme) · fstx     │
└─────────────────────────────────────────────┘
```

---

## Honest “who wins where”

| Domain | Edge |
|---|---|
| Package ecosystem breadth | **Oxys** (Gentoo tree + overlays) |
| Atomic upgrades / rollback | **moss** |
| Install performance (binary) | **moss** (CAS + hardlink blit) |
| Source flexibility / USE knobs | **Oxys** / Portage |
| Explainable policy | **Oxys** (decision log) |
| From-scratch purity / no Python Portage | **moss** |
| Whole-system install story in-tree | **Oxys** monorepo (still WIP) |
| Format design elegance | **stone** |
| Rolling binary OS as product | **AerynOS design center** |
| Source-based enthusiast OS as product | **Oxys design center** |

They are not competitors in the same niche so much as **two answers to
“Rust + modern Linux packaging”**:

- **Oxys** = tame and document Portage; ship a Gentoo-derived OS with a
  typed control plane.
- **moss** = throw away the tarball-PM tradition; treat the OS as
  versioned compositions of content-addressed files.

---

## Fun details worth stealing (mentally)

1. **Repo index is a stone** — one parser for packages, repos, and build
   manifests. Uniformity beats special cases.

2. **Blit rewrites the world every time** — counterintuitive until you
   realize hardlinking from CAS makes “full tree” cheap and conflict
   detection can run on the *complete* VFS before promote.

3. **`renameat2` RENAME_EXCHANGE** — the atomicity primitive. No half-swapped
   `/usr`. musl-friendly raw syscall in moss because libc may not expose it.

4. **Oxys compile-time password warnings** — plaintext passwords in
   manifests get compile warnings; `Prompt`/`Hashed` preferred. Policy
   UX at the type level.

5. **Kernel/ZFS build-id coupling** — oxys’s answer to “module ABI hell”
   is process/architecture, not the package manager format. moss would
   still need an equivalent product discipline for out-of-tree modules.

6. **Trigger scopes** — transaction-scope vs system-scope triggers, run in
   isolation before/after promote. Maps to “what must succeed before the
   world sees the new tree.”

7. **Oxys update pretends first** — parses `emerge --pretend` into a
   structured plan; refuses to dive into a world update blind. Moss’s
   atomicity reduces the need, but the *communication* pattern is gold.

---

## Open questions / things incomplete on both sides

| Oxys | moss / AerynOS |
|---|---|
| Installer TUI does not yet fully install “as per config” (disk + partial copy path) | System model + subscriptions still TODO in README |
| Many manifest fields parsed but not applied (hostname, locale, journal, …) | Dependency constraints still name/provider only |
| Target-root Portage apply for real installs still being designed | KDL recipes “coming soon” |
| ZFS-root boot not fully guaranteed | — |
| No atomic `/usr` story | No Gentoo-scale source/USE model (by design) |

---

## Bottom line

**Oxys** and **moss** share Rust, clap-shaped CLIs, and a desire to make
Linux system management less folkloric. They diverge at the foundation:

- Oxys is a **smart control plane** over a mature, source-centric PM.
- moss is a **content-addressed OS compositor** with a custom binary PM.

The most productive cross-pollination is not “rewrite oxys as moss” or
vice versa — it is:

1. Borrow moss’s **transaction/state vocabulary** and isolation habits for
   oxys target installs and updates.
2. Borrow oxys’s **typed whole-system manifest + decision audit trail** for
   anything declarative you put above either stack.
3. Keep stone/Portage as what they are: **excellent at different jobs**.

---

*Generated from local trees: `oxys/`, `oxys-*`, `os-tools-main/`.*
