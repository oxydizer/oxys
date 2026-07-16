use std::{error::Error, path::Path};

pub(crate) fn build(root: &Path, atom: &str, output: &Path) -> Result<(), Box<dyn Error>> {
    let metadata = oxys::packages::build(root, atom, output)?;
    println!(
        "Built {} for {} as {}",
        output.display(),
        metadata.target.triple,
        metadata.build_id
    );
    Ok(())
}

pub(crate) fn inspect(artifact: &Path) -> Result<(), Box<dyn Error>> {
    let metadata = oxys::packages::verify(artifact)?;
    print!("{}", toml::to_string_pretty(&metadata)?);
    Ok(())
}

pub(crate) fn verify(artifact: &Path) -> Result<(), Box<dyn Error>> {
    let metadata = oxys::packages::verify(artifact)?;
    println!(
        "Verified {}/{} ({} files, {} bytes uncompressed)",
        metadata.portage.category,
        metadata.portage.pf,
        metadata.payload.file_count,
        metadata.payload.uncompressed_size
    );
    Ok(())
}

pub(crate) fn install(artifact: &Path, root: &Path) -> Result<(), Box<dyn Error>> {
    println!(
        "Installing with {} workers (one available CPU reserved)",
        oxys::packages::install_worker_count()
    );
    let metadata = oxys::packages::install(artifact, root)?;
    println!(
        "Installed {}/{} into {}",
        metadata.portage.category,
        metadata.portage.pf,
        root.display()
    );
    Ok(())
}

pub(crate) fn remove(package: &str, root: &Path) -> Result<(), Box<dyn Error>> {
    oxys::packages::remove(root, package)?;
    println!("Removed {package} from {}", root.display());
    Ok(())
}
