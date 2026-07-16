mod format;
mod parallel;
mod receipt;
mod safe_fs;
mod target;
mod vdb;

use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    os::unix::fs::OpenOptionsExt,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};

pub use format::{Artifact, FileKind, FileRecord, Metadata, PackageError};

pub type Result<T> = std::result::Result<T, PackageError>;

/// Number of workers used for independent package verification and filesystem
/// operations. One available CPU is reserved for the rest of the system.
pub fn install_worker_count() -> usize {
    parallel::worker_count()
}

/// Build a format-v1 `.oxys` artifact from a package installed by Portage in
/// `reference_root`.
pub fn build(reference_root: &Path, atom: &str, output: &Path) -> Result<Metadata> {
    let captured = vdb::capture(reference_root, atom)?;
    format::write_artifact(output, captured.metadata, captured.entries)
}

/// Fully verify an artifact without changing a filesystem.
pub fn verify(path: &Path) -> Result<Metadata> {
    Ok(format::read_artifact(path)?.metadata)
}

/// Install an artifact into `root`, including its captured Portage VDB entry.
pub fn install(path: &Path, root: &Path) -> Result<Metadata> {
    let snapshot = ArtifactSnapshot::create(path)?;
    let artifact = format::read_artifact(&snapshot.path)?;
    let digest = format::sha256_file(&snapshot.path)?;
    target::validate(root, &artifact.metadata.target)?;
    receipt::install(&snapshot.path, root, &digest, &artifact)?;
    Ok(artifact.metadata)
}

/// Remove an artifact-installed package by `category/PF`.
pub fn remove(root: &Path, package: &str) -> Result<()> {
    receipt::remove(root, package)
}

pub fn receipt_path(root: &Path, category: &str, pf: &str) -> PathBuf {
    root.join("var/lib/oxys/installed")
        .join(category)
        .join(format!("{pf}.toml"))
}

static SNAPSHOT_SEQUENCE: AtomicU64 = AtomicU64::new(0);

struct ArtifactSnapshot {
    path: PathBuf,
}

impl ArtifactSnapshot {
    fn create(source: &Path) -> Result<Self> {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            ".oxys-artifact-{}-{nonce}-{}",
            std::process::id(),
            SNAPSHOT_SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ));
        let mut input = File::open(source)?;
        let snapshot = Self { path };
        let mut output = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&snapshot.path)?;
        let mut limited = (&mut input).take(format::MAX_ARTIFACT_SIZE + 1);
        let copied = std::io::copy(&mut limited, &mut output)?;
        if copied > format::MAX_ARTIFACT_SIZE {
            return Err(PackageError::invalid("artifact exceeds total size limit"));
        }
        output.flush()?;
        Ok(snapshot)
    }
}

impl Drop for ArtifactSnapshot {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}
