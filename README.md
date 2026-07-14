# OxysOS

A Rust-first, declarative Gentoo/Portage system: a manifest/resolver/CLI, a
reproducible kernel+package build pipeline, and a catalyst-based installable
ISO — kept as one repo so the kernel that ships on the ISO and the kernel the
post-install package pipeline builds against can never silently diverge.

## Layout

```text
oxys/           Rust manifest/resolver/CLI (SystemManifest, Portage resolver,
                 disk provisioning, emerge streaming). See [oxys/OVERVIEW.md](oxys/OVERVIEW.md)
                 and the system [CONFIG.md](CONFIG.md) for a deep dive.

oxys-build/      Podman/Gentoo build pipeline: compiles the kernel and
                 sys-fs/zfs-kmod together against one Portage snapshot, then
                 archives each as a build-id-tagged, vermagic-verified tarball
                 under output/<arch>/ — the single source of truth for "which
                 kernel and which zfs-kmod belong together."

oxys-iso/        catalyst-based installable-ISO builder. Consumes
                 oxys-build's tagged kernel+zfs-kmod tarballs directly
                 (catalyst's own kernel-build step is disabled for this) so
                 the ISO's kernel is byte-for-byte the same one oxys-build
                 produced, not a separately re-derived one. See
                 oxys-iso/README.md.

oxys-login/        PAM-backed TUI login manager.
oxys-installer/    Ratatui installer wizard.
```

## Why one repo

`oxys-build` tags each kernel/zfs-kmod pair with a shared `build_id` and
verifies their vermagic matches at build time. If `oxys-iso` built or fetched
its *own* kernel for the live ISO, that guarantee would only hold within
`oxys-build`'s own output — the ISO could still end up booting a different
kernel than the one the installed system's package pipeline targets. Keeping
both in one repo, with `oxys-iso` consuming `oxys-build`'s tagged output
directly, keeps there being exactly one kernel build to reason about.

See [oxys/OVERVIEW.md](oxys/OVERVIEW.md) for the `oxys` crate's architecture, [CONFIG.md](CONFIG.md) for the configuration reference, and each subproject's own README/docs for build/run instructions.

The proposed native `.oxys` binary package format, dependency resolver,
Portage compatibility model, and fast live-ISO composition roadmap are detailed
in [OXYS_PACKAGE_FORMAT.md](OXYS_PACKAGE_FORMAT.md).
