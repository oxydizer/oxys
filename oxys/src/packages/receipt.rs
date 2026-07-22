use std::{
    collections::{BTreeSet, HashMap},
    fs,
    io::Read,
    path::Path,
};

use chrono::{SecondsFormat, Utc};
use serde::{Deserialize, Serialize};

use super::{
    FileKind, FileRecord, PackageError, Result,
    format::{Artifact, hex, parse_prefixed_hash, read_artifact_file, sha256_reader},
    parallel,
    safe_fs::{Presence, SafeRoot},
    vdb,
};

const MAX_RECEIPT_SIZE: u64 = 64 * 1024;
const MAX_WORLD_SIZE: u64 = 16 * 1024 * 1024;
const MAX_TRANSACTION_SIZE: u64 = 64 * 1024;
const OPERATION_LOCK: &str = "var/lib/oxys/operation.lock";
const PORTAGE_VDB_LOCK: &str = "var/db/pkg";
const MAX_CONFIG_UPDATES: u32 = 10_000;
const HOLD_FILE: &str = "etc/portage/package.mask/oxys";
const HOLD_HEADER: &str = "# Managed by oxys - do not edit.\n# Version holds for packages installed from .oxys artifacts.\n";

#[derive(Clone, Debug, Serialize, Deserialize)]
struct Receipt {
    package: String,
    artifact: String,
    build_id: String,
    installed_at: String,
    transaction: String,
    #[serde(default)]
    world_added: bool,
    #[serde(default)]
    hold_added: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct Transaction {
    operation: String,
    package: String,
    artifact: String,
    phase: String,
    started_at: String,
    #[serde(default)]
    world_added: bool,
    #[serde(default)]
    hold_added: bool,
}

pub(crate) fn install(
    source: &Path,
    root_path: &Path,
    digest: &[u8; 32],
    artifact: &Artifact,
) -> Result<()> {
    let root = SafeRoot::open(root_path)?;
    let _operation_lock = root.lock(OPERATION_LOCK)?;
    // This is the same lock acquired by Portage's vardbapi.lock(): lockdir on
    // var/db/pkg creates var/db/.pkg.portage_lockfile and takes an fcntl lock.
    // Hold it across VDB reads, filesystem mutation, VDB commit, and world
    // updates so emerge and Oxys cannot observe or create a partial state.
    let _portage_vdb_lock = root.lock_portage(PORTAGE_VDB_LOCK)?;
    let receipt_relative = relative_receipt(
        &artifact.metadata.portage.category,
        &artifact.metadata.portage.pf,
    )?;
    let expected_package = package_id(artifact);
    let existing_receipt = read_receipt_optional(&root, &receipt_relative)?;
    if let Some(receipt) = &existing_receipt
        && (receipt.package != expected_package
            || parse_prefixed_hash(&receipt.artifact, "receipt artifact")? != *digest)
    {
        return Err(PackageError::invalid(format!(
            "a different .oxys artifact receipt already owns {}/{}",
            artifact.metadata.portage.category, artifact.metadata.portage.pf
        )));
    }

    let transaction_relative = relative_transaction(
        &artifact.metadata.portage.category,
        &artifact.metadata.portage.pf,
    )?;
    let existing_transaction = read_transaction_optional(&root, &transaction_relative)?;
    let expected_artifact = format!("sha256:{}", hex(digest));
    let resuming_install = existing_transaction.as_ref().is_some_and(|transaction| {
        transaction.operation == "install"
            && transaction.package == expected_package
            && transaction.artifact == expected_artifact
    });
    let completed_removal = existing_transaction.as_ref().is_some_and(|transaction| {
        existing_receipt.is_none()
            && transaction.operation == "remove"
            && transaction.package == expected_package
            && transaction.artifact == expected_artifact
    });
    if existing_transaction.is_some() && !resuming_install && !completed_removal {
        return Err(PackageError::invalid(
            "a conflicting interrupted .oxys transaction requires recovery",
        ));
    }
    if completed_removal {
        // Removal deletes its receipt only after files and world state are
        // committed. No receipt means only the final journal unlink remained.
        root.remove_control(&transaction_relative)?;
    }

    // Portage's VDB is authoritative for ownership, including packages that
    // were not installed by Oxys. Rebuild the reverse view for every operation
    // rather than maintaining a second persistent ownership database.
    preflight_install_ownership(&root, root_path, artifact, resuming_install)?;
    root.preflight_hardlinks(&artifact.files)?;
    // Surface a user-managed etc/portage/package.mask regular file as a clear
    // error before any journal or payload state exists, instead of a raw
    // ENOTDIR when the version hold is committed after the payload.
    root.preflight_control_directory(HOLD_FILE)?;
    let config_updates = plan_config_updates(&root, artifact)?;

    let cache_relative = cache_path(digest);
    match root.open_regular(&cache_relative) {
        Ok(file) => {
            if sha256_reader(file)? != *digest {
                return Err(PackageError::invalid(
                    "content-addressed artifact cache is corrupt",
                ));
            }
        }
        Err(PackageError::Io(error)) if error.kind() == std::io::ErrorKind::NotFound => {
            root.copy_control(&cache_relative, source, 0o644)?;
            if sha256_reader(root.open_regular(&cache_relative)?)? != *digest {
                return Err(PackageError::invalid(
                    "artifact changed while it was copied into the content-addressed cache",
                ));
            }
        }
        Err(error) => return Err(error),
    }

    let mut transaction = Transaction {
        operation: "install".into(),
        package: expected_package.clone(),
        artifact: expected_artifact,
        phase: "installing".into(),
        started_at: Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true),
        world_added: false,
        hold_added: false,
    };
    if resuming_install {
        transaction = existing_transaction.expect("resuming install has a transaction");
    } else {
        write_transaction(&root, &transaction_relative, &transaction)?;
    }

