use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    fs,
    os::unix::fs::{MetadataExt, PermissionsExt},
    path::{Path, PathBuf},
};

use super::{
    FileKind, FileRecord, Metadata, PackageError, Result,
    format::{
        BuildMetadata, PayloadEntry, PayloadMetadata, PortageMetadata, TargetMetadata, sha256,
    },
};

pub(crate) struct CapturedPackage {
    pub metadata: Metadata,
    pub entries: Vec<PayloadEntry>,
}

/// Package-file ownership reconstructed from Portage's authoritative VDB.
///
/// This is deliberately an in-memory view rather than persistent Oxys state:
/// every operation observes packages installed by either Portage or Oxys and
/// cannot drift from `/var/db/pkg/*/*/CONTENTS`.
pub(crate) struct OwnershipMap {
    owners: HashMap<String, Vec<String>>,
}

impl OwnershipMap {
    pub(crate) fn owners(&self, path: &str) -> &[String] {
        self.owners.get(path).map(Vec::as_slice).unwrap_or(&[])
    }
}

/// Load the reverse path ownership map from every complete VDB entry except
/// `excluded_package`, whose identity is `category/PF`.
pub(crate) fn load_ownership(root: &Path, excluded_package: Option<&str>) -> Result<OwnershipMap> {
    let vdb_root = root.join("var/db/pkg");
    let categories = match fs::read_dir(&vdb_root) {
        Ok(entries) => entries.collect::<std::result::Result<Vec<_>, _>>()?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(OwnershipMap {
                owners: HashMap::new(),
            });
        }
        Err(error) => return Err(error.into()),
    };

    let mut owners: HashMap<String, Vec<String>> = HashMap::new();
    for category in categories {
        if !category.file_type()?.is_dir() {
            continue;
        }
        let category_name = category
            .file_name()
            .into_string()
            .map_err(|_| PackageError::invalid("VDB category is not UTF-8"))?;
        validate_vdb_component(&category_name)?;
        for package in fs::read_dir(category.path())? {
            let package = package?;
            if !package.file_type()?.is_dir() {
                continue;
            }
            let pf = package
                .file_name()
                .into_string()
                .map_err(|_| PackageError::invalid("VDB PF is not UTF-8"))?;
            validate_vdb_component(&pf)?;
            let package_id = format!("{category_name}/{pf}");
            if excluded_package == Some(package_id.as_str()) {
                continue;
            }
            let contents_pathname = package.path().join("CONTENTS");
            let contents = fs::read_to_string(&contents_pathname).map_err(|error| {
                PackageError::invalid(format!(
                    "cannot read authoritative VDB ownership from {}: {error}",
                    contents_pathname.display()
                ))
            })?;
            let mut package_paths = BTreeSet::new();
            for line in contents.lines() {
                package_paths.insert(contents_path(line)?);
            }
            for path in package_paths {
                owners.entry(path).or_default().push(package_id.clone());
            }
        }
    }
    for packages in owners.values_mut() {
        packages.sort();
    }
    Ok(OwnershipMap { owners })
}

/// Return only the installed package paths declared by the artifact's captured
/// VDB CONTENTS. The artifact's private VDB control tree is intentionally not
/// considered package-file ownership.
pub(crate) fn artifact_owned_paths(artifact: &super::format::Artifact) -> Result<BTreeSet<String>> {
    let contents_pathname = format!(
        "var/db/pkg/{}/{}/CONTENTS",
        artifact.metadata.portage.category, artifact.metadata.portage.pf
    );
    let contents = artifact
        .entries
        .iter()
        .find(|entry| entry.record.path == contents_pathname)
        .ok_or_else(|| PackageError::invalid("artifact has no VDB CONTENTS payload"))?;
    let contents = std::str::from_utf8(&contents.data)
        .map_err(|_| PackageError::invalid("VDB CONTENTS is not UTF-8"))?;
    contents.lines().map(contents_path).collect()
}

fn validate_vdb_component(value: &str) -> Result<()> {
    if value.is_empty()
        || value == "."
        || value == ".."
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"+_.-".contains(&byte))
    {
        return Err(PackageError::invalid(format!(
            "unsafe VDB package identity component {value:?}"
        )));
    }
    Ok(())
}

