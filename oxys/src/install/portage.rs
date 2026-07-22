use std::{fs, path::Path, sync::mpsc::Sender};

use crate::{
    exec,
    manifest::SystemManifest,
    use_resolver::{
        EmergeLine, emerge_select, plan_portage, run_emerge_chroot, write_portage_plan_config,
    },
};

use super::{SystemInstallError, SystemInstallEvent};

pub(super) fn emerge_manifest_packages(
    manifest: &SystemManifest,
    target_mount: &Path,
    sender: &Sender<SystemInstallEvent>,
) -> Result<(), SystemInstallError> {
    // Package failures currently fail the install so we never report success
    // for a manifest that was only partially applied. A future partial-success
    // design should still run boot-critical finalization and unmount cleanly,
    // report an explicit "completed with package errors" outcome, avoid
    // recording the manifest as fully applied, and preserve a retry path. Do
    // not restore the old warning-and-Ok behavior: it silently skipped packages
    // while allowing the installer to announce full success.
    let portage_tree = target_mount.join("var/db/repos");
    let gentoo_tree = portage_tree.join("gentoo");
    if !gentoo_tree.join("metadata/md5-cache").is_dir() {
        return Err(SystemInstallError::PackageInstall(format!(
            "target Portage tree is missing or incomplete: {}",
            gentoo_tree.display()
        )));
    }

    let cache_dir = target_mount.join("var/cache/oxys/use-resolver");
    let plan = plan_portage(manifest, &portage_tree, &cache_dir)?;
    if !plan.resolution.conflicts.is_empty() {
        let conflicts = plan
            .resolution
            .conflicts
            .iter()
            .map(|conflict| {
                format!(
                    "{}: {} ({})",
                    conflict.flag,
                    conflict.reason,
                    conflict.packages.join(", ")
                )
            })
            .collect::<Vec<_>>()
            .join("; ");
        return Err(SystemInstallError::PackageInstall(format!(
            "package plan has unresolved conflicts: {conflicts}"
        )));
    }

    write_portage_plan_config(&plan, &target_mount.join("etc/portage"))?;
    let _ = sender.send(SystemInstallEvent::StepOutput {
        line: format!("planned package target(s): {}", plan.targets.join(", ")),
    });

    verify_target_build_tools(manifest, target_mount, sender)?;

    ensure_target_resolv_conf(target_mount, sender);

    if !chroot_has_connectivity(target_mount, sender) {
        return Err(SystemInstallError::PackageInstall(
            "target network preflight failed; manifest packages were not installed".to_owned(),
        ));
    }

    let mut stream = run_emerge_chroot(
        &plan.targets,
        target_mount,
        Path::new("/var/tmp"),
        plan.manifest.compiler.emerge_jobs,
        plan.use_binpkgs,
    )?;

    for line in &mut stream {
        send_emerge_line(line, sender);
    }

    stream.wait()?;

    // emerge ran with --update --changed-use, so already-satisfied packages
    // were skipped without a world entry. Register every manifest package
    // (unversioned, so @world doesn't pin versions) — --noreplace never
    // rebuilds, it only records the selection. Without this, a future
    // depclean would treat skipped manifest packages as removable.
    let world_atoms = manifest
        .packages
        .iter()
        .map(|package| package.package.clone())
        .collect::<Vec<_>>();
    emerge_select(&world_atoms, target_mount).map_err(|error| {
        SystemInstallError::PackageInstall(format!(
            "packages installed, but recording them in the target @world failed: {error}"
        ))
    })?;
    let _ = sender.send(SystemInstallEvent::StepOutput {
        line: format!("recorded {} package(s) in target @world", world_atoms.len()),
    });

    Ok(())
}

/// Linker name requested via `-fuse-ld=<name>` in LDFLAGS, if any (last wins,
/// matching gcc's handling of repeated flags).
fn requested_linker(ldflags: &str) -> Option<&str> {
    ldflags
        .split_whitespace()
        .filter_map(|flag| flag.strip_prefix("-fuse-ld="))
        .filter(|name| !name.is_empty())
        .next_back()
}

