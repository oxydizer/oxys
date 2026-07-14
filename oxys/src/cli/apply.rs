use std::path::Path;

use colored::Colorize;
use oxys::{
    diff::{diff_packages, PackageChange},
    runtime::sync_runtime_config,
    use_resolver::{apply_portage_plan, emerge_deselect, emerge_depclean_pretend, emerge_select},
};

use super::{
    manifest_io::{effective_portage_config_dir, effective_system_manifest_path, load_manifest_optional},
    output::{fail_on_conflicts, print_changes, print_plan},
};
use crate::{
    create_plan, load_manifest, persist_manifest, print_emerge_event, DEFAULT_PORTAGE_TMPDIR,
    DEFAULT_ROOT, LOCAL_MANIFEST, Result,
};

pub(crate) fn run() -> Result<()> {
    let desired_path = Path::new(LOCAL_MANIFEST);
    let desired = load_manifest(desired_path)?;
    let system_manifest_path = effective_system_manifest_path();
    let current = load_manifest_optional(&system_manifest_path)?;

    println!("{}", "Applying manifest to running system".cyan().bold());
    let current_packages = current
        .as_ref()
        .map(|manifest| manifest.packages.as_slice())
        .unwrap_or(&[]);
    let changes = diff_packages(current_packages, &desired.packages);
    print_changes(&changes);

    let plan = create_plan(&desired)?;
    print_plan(&plan);
    fail_on_conflicts(&plan)?;
    println!("{}", "Applying Portage plan".yellow().bold());
    let mut stream = apply_portage_plan(
        &plan,
        &effective_portage_config_dir(),
        Path::new(DEFAULT_ROOT),
        Path::new(DEFAULT_PORTAGE_TMPDIR),
        plan.manifest.compiler.emerge_jobs,
    )?;
    for event in &mut stream {
        print_emerge_event(event);
    }
    stream.wait()?;

    reconcile_world(&changes);

    if sync_runtime_config(&desired, Path::new(DEFAULT_ROOT))?.prime_offload_configured {
        println!(
            "{}",
            "Configured NVIDIA PRIME render offload runtime files"
                .green()
                .bold()
        );
    }
    persist_manifest(desired_path, &system_manifest_path)?;
    println!("{}", "Apply completed successfully".green().bold());
    Ok(())
}

/// Brings the Portage world set in line with the manifest and surfaces (without running)
/// what `emerge --depclean` would remove. These are bookkeeping/advisory steps: the
/// packages themselves already converged successfully by the time this runs, so failures
/// here are reported as warnings rather than failing the whole apply.
fn reconcile_world(changes: &[PackageChange]) {
    let root = Path::new(DEFAULT_ROOT);
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