struct CapturedPath {
    entry: PayloadEntry,
    inode: Option<(u64, u64, u64)>,
}

pub(crate) fn capture(root: &Path, atom: &str) -> Result<CapturedPackage> {
    reject_symlink_root(root)?;
    let (category, package) = parse_atom(atom)?;
    let category_dir = root.join("var/db/pkg").join(category);
    let prefix = format!("{package}-");
    let mut matches = fs::read_dir(&category_dir)
        .map_err(|error| {
            PackageError::invalid(format!("cannot read {}: {error}", category_dir.display()))
        })?
        .filter_map(|entry| entry.ok())
        .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
        .filter(|entry| {
            entry
                .file_name()
                .to_str()
                .is_some_and(|name| name.starts_with(&prefix))
        })
        .map(|entry| entry.path())
        .collect::<Vec<_>>();
    matches.sort();
    let vdb = match matches.as_slice() {
        [only] => only.clone(),
        [] => {
            return Err(PackageError::invalid(format!(
                "{atom} is not installed in {}",
                root.display()
            )));
        }
        _ => {
            return Err(PackageError::invalid(format!(
                "{atom} has multiple installed VDB instances; specify an MVP-safe single-version root"
            )));
        }
    };
    reject_symlink_path(root, &vdb)?;
    let pf = vdb
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| PackageError::invalid("VDB PF is not UTF-8"))?
        .to_owned();
    let version = pf
        .strip_prefix(&prefix)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| PackageError::invalid("cannot derive version from VDB PF"))?
        .to_owned();
    let contents = fs::read_to_string(vdb.join("CONTENTS")).map_err(|error| {
        PackageError::invalid(format!("cannot read complete VDB CONTENTS: {error}"))
    })?;
    let mut package_paths = BTreeSet::new();
    for line in contents.lines() {
        let path = contents_path(line)?;
        package_paths.insert(path);
    }

    let mut captured = Vec::new();
    for relative in package_paths {
        captured.push(capture_path(root, &relative)?);
    }
    capture_tree(root, &vdb, &mut captured)?;
    captured.sort_by(|left, right| {
        left.entry
            .record
            .path
            .as_bytes()
            .cmp(right.entry.record.path.as_bytes())
    });
    for pair in captured.windows(2) {
        if pair[0].entry.record.path == pair[1].entry.record.path {
            return Err(PackageError::invalid(format!(
                "duplicate captured path {}",
                pair[0].entry.record.path
            )));
        }
    }
    canonicalize_hardlinks(&mut captured)?;
    let entries = captured.into_iter().map(|path| path.entry).collect();

    let repository = read_vdb(&vdb, "repository").unwrap_or_else(|| "gentoo".into());
    let slot = read_vdb(&vdb, "SLOT").unwrap_or_else(|| "0".into());
    let chost = read_vdb(&vdb, "CHOST").unwrap_or_else(|| "unknown-linux-gnu".into());
    let cflags = read_vdb(&vdb, "CFLAGS").unwrap_or_default();
    let cpu = cflags
        .split_whitespace()
        .find_map(|flag| flag.strip_prefix("-march="))
        .unwrap_or(if cfg!(target_arch = "x86_64") {
            "x86-64"
        } else {
            std::env::consts::ARCH
        })
        .to_owned();
    let use_flags = read_vdb(&vdb, "USE").map_or_else(Vec::new, |value| {
        value.split_whitespace().map(str::to_owned).collect()
    });
    let revision = read_vdb(&vdb, "BUILD_ID")
        .and_then(|value| value.parse().ok())
        .unwrap_or(0);
    let profile = read_vdb(&vdb, "PROFILE_PATHS").unwrap_or_else(|| "unknown".into());
    let compiler = read_vdb(&vdb, "CC").unwrap_or_else(|| "unknown".into());
    let metadata = Metadata {
        format: 1,
        kind: "package".into(),
        namespace: "gentoo".into(),
        name: format!("{category}/{package}"),
        version,
        version_scheme: "gentoo".into(),
        revision,
        slot,
        build_id: format!("sha256:{}", "0".repeat(64)),
        target: TargetMetadata {
            triple: chost,
            cpu,
            libc: "glibc".into(),
            libc_min: "unknown".into(),
            init: "openrc".into(),
        },
        payload: PayloadMetadata {
            compression: "zstd".into(),
            uncompressed_size: 0,
            file_count: 0,
            sha256: format!("sha256:{}", "0".repeat(64)),
            file_table_sha256: format!("sha256:{}", "0".repeat(64)),
        },
        build: BuildMetadata {
            builder: "oxys-package-spike".into(),
            method: "portage-reference-root".into(),
            repo: repository.clone(),
            repo_commit: read_vdb(&vdb, "REPO_REVISIONS").unwrap_or_else(|| "unknown".into()),
            ebuild: format!("{category}/{pf}.ebuild"),
            profile,
            use_flags,
            compiler,
            cflags,
        },
        portage: PortageMetadata {
            category: category.into(),
            pf,
            repository,
            vdb_payload: true,
        },
    };
    Ok(CapturedPackage { metadata, entries })
}

