use super::*;

pub fn write_portage_config(
    resolution: &UseResolution,
    portage_config_dir: &Path,
) -> Result<(), UseResolverError> {
    fs::create_dir_all(portage_config_dir).map_err(|source| UseResolverError::Io {
        path: portage_config_dir.to_path_buf(),
        source,
    })?;

    write_generated_file(
        &portage_config_dir.join("package.use"),
        &render_package_use(&resolution.package_use),
    )?;
    write_generated_file(
        &portage_config_dir.join("package.accept_keywords"),
        &render_accept_keywords(&resolution.accept_keywords)?,
    )?;
    write_generated_file(
        &portage_config_dir.join("package.license"),
        &render_accept_licenses(&resolution.accept_licenses)?,
    )?;
    write_generated_file(
        &portage_config_dir.join("make.conf"),
        &render_make_conf(&resolution.global_use),
    )?;

    Ok(())
}

/// Writes generated Portage configuration files for the supplied Portage plan.
pub fn write_portage_plan_config(
    plan: &PortagePlan,
    portage_config_dir: &Path,
) -> Result<(), UseResolverError> {
    fs::create_dir_all(portage_config_dir).map_err(|source| UseResolverError::Io {
        path: portage_config_dir.to_path_buf(),
        source,
    })?;

    let package_use_dir = portage_config_dir.join("package.use");
    let accept_keywords_dir = portage_config_dir.join("package.accept_keywords");
    let package_license_dir = portage_config_dir.join("package.license");
    ensure_generated_directory(&package_use_dir)?;
    ensure_generated_directory(&accept_keywords_dir)?;
    ensure_generated_directory(&package_license_dir)?;

    write_generated_file(
        &package_use_dir.join("oxys"),
        &render_package_use(&plan.resolution.package_use),
    )?;
    write_generated_file(
        &accept_keywords_dir.join("oxys"),
        &render_accept_keywords(&plan.resolution.accept_keywords)?,
    )?;
    write_generated_file(
        &package_license_dir.join("oxys"),
        &render_accept_licenses(&plan.resolution.accept_licenses)?,
    )?;

    let make_conf_output = generate_make_conf(&plan.manifest, &plan.resolution.global_use);
    write_generated_file(
        &portage_config_dir.join("make.conf"),
        &make_conf_output.make_conf,
    )?;

    write_generated_file(
        &package_use_dir.join("pgo"),
        &package_use_pgo_contents(&plan.manifest),
    )?;
    write_generated_file(
        &package_use_dir.join("no-pgo"),
        &package_use_no_pgo_contents(&plan.manifest),
    )?;

    Ok(())
}

/// Generates /etc/portage/make.conf content from the Oxys config plus resolved Portage policy.
fn ensure_generated_directory(path: &Path) -> Result<(), UseResolverError> {
    if path.is_file() {
        fs::remove_file(path).map_err(|source| UseResolverError::Io {
            path: path.to_path_buf(),
            source,
        })?;
    }

    fs::create_dir_all(path).map_err(|source| UseResolverError::Io {
        path: path.to_path_buf(),
        source,
    })?;

    Ok(())
}

fn write_generated_file(path: &Path, contents: &str) -> Result<(), UseResolverError> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|source| UseResolverError::Io {
            path: parent.to_path_buf(),
            source,
        })?;
    }

    let temp_path = sibling_temp_path(path, "generated.tmp");
    fs::write(&temp_path, contents).map_err(|source| UseResolverError::Io {
        path: temp_path.clone(),
        source,
    })?;
    fs::rename(&temp_path, path).map_err(|source| UseResolverError::Io {
        path: path.to_path_buf(),
        source,
    })?;

    Ok(())
}
