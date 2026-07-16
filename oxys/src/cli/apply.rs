use std::path::Path;

use colored::Colorize;
use oxys::{
    SystemManifest,
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
    let system_manifest_path = effective_system_manifest_path();
    let current = load_manifest_optional(&system_manifest_path)?;

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
    persist_manifest_value(&effective_desired, &system_manifest_path)?;
    println!("{}", "Apply completed successfully".green().bold());
    Ok(())
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