fn parse_atom(atom: &str) -> Result<(&str, &str)> {
    let (category, package) = atom
        .split_once('/')
        .ok_or_else(|| PackageError::invalid("package must be category/package"))?;
    if category.is_empty()
        || package.is_empty()
        || package.contains('/')
        || [category, package].iter().any(|value| {
            *value == "."
                || *value == ".."
                || !value
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || b"+_.-".contains(&byte))
        })
    {
        return Err(PackageError::invalid(
            "package must be a safe category/package atom",
        ));
    }
    Ok((category, package))
}

fn contents_path(line: &str) -> Result<String> {
    let path = if let Some(rest) = line.strip_prefix("obj ") {
        let (rest, _) = rest.rsplit_once(' ').ok_or_else(|| {
            PackageError::invalid(format!("malformed VDB CONTENTS line {line:?}"))
        })?;
        let (path, _) = rest.rsplit_once(' ').ok_or_else(|| {
            PackageError::invalid(format!("malformed VDB CONTENTS line {line:?}"))
        })?;
        path
    } else if let Some(rest) = line.strip_prefix("sym ") {
        let (rest, _) = rest.rsplit_once(' ').ok_or_else(|| {
            PackageError::invalid(format!("malformed VDB CONTENTS line {line:?}"))
        })?;
        let (path, _) = rest.split_once(" -> ").ok_or_else(|| {
            PackageError::invalid(format!("malformed VDB CONTENTS line {line:?}"))
        })?;
        path
    } else if let Some(path) = line.strip_prefix("dir ") {
        path
    } else if line.trim().is_empty() {
        return Err(PackageError::invalid("blank line in VDB CONTENTS"));
    } else {
        return Err(PackageError::invalid(format!(
            "unsupported VDB CONTENTS record {line:?}"
        )));
    };
    let relative = path.strip_prefix('/').ok_or_else(|| {
        PackageError::invalid(format!("VDB CONTENTS path is not absolute: {path:?}"))
    })?;
    super::format::validate_relative_path(relative)?;
    Ok(relative.to_owned())
}

fn capture_tree(root: &Path, directory: &Path, entries: &mut Vec<CapturedPath>) -> Result<()> {
    entries.push(capture_path(root, relative_utf8(root, directory)?)?);
    let mut children = fs::read_dir(directory)?.collect::<std::result::Result<Vec<_>, _>>()?;
    children.sort_by_key(|entry| entry.file_name());
    for child in children {
        let path = child.path();
        reject_symlink_path(root, &path)?;
        let metadata = fs::symlink_metadata(&path)?;
        if metadata.file_type().is_dir() {
            capture_tree(root, &path, entries)?;
        } else {
            entries.push(capture_path(root, relative_utf8(root, &path)?)?);
        }
    }
    Ok(())
}

fn relative_utf8<'a>(root: &Path, path: &'a Path) -> Result<&'a str> {
    path.strip_prefix(root)
        .map_err(|_| PackageError::invalid("captured path escaped reference root"))?
        .to_str()
        .ok_or_else(|| PackageError::invalid("captured path is not UTF-8"))
}

