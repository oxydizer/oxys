use std::{
    fs, io,
    path::{Path, PathBuf},
};

use colored::Colorize;
use oxys::{
    SystemManifest, manifest_to_toml, parse_generated_manifest_toml,
    use_resolver::{PortagePlan, plan_portage},
};

use crate::{DEFAULT_PORTAGE_CONFIG_DIR, DEFAULT_PORTAGE_TREE, DEFAULT_ROOT, Result, SYSTEM_MANIFEST};

pub(crate) fn create_plan(manifest: &SystemManifest) -> Result<PortagePlan> {
    let resolved_graphics = manifest.resolved_graphics()?;
    let effective = resolved_graphics.materialize_manifest(manifest);
    Ok(plan_portage(
        &effective,
        &effective_portage_tree(),
        &oxys::util::default_use_resolver_cache_dir(),
    )?)
}

pub(crate) fn effective_portage_tree() -> PathBuf {
    std::env::var_os("OXYS_PORTAGE_TREE")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(DEFAULT_PORTAGE_TREE))
}

pub(crate) fn effective_portage_config_dir() -> PathBuf {
    std::env::var_os("OXYS_PORTAGE_CONFIG_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(DEFAULT_PORTAGE_CONFIG_DIR))
}

/// Root the running system is mounted at. Overridable so tests (and alternate-root
/// applies) can target a sandbox instead of writing runtime config under a real `/`.
pub(crate) fn effective_root() -> PathBuf {
    std::env::var_os("OXYS_ROOT")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(DEFAULT_ROOT))
}

pub(crate) fn effective_system_manifest_path() -> PathBuf {
    std::env::var_os("OXYS_SYSTEM_MANIFEST")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(SYSTEM_MANIFEST))
}

pub(crate) fn load_manifest(path: &Path) -> Result<SystemManifest> {
    let text = fs::read_to_string(path)?;
    parse_generated_manifest_toml(&text).map_err(|err| io::Error::new(
        io::ErrorKind::InvalidData,
        format!("failed to parse {} as generated oxys manifest: {err}. Regenerate it with the real oxys crate", path.display()),
    ).into())
}

pub(crate) fn load_manifest_optional(path: &Path) -> Result<Option<SystemManifest>> {
    if path.exists() {
        load_manifest(path).map(Some)
    } else {
        Ok(None)
    }
}

pub(crate) fn persist_manifest_value(manifest: &SystemManifest, destination: &Path) -> Result<()> {
    if let Some(parent) = destination.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(destination, manifest_to_toml(manifest)?)?;
    println!(
        "{} {}",
        "Saved current manifest:".green().bold(),
        destination.display().to_string().green()
    );
    Ok(())
}