    for entry in artifact
        .entries
        .iter()
        .filter(|entry| entry.record.kind == FileKind::Directory)
    {
        root.install_entry(entry)?;
    }
    parallel::try_for_each(&artifact.entries, |entry| {
        if matches!(entry.record.kind, FileKind::Directory | FileKind::Hardlink)
            || config_updates.contains_key(&entry.record.path)
        {
            Ok(())
        } else {
            root.install_entry(entry)
        }
    })?;
    for entry in artifact.entries.iter().filter(|entry| {
        config_updates.contains_key(&entry.record.path)
            && !matches!(entry.record.kind, FileKind::Directory | FileKind::Hardlink)
    }) {
        let mut protected = entry.clone();
        protected.record.path = config_updates[&entry.record.path].clone();
        root.install_entry(&protected)?;
    }
    parallel::try_for_each(
        &artifact
            .entries
            .iter()
            .filter(|entry| entry.record.kind == FileKind::Hardlink)
            .collect::<Vec<_>>(),
        |entry| root.install_entry(entry),
    )?;
    for record in artifact
        .files
        .iter()
        .rev()
        .filter(|record| record.kind == FileKind::Directory)
    {
        root.finish_directory(record)?;
    }
    let mtimes = portage_mtimes(artifact)?;
    parallel::try_for_each(&artifact.files, |record| {
        let mtime = mtimes.get(&record.path).copied();
        let protected;
        let record = if let Some(path) = config_updates.get(&record.path) {
            protected = FileRecord {
                path: path.clone(),
                ..record.clone()
            };
            &protected
        } else {
            record
        };
        if let Some(seconds) = mtime {
            root.set_mtime(record, seconds)?;
        }
        Ok(())
    })?;