fn capture_path(root: &Path, relative: &str) -> Result<CapturedPath> {
    super::format::validate_relative_path(relative)?;
    let path = root.join(relative);
    reject_symlink_parents(root, &path)?;
    let metadata = fs::symlink_metadata(&path).map_err(|error| {
        PackageError::invalid(format!(
            "VDB-owned path /{relative} cannot be captured: {error}"
        ))
    })?;
    let mode = metadata.permissions().mode() & 0o7777;
    if mode & 0o6000 != 0 {
        return Err(PackageError::invalid(format!(
            "MVP rejects setuid/setgid path /{relative}"
        )));
    }
    let (kind, data, target) = if metadata.file_type().is_file() {
        (FileKind::Regular, fs::read(&path)?, None)
    } else if metadata.file_type().is_dir() {
        (FileKind::Directory, Vec::new(), None)
    } else if metadata.file_type().is_symlink() {
        let target = fs::read_link(&path)?
            .to_str()
            .ok_or_else(|| {
                PackageError::invalid(format!("symlink target is not UTF-8: /{relative}"))
            })?
            .to_owned();
        if target.contains('\0') {
            return Err(PackageError::invalid(format!(
                "symlink target contains NUL: /{relative}"
            )));
        }
        (FileKind::Symlink, Vec::new(), Some(target))
    } else {
        return Err(PackageError::invalid(format!(
            "MVP rejects special file /{relative}"
        )));
    };
    let logical = target.as_deref().map_or(data.as_slice(), str::as_bytes);
    let inode =
        (kind == FileKind::Regular).then(|| (metadata.dev(), metadata.ino(), metadata.nlink()));
    Ok(CapturedPath {
        entry: PayloadEntry {
            record: FileRecord {
                kind,
                mode,
                uid: metadata.uid(),
                gid: metadata.gid(),
                size: logical.len() as u64,
                sha256: sha256(logical),
                path: relative.to_owned(),
                link_target: target,
            },
            data,
        },
        inode,
    })
}

fn canonicalize_hardlinks(captured: &mut [CapturedPath]) -> Result<()> {
    let mut groups: BTreeMap<(u64, u64), Vec<usize>> = BTreeMap::new();
    for (index, path) in captured.iter().enumerate() {
        if let Some((device, inode, _)) = path.inode {
            groups.entry((device, inode)).or_default().push(index);
        }
    }
    for indexes in groups.values() {
        let expected_links = captured[indexes[0]].inode.unwrap().2;
        if indexes
            .iter()
            .any(|index| captured[*index].inode.unwrap().2 != expected_links)
        {
            return Err(PackageError::invalid(format!(
                "hardlink group changed while capturing /{}",
                captured[indexes[0]].entry.record.path
            )));
        }
        if expected_links != indexes.len() as u64 {
            return Err(PackageError::invalid(format!(
                "hardlink group for /{} has {expected_links} filesystem links but only {} captured paths",
                captured[indexes[0]].entry.record.path,
                indexes.len()
            )));
        }
        if indexes.len() == 1 {
            continue;
        }
        let canonical_index = indexes[0];
        let canonical = captured[canonical_index].entry.record.clone();
        for index in indexes.iter().copied().skip(1) {
            let alias = &mut captured[index].entry;
            if alias.record.mode != canonical.mode
                || alias.record.uid != canonical.uid
                || alias.record.gid != canonical.gid
                || alias.record.size != canonical.size
                || alias.record.sha256 != canonical.sha256
            {
                return Err(PackageError::invalid(format!(
                    "hardlink group changed while capturing /{}",
                    alias.record.path
                )));
            }
            alias.record.kind = FileKind::Hardlink;
            alias.record.link_target = Some(canonical.path.clone());
            alias.data.clear();
        }
    }
    Ok(())
}

fn read_vdb(vdb: &Path, name: &str) -> Option<String> {
    fs::read_to_string(vdb.join(name))
        .ok()
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
}

fn reject_symlink_root(root: &Path) -> Result<()> {
    let metadata = fs::symlink_metadata(root)?;
    if !metadata.file_type().is_dir() || metadata.file_type().is_symlink() {
        return Err(PackageError::invalid(
            "reference root must be a real directory",
        ));
    }
    Ok(())
}

