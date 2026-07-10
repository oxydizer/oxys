# Oxys Overview

`oxys` is a small Rust library that turns a declarative system manifest into a Portage execution plan.

At a high level it does four things:

1. Accepts a `SystemManifest` config that describes the user-facing system DSL and converts it into an internal planning manifest.
2. Reads Gentoo `metadata/md5-cache` entries for those packages and parses the parts needed for USE resolution.
3. Produces a deterministic `PortagePlan` with:
   - exact `emerge` targets
   - per-package USE flags
   - global USE flags
   - accepted keywords
   - warnings, conflicts, and decision records
4. Optionally writes generated Portage config files and runs `emerge`.

## Main pieces

### 1. Manifest input

The user-facing entry point is [`SystemManifest`](oxys/src/manifest.rs). It contains nested system sections such as:

- `os`
- `disk`
- `hardware`
- `kernel`
- `packages`
- `services`
- `users`

Short example:

```rust
use oxys::{Libc, Os, Package, Shell, SystemManifest};

let manifest = SystemManifest {
    os: Os {
        hostname: "oxys".into(),
        timezone: "UTC".into(),
        locale: "en_US.UTF-8".into(),
        shell: Shell::Bash,
        libc: Libc::Glibc,
    },
    packages: vec![Package::new("gui-wm/niri").use_flags(["screencast", "-debug"])],
    ..SystemManifest::default()
};
```

### 2. Metadata loading and cache

The resolver loads md5-cache files from the Portage tree, parses fields like:

- `IUSE`
- `DEPEND`
- `RDEPEND`
- `KEYWORDS`

Parsed metadata is stored as JSON in a local cache for 7 days.

Conceptually:

```rust
use oxys::use_resolver::load_or_parse_metadata;

let metadata = load_or_parse_metadata(
    std::path::Path::new("/var/db/repos/gentoo/metadata/md5-cache/gui-wm/niri-25.11-r1"),
    std::path::Path::new("/var/cache/oxys/use-resolver"),
)?;
```

This avoids reparsing the same md5-cache files on every run.

### 3. Planning

The main planner API is [`plan_portage`](oxys/src/use_resolver/resolver.rs).

It combines:

- internal manifest policy derived from the user config
- package-level USE overrides from the manifest packages
- inferred preferences from selected packages when policy is not explicit
- md5-cache metadata such as available USE flags and keywords

It then returns a [`PortagePlan`](oxys/src/use_resolver/types.rs).

Short example:

```rust
use oxys::use_resolver::plan_portage;

let plan = plan_portage(
    &manifest,
    std::path::Path::new("/var/db/repos/gentoo"),
    std::path::Path::new("/var/cache/oxys/use-resolver"),
)?;
```

The plan contains:

- `targets`: exact versioned atoms like `=gui-wm/niri-25.11-r1`
- `resolution.package_use`: per-package USE output for `package.use`
- `resolution.global_use`: global USE entries for `make.conf`
- `resolution.accept_keywords`: entries for `package.accept_keywords`
- `resolution.conflicts`: things the resolver could not safely decide
- `resolution.warnings`: non-fatal issues
- `resolution.decisions`: deterministic explanation records

### 4. Writing config and applying

Once a plan exists, `oxys` can write generated Portage config files:

```rust
use oxys::use_resolver::write_portage_plan_config;

write_portage_plan_config(&plan, std::path::Path::new("/etc/portage"))?;
```

That writes:

- `package.use`
- `package.accept_keywords`
- `make.conf`

Or you can write config and launch `emerge` in one step:

```rust
use oxys::use_resolver::apply_portage_plan;

let stream = apply_portage_plan(
    &plan,
    std::path::Path::new("/etc/portage"),
    std::path::Path::new("/"),
    std::path::Path::new("/var/tmp/portage"),
    8,
)?;
```

The returned stream emits structured events like build start, fetch start, progress, completion, and errors.

## Typical flow

In practice the flow looks like this:

```text
SystemManifest
  -> md5-cache lookup
  -> parsed/cached PackageMetadata
  -> resolver rules
  -> PortagePlan
  -> generated /etc/portage files
  -> emerge
```

## What the resolver is deciding

The resolver mostly answers: "given these packages and policies, which USE flags and keywords should Portage see?"

Examples:

- If the manifest says `init_system = systemd`, packages exposing `systemd`/`openrc` flags that the user did *not* pin explicitly are driven toward `systemd`.
- Manifest policy never silently overrides an explicit per-package USE flag: if a package pins `+openrc` while `init_system = systemd`, the explicit flag is kept (matching `package.use` precedence) and the disagreement is recorded as a hard conflict naming the package, flag, and manifest field.
- `init_system` is a required field (defaulting to `openrc`); `display_stack`/`audio_stack` remain optional and are inferred from selected packages when unset.
- If a package is only keyworded `~amd64`, the planner adds the appropriate `package.accept_keywords` entry.
- If mutually exclusive selections appear together, the planner records a conflict instead of silently guessing.
- When two checks describe the same root cause, the more specific conflict wins: an explicit-vs-policy conflict (which names the package, flag, and manifest field) suppresses the generic "both … remain enabled" pair conflict for that package, so one mistake is reported once, not twice.

## Example output

A generated `package.use` file can look like:

```text
# generated by oxys - do not edit manually
gui-apps/waybar wayland
gui-wm/niri -X wayland
```

A generated `package.accept_keywords` file can look like:

```text
# generated by oxys - do not edit manually
~amd64 gui-apps/waybar
~amd64 gui-wm/niri
```

## Public API summary

The main exported APIs live under [`use_resolver`](oxys/src/use_resolver/mod.rs):

- `plan_portage`: build an apply-ready plan from a full manifest
- `resolve`: compatibility entry point when only package selections are available
- `write_portage_config` / `write_portage_plan_config`: render Portage config files
- `apply_portage_plan`: write config and run `emerge`
- `run_emerge`: stream structured `emerge` output directly
- `load_or_parse_metadata` / `sync`: manage the metadata cache

## Mental model

The simplest way to think about `oxys` is:

- `manifest.rs` defines what the system wants
- `parse.rs` and `cache.rs` understand Portage metadata
- `resolver.rs` turns that into decisions
- `generate.rs` renders those decisions into Portage files
- `emerge.rs` executes the plan and reports progress