/// The generated make.conf can demand tools that emerge never pulls in as
/// dependencies: an alternative linker via LDFLAGS and ccache via FEATURES.
/// The target root is rsync'd from the live image, so if the live image
/// dropped the linker, every source build dies in configure with "C compiler
/// cannot create executables" (collect2: cannot find 'ld'). Catch that before
/// emerge starts instead of failing mid-install on the first source build.
/// A missing ccache only costs portage a warning, so it degrades to one here.
fn verify_target_build_tools(
    manifest: &SystemManifest,
    target_mount: &Path,
    sender: &Sender<SystemInstallEvent>,
) -> Result<(), SystemInstallError> {
    if let Some(linker) = requested_linker(&manifest.compiler.ldflags) {
        // gcc resolves -fuse-ld=<name> to `ld.<name>`; mold also installs a
        // bare `mold` binary that some setups symlink instead.
        let candidates = [
            target_mount.join(format!("usr/bin/ld.{linker}")),
            target_mount.join(format!("usr/bin/{linker}")),
        ];
        if !candidates.iter().any(|path| path.exists()) {
            return Err(SystemInstallError::PackageInstall(format!(
                "target has no '{linker}' linker, but compiler.ldflags is {:?}; \
                 every source build would fail to link. Ship the linker on the \
                 live image (sys-devel/mold in installcd-stage1.spec) or change \
                 compiler.ldflags in the manifest.",
                manifest.compiler.ldflags
            )));
        }
    }

    if manifest.compiler.ccache && !target_mount.join("usr/bin/ccache").exists() {
        let _ = sender.send(SystemInstallEvent::StepOutput {
            line: "Warning: compiler.ccache is enabled but ccache is not installed \
                   on the target; source builds proceed without a compiler cache"
                .to_owned(),
        });
    }

    Ok(())
}

/// Ensure the target has a usable `/etc/resolv.conf` before we emerge into it.
///
/// The target's resolv.conf is rsync'd from the live system, but on many setups
/// `/etc/resolv.conf` is a symlink into `/run` (NetworkManager, systemd-resolved)
/// -- and `/run` is excluded from the rsync, so the target inherits a *dangling*
/// symlink and the chroot has no DNS. That makes both the connectivity preflight
/// and emerge's fetches fail. Reading the host file follows the symlink to its
/// real content; we write that through as a plain file (replacing any dangling
/// link) so name resolution works inside the chroot.
/// Best-effort: on failure we log and let the connectivity preflight decide.
fn ensure_target_resolv_conf(target_mount: &Path, sender: &Sender<SystemInstallEvent>) {
    let target_resolv = target_mount.join("etc/resolv.conf");
    match fs::read("/etc/resolv.conf") {
        Ok(contents) => {
            // Drop any inherited (possibly dangling) symlink before writing.
            let _ = fs::remove_file(&target_resolv);
            if let Some(parent) = target_resolv.parent() {
                let _ = fs::create_dir_all(parent);
            }
            if let Err(error) = fs::write(&target_resolv, contents) {
                let _ = sender.send(SystemInstallEvent::StepOutput {
                    line: format!("Warning: could not write target resolv.conf: {error}"),
                });
            }
        }
        Err(error) => {
            let _ = sender.send(SystemInstallEvent::StepOutput {
                line: format!("Warning: no readable host resolv.conf for target DNS: {error}"),
            });
        }
    }
}

fn chroot_has_connectivity(target_mount: &Path, sender: &Sender<SystemInstallEvent>) -> bool {
    let target = target_mount.display().to_string();
    match exec::capture_command(
        "chroot",
        [&target, "getent", "hosts", "distfiles.gentoo.org"],
    ) {
        Ok(output) if output.status.success() => {
            for line in String::from_utf8_lossy(&output.stdout).lines() {
                let _ = sender.send(SystemInstallEvent::StepOutput {
                    line: format!("network preflight: {line}"),
                });
            }
            true
        }
        Ok(output) => {
            for line in String::from_utf8_lossy(&output.stderr).lines() {
                let _ = sender.send(SystemInstallEvent::StepOutput {
                    line: format!("network preflight: {line}"),
                });
            }
            false
        }
        Err(error) => {
            let _ = sender.send(SystemInstallEvent::StepOutput {
                line: format!("network preflight failed: {error}"),
            });
            false
        }
    }
}

