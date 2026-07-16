use std::{fs, path::Path};

use oxys::manifest::{AudioStack, DisplayStack, InitSystem, Libc, Os, Package, SystemManifest};
use oxys::use_resolver::{
    DecisionAction, DecisionScope, DecisionSource, emerge_chroot_command_for_test,
    emerge_command_for_test, plan_portage, resolve_latest_version, write_portage_plan_config,
};

#[path = "portage_integration/constraints.rs"]
mod constraints;
#[path = "portage_integration/policy.rs"]
mod policy;
#[path = "portage_integration/resolution.rs"]
mod resolution;

fn write_md5_cache(
    portage_tree: &Path,
    package: &str,
    version: &str,
    contents: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let (category, package_name) = package.split_once('/').ok_or("invalid package")?;
    let path = portage_tree
        .join("metadata")
        .join("md5-cache")
        .join(category)
        .join(format!("{package_name}-{version}"));

    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }

    fs::write(path, contents)?;
    Ok(())
}

fn test_root(name: &str) -> std::path::PathBuf {
    let unique = format!(
        "oxys_portage_integration_{name}_{}_{}",
        std::process::id(),
        chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default()
    );
    std::env::temp_dir().join(unique)
}

fn cleanup(path: &Path) -> Result<(), Box<dyn std::error::Error>> {
    if path.exists() {
        fs::remove_dir_all(path)?;
    }
    Ok(())
}
