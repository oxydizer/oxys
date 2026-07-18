use std::path::Path;

use colored::Colorize;
use oxys::{
    InitSystem, ResolvedSwap, SystemManifest, activate_openrc_services,
    diff::{PackageChange, diff_packages},
    runtime::sync_runtime_config,
    use_resolver::{apply_portage_plan, emerge_depclean_pretend, emerge_deselect, emerge_select},
};

use super::{
    manifest_io::{
        effective_portage_config_dir, effective_root, effective_system_manifest_path,
        load_manifest_optional,
    },
    output::{fail_on_conflicts, print_changes, print_plan},
};
use crate::{
    DEFAULT_PORTAGE_TMPDIR, LOCAL_MANIFEST, Result, create_plan, load_manifest,
    persist_manifest_value, print_emerge_event,
};

pub(crate) fn run() -> Result<()> {
    let desired_path = Path::new(LOCAL_MANIFEST);
    let desired = load_manifest(desired_path)?;
    apply_manifest(desired)
}

/// Apply an already-loaded desired manifest to the running system: resolve the
/// graphics policy, diff against the current applied state, run the Portage plan,
/// reconcile `@world`/runtime config, and persist the new state to
/// `current-manifest.toml`. Shared by `oxys apply` (which loads the cwd
/// `manifest.toml`) and `oxys install` (which compiles it from the source config).
pub(crate) fn apply_manifest(desired: SystemManifest) -> Result<()> {
    let root = effective_root();
    let resolved_graphics = desired
        .resolved_graphics()?
        .resolve_runtime_nodes()?
        .validate_source(&root)?;
    println!("{}", "Resolved graphics policy".cyan().bold());
    println!("{}", resolved_graphics.render());
    let effective_desired = resolved_graphics.materialize_manifest(&desired);
    let resolved_swap = effective_desired.resolved_swap()?;
    let effective_desired = resolved_swap.materialize_manifest(&effective_desired);
    let system_manifest_path = effective_system_manifest_path();
    let current = load_manifest_optional(&system_manifest_path)?;
    let current_swap = current
        .as_ref()
        .map(SystemManifest::resolved_swap)
        .transpose()?;
    let swap_reboot_required = validate_swap_transition(current_swap.as_ref(), &resolved_swap)?;

    println!("{}", "Applying manifest to running system".cyan().bold());
    let current_packages = current
        .as_ref()
        .map(|manifest| manifest.packages.as_slice())
        .unwrap_or(&[]);
    let changes = diff_packages(current_packages, &effective_desired.packages);
    print_changes(&changes);

    let plan = create_plan(&effective_desired)?;
    print_plan(&plan);
    fail_on_conflicts(&plan)?;
    println!("{}", "Applying Portage plan".yellow().bold());
    let mut stream = apply_portage_plan(
        &plan,
        &effective_portage_config_dir(),
        &root,
        Path::new(DEFAULT_PORTAGE_TMPDIR),
        plan.manifest.compiler.emerge_jobs,
    )?;
    for event in &mut stream {
        print_emerge_event(event);
    }
    stream.wait()?;

    reconcile_world(&changes, &root);

    let runtime = sync_runtime_config(&effective_desired, &root)?;
    if effective_desired.init_system == InitSystem::Openrc
        && effective_desired
            .services
            .openrc
            .runlevels()
            .any(|(_, services)| !services.is_empty())
    {
        let (sender, _receiver) = std::sync::mpsc::channel();
        activate_openrc_services(&effective_desired, &root, &sender)?;
        println!(
            "{}",
            "Reconciled authoritative OpenRC runlevels".green().bold()
        );
    }
    if runtime.prime_offload_configured {
        println!(
            "{}",
            "Configured NVIDIA PRIME render offload runtime files"
                .green()
                .bold()
        );
    } else if runtime.prime_primary_configured {
        println!(
            "{}",
            "Configured NVIDIA as the primary rendering path"
                .green()
                .bold()
        );
    }
    if swap_reboot_required {
        println!(
            "{}",
            "Swap configuration changed; reboot to reconcile the zram device safely"
                .yellow()
                .bold()
        );
    }
    persist_manifest_value(&effective_desired, &system_manifest_path)?;
    println!("{}", "Apply completed successfully".green().bold());
    Ok(())
}

fn validate_swap_transition(
    current: Option<&ResolvedSwap>,
    desired: &ResolvedSwap,
) -> std::io::Result<bool> {
    let Some(current) = current else {
        if desired.disk.is_some() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::Unsupported,
                "disk-backed swap requires an existing Oxys manifest proving the partition layout; use oxys install system to repartition",
            ));
        }
        return Ok(desired.zram.is_some());
    };

    if current.disk != desired.disk {
        return Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "changing disk-backed swap requires repartitioning and is not supported by oxys apply",
        ));
    }

    Ok(current.zram != desired.zram)
}

/// Brings the Portage world set in line with the manifest and surfaces (without running)
/// what `emerge --depclean` would remove. These are bookkeeping/advisory steps: the
/// packages themselves already converged successfully by the time this runs, so failures
/// here are reported as warnings rather than failing the whole apply.
fn reconcile_world(changes: &[PackageChange], root: &Path) {
    let added = changes
        .iter()
        .filter(|change| change.current.is_none() && change.desired.is_some())
        .map(|change| change.package.clone())
        .collect::<Vec<_>>();
    let removed = changes
        .iter()
        .filter(|change| change.current.is_some() && change.desired.is_none())
        .map(|change| change.package.clone())
        .collect::<Vec<_>>();

    if !added.is_empty() {
        println!("{}", "Registering new packages in world".yellow().bold());
        print_world_result(emerge_select(&added, root));
    }

    if !removed.is_empty() {
        println!("{}", "Deselecting removed packages".yellow().bold());
        print_world_result(emerge_deselect(&removed, root));
    }

    println!(
        "{}",
        "Packages depclean would remove (not run automatically \
         -- run `emerge --depclean` yourself to reclaim them):"
            .yellow()
            .bold()
    );
    print_world_result(emerge_depclean_pretend(root));
}

fn print_world_result(result: std::result::Result<String, oxys::use_resolver::UseResolverError>) {
    match result {
        Ok(output) => print!("{output}"),
        Err(err) => println!("{} {err}", "warning:".yellow().bold()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use oxys::{Compression, GB, ResolvedDiskSwap, ResolvedZram};

    fn resolved_swap(zram: bool, disk: bool) -> ResolvedSwap {
        ResolvedSwap {
            zram: zram.then_some(ResolvedZram {
                size: 4 * GB,
                algorithm: Compression::Zstd,
                priority: 100,
            }),
            disk: disk.then_some(ResolvedDiskSwap {
                size: 4 * GB,
                priority: 10,
            }),
            swappiness: 180,
        }
    }

    #[test]
    fn apply_rejects_disk_swap_without_a_trusted_current_layout() {
        let desired = resolved_swap(false, true);
        let error = validate_swap_transition(None, &desired).unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::Unsupported);
        assert!(error.to_string().contains("existing Oxys manifest"));
    }

    #[test]
    fn apply_requires_reboot_for_any_zram_transition() {
        let disabled = resolved_swap(false, false);
        let zram = resolved_swap(true, false);

        assert!(validate_swap_transition(Some(&disabled), &zram).unwrap());
        assert!(validate_swap_transition(Some(&zram), &disabled).unwrap());
        assert!(!validate_swap_transition(Some(&zram), &zram).unwrap());
    }
}