fn send_emerge_line(line: EmergeLine, sender: &Sender<SystemInstallEvent>) {
    let rendered = match line {
        EmergeLine::BuildStart { package } => format!("emerging {package}"),
        EmergeLine::BuildProgress { package, line } => package
            .map(|package| format!("{package}: {line}"))
            .unwrap_or(line),
        EmergeLine::BuildComplete {
            package,
            completed,
            total,
        } => total.map_or_else(
            || format!("completed {package}"),
            |total| format!("completed {package} ({completed}/{total})"),
        ),
        EmergeLine::FetchStart { package } => format!("fetching {package}"),
        EmergeLine::FetchComplete { package } => format!("fetched {package}"),
        EmergeLine::Error { package, message } => package
            .map(|package| format!("{package}: {message}"))
            .unwrap_or(message),
    };

    let _ = sender.send(SystemInstallEvent::StepOutput { line: rendered });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manifest::Package;
    use std::sync::mpsc;

    #[test]
    fn missing_portage_tree_fails_package_install() {
        let target = tempfile::tempdir().unwrap();
        let manifest = SystemManifest {
            packages: vec![Package::new("app-misc/example")],
            ..SystemManifest::default()
        };
        let (sender, _receiver) = mpsc::channel();

        let error = emerge_manifest_packages(&manifest, target.path(), &sender)
            .expect_err("missing package metadata must fail the install");

        assert!(matches!(error, SystemInstallError::PackageInstall(_)));
        assert!(
            error
                .to_string()
                .contains("Portage tree is missing or incomplete")
        );
    }

    #[test]
    fn requested_linker_parses_fuse_ld_flags() {
        assert_eq!(requested_linker("-fuse-ld=mold"), Some("mold"));
        assert_eq!(requested_linker("-Wl,-O1 -fuse-ld=lld"), Some("lld"));
        assert_eq!(
            requested_linker("-fuse-ld=bfd -fuse-ld=mold"),
            Some("mold"),
            "the last -fuse-ld wins, matching gcc"
        );
        assert_eq!(requested_linker(""), None);
        assert_eq!(requested_linker("-Wl,--as-needed"), None);
        assert_eq!(requested_linker("-fuse-ld="), None);
    }

    #[test]
    fn missing_requested_linker_fails_before_emerge() {
        let target = tempfile::tempdir().unwrap();
        let manifest = SystemManifest::default();
        assert_eq!(manifest.compiler.ldflags, "-fuse-ld=mold");
        let (sender, _receiver) = mpsc::channel();

        let error = verify_target_build_tools(&manifest, target.path(), &sender)
            .expect_err("a target without mold must fail the preflight");

        assert!(matches!(error, SystemInstallError::PackageInstall(_)));
        assert!(error.to_string().contains("no 'mold' linker"));
    }

    #[test]
    fn present_linker_passes_and_missing_ccache_only_warns() {
        let target = tempfile::tempdir().unwrap();
        let bin = target.path().join("usr/bin");
        std::fs::create_dir_all(&bin).unwrap();
        std::fs::write(bin.join("ld.mold"), "").unwrap();
        let manifest = SystemManifest::default();
        assert!(manifest.compiler.ccache);
        let (sender, receiver) = mpsc::channel();

        verify_target_build_tools(&manifest, target.path(), &sender)
            .expect("linker present: preflight must pass");

        match receiver.recv().unwrap() {
            SystemInstallEvent::StepOutput { line } => {
                assert!(line.contains("ccache is not installed"));
            }
            event => panic!("expected a ccache warning StepOutput, got {event:?}"),
        }
    }

    #[test]
    fn completed_package_event_exposes_count_for_installer_progress() {
        let (sender, receiver) = mpsc::channel();
        send_emerge_line(
            EmergeLine::BuildComplete {
                package: "sys-apps/iucode_tool".to_owned(),
                completed: 12,
                total: Some(133),
            },
            &sender,
        );

        assert_eq!(
            receiver.recv().unwrap(),
            SystemInstallEvent::StepOutput {
                line: "completed sys-apps/iucode_tool (12/133)".to_owned(),
            }
        );
    }
}