    // Re-check after writes. This is also what makes a repeated install a
    // verification-only idempotent operation.
    parallel::try_for_each(&artifact.files, |record| {
        let protected;
        let record = if let Some(path) = config_updates.get(&record.path) {
            protected = FileRecord {
                path: path.clone(),
                ..record.clone()
            };
            &protected
        } else {
            record
        };
        if root.inspect(record)? == Presence::Matches {
            Ok(())
        } else {
            Err(PackageError::invalid(format!(
                "installed path disappeared: {}",
                record.path
            )))
        }
    })?;
    if existing_receipt.is_none() {
        add_to_world(
            &root,
            &artifact.metadata.name,
            &transaction_relative,
            &mut transaction,
        )?;
        add_hold(
            &root,
            &artifact.metadata.portage.category,
            &artifact.metadata.portage.pf,
            &transaction_relative,
            &mut transaction,
        )?;
        let now = Utc::now();
        let receipt = Receipt {
            package: expected_package,
            artifact: format!("sha256:{}", hex(digest)),
            build_id: artifact.metadata.build_id.clone(),
            installed_at: now.to_rfc3339_opts(SecondsFormat::Secs, true),
            transaction: format!("{}-{}", now.format("%Y%m%dT%H%M%SZ"), &hex(digest)[..12]),
            world_added: transaction.world_added,
            hold_added: transaction.hold_added,
        };
        let rendered = toml::to_string(&receipt)?;
        root.write_control(&receipt_relative, rendered.as_bytes(), 0o644)?;
    }
    root.remove_control(&transaction_relative)?;
    Ok(())
}

pub(crate) fn remove(root_path: &Path, package: &str) -> Result<()> {
    let (category, pf) = parse_installed_package(package)?;
    let root = SafeRoot::open(root_path)?;
    let _operation_lock = root.lock(OPERATION_LOCK)?;
    let _portage_vdb_lock = root.lock_portage(PORTAGE_VDB_LOCK)?;
    let receipt_relative = relative_receipt(category, pf)?;
    let transaction_relative = relative_transaction(category, pf)?;
    let existing_transaction = read_transaction_optional(&root, &transaction_relative)?;
    let receipt = match read_receipt_optional(&root, &receipt_relative)? {
        Some(receipt) => receipt,
        None if existing_transaction
            .as_ref()
            .is_some_and(|transaction| transaction.operation == "remove") =>
        {
            root.remove_control(&transaction_relative)?;
            return Ok(());
        }
        None => {
            return Err(PackageError::invalid("package is not installed by .oxys"));
        }
    };
    let digest = parse_prefixed_hash(&receipt.artifact, "receipt artifact")?;
    let cache_relative = cache_path(&digest);
    let cache = root.open_regular(&cache_relative).map_err(|error| {
        PackageError::invalid(format!(
            "cached artifact required for safe removal is unavailable: {error}"
        ))
    })?;
    if sha256_reader(cache.try_clone()?)? != digest {
        return Err(PackageError::invalid(
            "cached artifact SHA-256 does not match receipt",
        ));
    }
    let artifact = read_artifact_file(cache)?;
    if artifact.metadata.portage.category != category
        || artifact.metadata.portage.pf != pf
        || receipt.package != package_id(&artifact)
        || receipt.build_id != artifact.metadata.build_id
    {
        return Err(PackageError::invalid(
            "receipt, cached artifact, and requested package do not agree",
        ));
    }

    if receipt.hold_added {
        // Surface a user-managed etc/portage/package.mask regular file as a
        // clear error before any payload deletion, not as a raw ENOTDIR when
        // the hold is pruned after the files are already gone.
        root.preflight_control_directory(HOLD_FILE)?;
    }

    let installed_package = format!("{category}/{pf}");
    let ownership = vdb::load_ownership(root_path, Some(&installed_package))?;
    let package_paths = vdb::artifact_owned_paths(&artifact)?;
    let shared_paths = package_paths
        .into_iter()
        .filter(|path| !ownership.owners(path).is_empty())
        .collect::<std::collections::BTreeSet<_>>();

    let removing = existing_transaction.as_ref().is_some_and(|transaction| {
        transaction.operation == "remove"
            && transaction.package == receipt.package
            && transaction.artifact == receipt.artifact
            && transaction.phase == "removing"
    });
    if existing_transaction.is_some() && !removing {
        return Err(PackageError::invalid(
            "a conflicting interrupted .oxys transaction requires recovery",
        ));
    }

    // A fresh removal verifies the complete package before committing to the
    // removal journal. A resumed removal permits objects already recorded as
    // deleted, but still verifies every object that remains.
    let mut preserved_config = BTreeSet::new();
    for record in &artifact.files {
        match root.inspect(record) {
            Ok(Presence::Matches) => {}
            Ok(Presence::Missing) if removing => {}
            Ok(Presence::Missing) => {
                return Err(PackageError::invalid(format!(
                    "installed path is missing: {}",
                    record.path
                )));
            }
            Err(_) if is_config_protected(record) && root.path_exists(&record.path)? => {
                preserved_config.insert(record.path.clone());
            }
            Err(error) => return Err(error),
        }
    }
    if !removing {
        write_transaction(
            &root,
            &transaction_relative,
            &Transaction {
                operation: "remove".into(),
                package: receipt.package.clone(),
                artifact: receipt.artifact.clone(),
                phase: "removing".into(),
                started_at: Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true),
                world_added: receipt.world_added,
                hold_added: receipt.hold_added,
            },
        )?;
    }
    // Aliases are removed before their canonical regular files. This keeps
    // hardlink identity verifiable throughout an interrupted/resumed removal.
    parallel::try_for_each(
        &artifact
            .files
            .iter()
            .filter(|record| {
                record.kind == FileKind::Hardlink
                    && !shared_paths.contains(&record.path)
                    && !preserved_config.contains(&record.path)
            })
            .collect::<Vec<_>>(),
        |record| root.remove_verified(record, removing),
    )?;
    parallel::try_for_each(&artifact.files, |record| {
        if matches!(record.kind, FileKind::Directory | FileKind::Hardlink)
            || shared_paths.contains(&record.path)
            || preserved_config.contains(&record.path)
        {
            Ok(())
        } else {
            root.remove_verified(record, removing)
        }
    })?;
    for record in
        artifact.files.iter().rev().filter(|record| {
            record.kind == FileKind::Directory && !shared_paths.contains(&record.path)
        })
    {
        root.remove_verified(record, true)?;
    }
    if receipt.world_added {
        remove_from_world(&root, &artifact.metadata.name)?;
    }
    if receipt.hold_added {
        remove_hold(&root, category, pf)?;
    }
    root.remove_control(&receipt_relative)?;
    root.remove_control(&transaction_relative)?;
    Ok(())
}