fn reject_symlink_path(root: &Path, path: &Path) -> Result<()> {
    reject_symlink_parents(root, path)?;
    if fs::symlink_metadata(path)?.file_type().is_symlink() {
        return Err(PackageError::invalid(format!(
            "VDB tree contains symlink {}",
            path.display()
        )));
    }
    Ok(())
}

fn reject_symlink_parents(root: &Path, path: &Path) -> Result<()> {
    let relative = path
        .strip_prefix(root)
        .map_err(|_| PackageError::invalid("path escaped reference root"))?;
    let mut current = PathBuf::from(root);
    let count = relative.components().count();
    for component in relative.components().take(count.saturating_sub(1)) {
        current.push(component);
        if fs::symlink_metadata(&current)?.file_type().is_symlink() {
            return Err(PackageError::invalid(format!(
                "path traverses symlink {}",
                current.display()
            )));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn parses_vdb_contents_paths() {
        assert_eq!(
            contents_path("obj /usr/bin/wl-copy deadbeef 1").unwrap(),
            "usr/bin/wl-copy"
        );
        assert_eq!(
            contents_path("sym /usr/bin/x -> ../lib/x 1").unwrap(),
            "usr/bin/x"
        );
        assert_eq!(
            contents_path("dir /usr/share/doc").unwrap(),
            "usr/share/doc"
        );
        assert!(contents_path("obj /usr/../etc/x deadbeef 1").is_err());
    }

    #[test]
    fn reconstructs_shared_ownership_from_portage_vdb() {
        let root = TempDir::new().unwrap();
        for (package, contents) in [
            (
                "app-one/first-1",
                "obj /usr/bin/shared deadbeef 1\ndir /usr/share/shared\n",
            ),
            (
                "app-two/second-2",
                "sym /usr/bin/shared -> target 1\ndir /usr/share/shared\n",
            ),
        ] {
            let vdb = root.path().join("var/db/pkg").join(package);
            fs::create_dir_all(&vdb).unwrap();
            fs::write(vdb.join("CONTENTS"), contents).unwrap();
        }

        let ownership = load_ownership(root.path(), None).unwrap();
        assert_eq!(
            ownership.owners("usr/bin/shared"),
            &["app-one/first-1".to_owned(), "app-two/second-2".to_owned()]
        );
        assert_eq!(
            ownership.owners("usr/share/shared"),
            &["app-one/first-1".to_owned(), "app-two/second-2".to_owned()]
        );
        assert!(ownership.owners("usr/bin/missing").is_empty());

        let without_first = load_ownership(root.path(), Some("app-one/first-1")).unwrap();
        assert_eq!(
            without_first.owners("usr/bin/shared"),
            &["app-two/second-2".to_owned()]
        );
    }

    #[test]
    fn ownership_scan_fails_closed_for_incomplete_vdb_entries() {
        let root = TempDir::new().unwrap();
        fs::create_dir_all(root.path().join("var/db/pkg/app-one/first-1")).unwrap();
        let error = load_ownership(root.path(), None).err().unwrap().to_string();
        assert!(error.contains("authoritative VDB ownership"));
        assert!(error.contains("CONTENTS"));
    }

    #[test]
    fn chooses_bytewise_lowest_hardlink_path_as_canonical() {
        let make = |path: &str| CapturedPath {
            entry: PayloadEntry {
                record: FileRecord {
                    kind: FileKind::Regular,
                    mode: 0o755,
                    uid: 0,
                    gid: 0,
                    size: 1,
                    sha256: sha256(b"x"),
                    path: path.into(),
                    link_target: None,
                },
                data: b"x".to_vec(),
            },
            inode: Some((1, 2, 2)),
        };
        let mut captured = vec![make("usr/bin/z"), make("usr/bin/A")];
        captured.sort_by(|left, right| {
            left.entry
                .record
                .path
                .as_bytes()
                .cmp(right.entry.record.path.as_bytes())
        });
        canonicalize_hardlinks(&mut captured).unwrap();
        assert_eq!(captured[0].entry.record.path, "usr/bin/A");
        assert_eq!(captured[0].entry.record.kind, FileKind::Regular);
        assert_eq!(captured[1].entry.record.kind, FileKind::Hardlink);
        assert_eq!(
            captured[1].entry.record.link_target.as_deref(),
            Some("usr/bin/A")
        );
    }
}
