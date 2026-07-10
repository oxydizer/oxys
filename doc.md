I looked through the installer and the lower-level oxys install path. Short version: the TUI is currently a disk formatter with
  a UI around config selection. It does not yet install the OS “as per config”.

  Current Reality
  The TUI flow in oxys-installer/src/app.rs:210 calls:

  provisioning::run_install(disk, tx);

  That function in oxys-installer/src/provisioning.rs:40 only does:

  1. disk preflight
  2. plan_disk
  3. apply_disk_plan
  4. reports /mnt/oxys is ready

  It never reads the selected config, never generates a manifest, never copies the live system, never installs packages, and never
  writes boot/runtime config.

  What Already Exists
  The library already has partial system-copy support in oxys/src/install.rs:205:

  - copy live root to target with rsync
  - write fstab
  - reset machine-id
  - install GRUB or systemd-boot
  - copy kernel/initramfs to ESP
  - enable/disable systemd or OpenRC services

  The CLI wires this through oxys install --copy-system in oxys/src/main.rs:388, but the TUI does not use it.

  Major Missing Pieces

 
  2. No config compilation path in installer
     The selected Rust config must become a SystemManifest. Today that happens by running a Rust config binary via oxys compile,
     but the live ISO installer probably should not depend on Cargo/Rust being present. Better path: compile/embed profile
     manifests at ISO build time, or ship editable TOML instead of Rust for the installer.

  3. Disk UI choices are not merged into the config
     The selected disk/layout exists only as a Disk passed to disk provisioning. The final manifest must be updated with:
      - selected device
      - selected layout
      - EFI/swap/root partition settings
      - maybe bootloader choice

  4. No package installation into target
     “As per config” means packages, USE flags, keywords, licenses, binpkg/source policy, etc. Current system-copy explicitly does
     not install packages. apply_portage_plan exists, but target install needs a correct target-root mode:
      - write generated Portage config for the target
      - run emerge with --root /mnt/oxys
      - likely set/configure PORTAGE_CONFIGROOT or verify Portage uses target config as intended
      - consume Oxys binary packages/binhost

  5. Live ISO is not the desired target system
     The ISO is intentionally minimal/no GUI. Its make.conf disables GUI stacks in oxys-iso/portage_confdir/make.conf:20. Copying
     the live system alone will not produce your Niri/Noctalia target OS.

  6. Boot/initramfs is not target-grade yet
     The copy step grabs the newest kernel/initramfs under /boot. For a real installed OS, we need a target-specific initramfs
     strategy, especially for ZFS. The docs already call out ZFS root as not guaranteed bootable.

  7. Manifest fields are mostly not applied
     Packages and services are partially handled elsewhere, but these are not fully applied during install:
      - users/passwords/groups/shells
      - hostname/timezone/locale
      - environment
      - runtime GPU config
      - compiler/build policy
      - current manifest persistence under /etc/oxys/current-manifest.toml

  8. Installer success/error handling is too shallow
     The TUI moves to Done when the channel disconnects, even after an [error] line. Progress is line-count based. For real
     installs, it needs structured phases and fatal state.

  Suggested Implementation Map

 

  2. Build final manifest:
      - load selected profile manifest
      - merge detected/selected disk layout
      - merge hardware detection if desired
      - show manifest summary before destructive action

  3. Replace run_install(disk) with a real pipeline:
      - disk preflight/provision
      - system copy or stage3/base extraction
      - target Portage config generation
      - target package install from binpkgs/source
      - runtime config
      - bootloader/initramfs
      - persist /etc/oxys/current-manifest.toml

  4. Decide install model:
      - Copy live root then mutate target: fastest to implement, but live ISO must be close enough to target base.
      - Install target from packages/stage3: cleaner long-term, more work.
      - For Oxys, I’d choose copy live root for base bootstrap, then run target Portage plan to converge to manifest.

  5. Make ext4 the first “real supported” target:
      - keep ZFS selectable only with a warning until target initramfs/import flow is proven.