fn is_config_protected(record: &FileRecord) -> bool {
    record.path.starts_with("etc/") && matches!(record.kind, FileKind::Regular | FileKind::Symlink)
}

fn plan_config_updates(root: &SafeRoot, artifact: &Artifact) -> Result<HashMap<String, String>> {
    let mut updates = HashMap::new();
    for entry in artifact
        .entries
        .iter()
        .filter(|entry| is_config_protected(&entry.record))
    {
        match root.inspect(&entry.record) {
            Ok(Presence::Missing | Presence::Matches) => continue,
            Err(_) if root.path_exists(&entry.record.path)? => {}
            Err(error) => return Err(error),
        }

        let (parent, name) = entry.record.path.rsplit_once('/').ok_or_else(|| {
            PackageError::invalid(format!(
                "protected config has no parent: {}",
                entry.record.path
            ))
        })?;
        let mut selected = None;
        for number in 0..MAX_CONFIG_UPDATES {
            let candidate = format!("{parent}/._cfg{number:04}_{name}");
            let candidate_record = FileRecord {
                path: candidate.clone(),
                ..entry.record.clone()
            };
            match root.inspect(&candidate_record) {
                Ok(Presence::Matches) => {
                    selected = Some(candidate);
                    break;
                }
                Ok(Presence::Missing) => {
                    selected = Some(candidate);
                    break;
                }
                Err(_) if root.path_exists(&candidate)? => continue,
                Err(error) => return Err(error),
            }
        }
        let selected = selected.ok_or_else(|| {
            PackageError::invalid(format!(
                "no free Portage config-update name remains for {}",
                entry.record.path
            ))
        })?;
        updates.insert(entry.record.path.clone(), selected);
    }
    Ok(updates)
}

fn preflight_install_ownership(
    root: &SafeRoot,
    root_path: &Path,
    artifact: &Artifact,
    resuming_install: bool,
) -> Result<()> {
    let installed_package = format!(
        "{}/{}",
        artifact.metadata.portage.category, artifact.metadata.portage.pf
    );
    let ownership = vdb::load_ownership(
        root_path,
        resuming_install.then_some(installed_package.as_str()),
    )?;
    let package_paths = vdb::artifact_owned_paths(artifact)?;
    parallel::try_for_each(&artifact.files, |record| {
        let (presence, protected_change) = match root.inspect(record) {
            Ok(presence) => (presence, false),
            Err(_) if is_config_protected(record) && root.path_exists(&record.path)? => {
                (Presence::Matches, true)
            }
            Err(error) => return Err(error),
        };
        if !package_paths.contains(&record.path) {
            return Ok(());
        }
        let owners = ownership.owners(&record.path);
        if protected_change {
            let foreign_owners = owners
                .iter()
                .filter(|owner| owner.as_str() != installed_package)
                .cloned()
                .collect::<Vec<_>>();
            if !foreign_owners.is_empty() {
                return Err(PackageError::invalid(format!(
                    "protected config {} differs from the artifact but is owned by {}",
                    record.path,
                    foreign_owners.join(", ")
                )));
            }
            return Ok(());
        }
        match presence {
            Presence::Missing if !owners.is_empty() => Err(PackageError::invalid(format!(
                "Portage VDB says {} is owned by {}, but the path is missing",
                record.path,
                owners.join(", ")
            ))),
            Presence::Matches
                if owners.is_empty() && record.kind != FileKind::Directory && !resuming_install =>
            {
                Err(PackageError::invalid(format!(
                    "refusing to claim unowned existing path {}",
                    record.path
                )))
            }
            Presence::Missing | Presence::Matches => Ok(()),
        }
    })
}

fn package_id(artifact: &Artifact) -> String {
    format!(
        "gentoo/{}@{}#{}:{}",
        artifact.metadata.name,
        artifact.metadata.version,
        artifact.metadata.revision,
        artifact.metadata.slot
    )
}

fn cache_path(digest: &[u8; 32]) -> String {
    format!("var/cache/oxys/artifacts/sha256/{}.oxys", hex(digest))
}

fn portage_mtimes(artifact: &Artifact) -> Result<HashMap<String, i64>> {
    let contents_path = format!(
        "var/db/pkg/{}/{}/CONTENTS",
        artifact.metadata.portage.category, artifact.metadata.portage.pf
    );
    let contents = artifact
        .entries
        .iter()
        .find(|entry| entry.record.path == contents_path)
        .ok_or_else(|| PackageError::invalid("artifact has no VDB CONTENTS payload"))?;
    let text = std::str::from_utf8(&contents.data)
        .map_err(|_| PackageError::invalid("VDB CONTENTS is not UTF-8"))?;
    let mut result = HashMap::new();
    for line in text.lines() {
        let parsed = if let Some(rest) = line.strip_prefix("obj ") {
            let (rest, timestamp) = rest.rsplit_once(' ').ok_or_else(|| {
                PackageError::invalid(format!("malformed VDB CONTENTS line {line:?}"))
            })?;
            let (path, _) = rest.rsplit_once(' ').ok_or_else(|| {
                PackageError::invalid(format!("malformed VDB CONTENTS line {line:?}"))
            })?;
            Some((path, timestamp))
        } else if let Some(rest) = line.strip_prefix("sym ") {
            let (rest, timestamp) = rest.rsplit_once(' ').ok_or_else(|| {
                PackageError::invalid(format!("malformed VDB CONTENTS line {line:?}"))
            })?;
            let (path, _) = rest.split_once(" -> ").ok_or_else(|| {
                PackageError::invalid(format!("malformed VDB CONTENTS line {line:?}"))
            })?;
            Some((path, timestamp))
        } else if line.starts_with("dir ") {
            None
        } else {
            return Err(PackageError::invalid(format!(
                "unsupported VDB CONTENTS line {line:?}"
            )));
        };
        if let Some((path, timestamp)) = parsed {
            let relative = path.strip_prefix('/').ok_or_else(|| {
                PackageError::invalid(format!("VDB CONTENTS path is not absolute: {path:?}"))
            })?;
            super::format::validate_relative_path(relative)?;
            let timestamp = timestamp.parse::<i64>().map_err(|_| {
                PackageError::invalid(format!("invalid VDB CONTENTS timestamp {timestamp:?}"))
            })?;
            if result.insert(relative.to_owned(), timestamp).is_some() {
                return Err(PackageError::invalid(format!(
                    "duplicate VDB CONTENTS path {path:?}"
                )));
            }
        }
    }
    Ok(result)
}

fn relative_receipt(category: &str, pf: &str) -> Result<String> {
    validate_component(category)?;
    validate_component(pf)?;
    Ok(format!("var/lib/oxys/installed/{category}/{pf}.toml"))
}

fn relative_transaction(category: &str, pf: &str) -> Result<String> {
    validate_component(category)?;
    validate_component(pf)?;
    Ok(format!("var/lib/oxys/transactions/{category}/{pf}.toml"))
}

fn parse_installed_package(package: &str) -> Result<(&str, &str)> {
    let (category, pf) = package
        .split_once('/')
        .ok_or_else(|| PackageError::invalid("remove target must be category/PF"))?;
    validate_component(category)?;
    validate_component(pf)?;
    Ok((category, pf))
}

fn validate_component(value: &str) -> Result<()> {
    if value.is_empty()
        || value == "."
        || value == ".."
        || value.contains('/')
        || value.contains('\0')
    {
        return Err(PackageError::invalid("unsafe package identity component"));
    }
    Ok(())
}

fn read_receipt_optional(root: &SafeRoot, path: &str) -> Result<Option<Receipt>> {
    match root.open_regular(path) {
        Ok(file) => Ok(Some(read_receipt_file(file)?)),
        Err(PackageError::Io(error)) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

fn read_receipt_file(file: fs::File) -> Result<Receipt> {
    let mut bytes = Vec::new();
    file.take(MAX_RECEIPT_SIZE + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_RECEIPT_SIZE {
        return Err(PackageError::invalid("receipt exceeds size limit"));
    }
    let text =
        std::str::from_utf8(&bytes).map_err(|_| PackageError::invalid("receipt is not UTF-8"))?;
    Ok(toml::from_str(text)?)
}

fn read_transaction_optional(root: &SafeRoot, path: &str) -> Result<Option<Transaction>> {
    let file = match root.open_regular(path) {
        Ok(file) => file,
        Err(PackageError::Io(error)) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(None);
        }
        Err(error) => return Err(error),
    };
    let mut bytes = Vec::new();
    file.take(MAX_TRANSACTION_SIZE + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_TRANSACTION_SIZE {
        return Err(PackageError::invalid(
            "transaction journal exceeds size limit",
        ));
    }
    let text = std::str::from_utf8(&bytes)
        .map_err(|_| PackageError::invalid("transaction journal is not UTF-8"))?;
    Ok(Some(toml::from_str(text)?))
}

fn write_transaction(root: &SafeRoot, path: &str, transaction: &Transaction) -> Result<()> {
    let rendered = toml::to_string(transaction)?;
    root.write_control(path, rendered.as_bytes(), 0o600)
}

fn add_to_world(
    root: &SafeRoot,
    atom: &str,
    transaction_path: &str,
    transaction: &mut Transaction,
) -> Result<()> {
    let path = "var/lib/portage/world";
    let _portage_lock = root.lock_portage(path)?;
    if read_control_optional(root, path, MAX_WORLD_SIZE)?.is_none() {
        root.write_control(path, b"", 0o644)?;
    }
    let existing = read_control_optional(root, path, MAX_WORLD_SIZE)?.unwrap_or_default();
    if existing.lines().any(|line| line.trim() == atom) {
        return Ok(());
    }
    // Record ownership of the future world entry before committing it. This
    // makes a crash on either side of the world-file rename recoverable.
    transaction.world_added = true;
    write_transaction(root, transaction_path, transaction)?;
    let mut updated = existing;
    if !updated.is_empty() && !updated.ends_with('\n') {
        updated.push('\n');
    }
    updated.push_str(atom);
    updated.push('\n');
    root.write_control(path, updated.as_bytes(), 0o644)?;
    Ok(())
}

fn hold_atom(category: &str, pf: &str) -> String {
    format!(">{category}/{pf}")
}

/// Register a Portage version hold so `emerge -uDN @world` cannot upgrade
/// over the oxys-managed package when the tree carries a newer version. The
/// VDB entry remains the source of truth; the hold only pins the ceiling.
fn add_hold(
    root: &SafeRoot,
    category: &str,
    pf: &str,
    transaction_path: &str,
    transaction: &mut Transaction,
) -> Result<()> {
    let atom = hold_atom(category, pf);
    let existing = read_control_optional(root, HOLD_FILE, MAX_WORLD_SIZE)?;
    if existing
        .as_deref()
        .is_some_and(|content| content.lines().any(|line| line.trim() == atom))
    {
        // A pre-existing hold line stays user-owned; removal must not prune it.
        return Ok(());
    }
    // Record ownership of the future hold line before committing it. This
    // makes a crash on either side of the fragment rename recoverable.
    transaction.hold_added = true;
    write_transaction(root, transaction_path, transaction)?;
    let mut updated = existing.unwrap_or_else(|| HOLD_HEADER.to_owned());
    if !updated.is_empty() && !updated.ends_with('\n') {
        updated.push('\n');
    }
    updated.push_str(&atom);
    updated.push('\n');
    root.write_control(HOLD_FILE, updated.as_bytes(), 0o644)?;
    Ok(())
}

fn remove_hold(root: &SafeRoot, category: &str, pf: &str) -> Result<()> {
    let atom = hold_atom(category, pf);
    let Some(existing) = read_control_optional(root, HOLD_FILE, MAX_WORLD_SIZE)? else {
        return Ok(());
    };
    let mut updated = String::new();
    let mut holds_remain = false;
    for line in existing.lines() {
        let trimmed = line.trim();
        if trimmed == atom {
            continue;
        }
        if !trimmed.is_empty() && !trimmed.starts_with('#') {
            holds_remain = true;
        }
        updated.push_str(line);
        updated.push('\n');
    }
    if holds_remain {
        root.write_control(HOLD_FILE, updated.as_bytes(), 0o644)
    } else {
        // Only the oxys header (and blank lines) are left; drop the fragment
        // instead of keeping an empty stub in the user's /etc/portage.
        root.remove_control(HOLD_FILE)
    }
}

fn remove_from_world(root: &SafeRoot, atom: &str) -> Result<()> {
    let path = "var/lib/portage/world";
    let _portage_lock = root.lock_portage(path)?;
    let Some(existing) = read_control_optional(root, path, MAX_WORLD_SIZE)? else {
        return Ok(());
    };
    let mut updated = String::new();
    for line in existing.lines() {
        if line.trim() != atom {
            updated.push_str(line);
            updated.push('\n');
        }
    }
    root.write_control(path, updated.as_bytes(), 0o644)
}

fn read_control_optional(root: &SafeRoot, path: &str, limit: u64) -> Result<Option<String>> {
    let file = match root.open_regular(path) {
        Ok(file) => file,
        Err(PackageError::Io(error)) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(None);
        }
        Err(error) => return Err(error),
    };
    let mut bytes = Vec::new();
    file.take(limit + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > limit {
        return Err(PackageError::invalid(format!(
            "control file {path} exceeds size limit"
        )));
    }
    String::from_utf8(bytes)
        .map(Some)
        .map_err(|_| PackageError::invalid(format!("control file {path} is not UTF-8")))
}
