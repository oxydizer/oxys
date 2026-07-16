use std::{
    fs,
    fs::File,
    io::{Read, Write},
    os::fd::OwnedFd,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

use rustix::fs::{
    self as rfs, AtFlags, FileType, FlockOperation, Gid, Mode, OFlags, RenameFlags, Timestamps,
    UTIME_OMIT, Uid,
};
use rustix::time::Timespec;

use super::{
    FileKind, FileRecord, PackageError, Result,
    format::{PayloadEntry, hex, sha256, validate_relative_path},
};

pub(crate) struct SafeRoot {
    fd: OwnedFd,
    path: PathBuf,
}

static TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Presence {
    Missing,
    Matches,
}

impl SafeRoot {
    pub(crate) fn open(root: &Path) -> Result<Self> {
        let metadata = fs::symlink_metadata(root).map_err(|error| {
            PackageError::invalid(format!(
                "cannot inspect target root {}: {error}",
                root.display()
            ))
        })?;
        if !metadata.file_type().is_dir() || metadata.file_type().is_symlink() {
            return Err(PackageError::invalid(format!(
                "target root must be a real directory: {}",
                root.display()
            )));
        }
        let fd = rfs::open(
            root,
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .map_err(errno)?;
        Ok(Self {
            fd,
            path: root.to_owned(),
        })
    }

    pub(crate) fn preflight_hardlinks(&self, records: &[FileRecord]) -> Result<()> {
        for record in records
            .iter()
            .filter(|record| record.kind == FileKind::Hardlink)
        {
            if self.inspect(record)? == Presence::Matches {
                continue;
            }
            let target = record.link_target.as_deref().ok_or_else(|| {
                PackageError::invalid(format!("hardlink has no target: {}", record.path))
            })?;
            let target_parent = self.open_nearest_existing_parent(target)?;
            let alias_parent = self.open_nearest_existing_parent(&record.path)?;
            let target_stat = rfs::fstat(&target_parent).map_err(errno)?;
            let alias_stat = rfs::fstat(&alias_parent).map_err(errno)?;
            if target_stat.st_dev != alias_stat.st_dev {
                return Err(self.hardlink_error(record, rustix::io::Errno::XDEV));
            }
            self.probe_hardlink_support(&target_parent, &alias_parent, record)?;
        }
        Ok(())
    }

    pub(crate) fn inspect(&self, record: &FileRecord) -> Result<Presence> {
        validate_relative_path(&record.path)?;
        let (parent, name) = match self.open_parent(&record.path, false) {
            Ok(value) => value,
            Err(PackageError::Io(error)) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(Presence::Missing);
            }
            Err(error) => return Err(error),
        };
        let stat = match rfs::statat(&parent, name, AtFlags::SYMLINK_NOFOLLOW) {
            Ok(stat) => stat,
            Err(error) if error == rustix::io::Errno::NOENT => return Ok(Presence::Missing),
            Err(error) => return Err(errno(error).into()),
        };
        let actual_kind = file_kind(stat.st_mode).ok_or_else(|| {
            PackageError::invalid(format!("unsupported installed object at {}", record.path))
        })?;
        let expected_kind = if record.kind == FileKind::Hardlink {
            FileKind::Regular
        } else {
            record.kind
        };
        if actual_kind != expected_kind {
            return Err(PackageError::invalid(format!(
                "installed type mismatch at {}",
                record.path
            )));
        }
        if stat.st_uid != record.uid
            || stat.st_gid != record.gid
            || Mode::from_raw_mode(stat.st_mode).as_raw_mode() & 0o7777 != record.mode & 0o7777
        {
            return Err(PackageError::invalid(format!(
                "installed ownership/mode mismatch at {}",
                record.path
            )));
        }
        let actual_hash = match record.kind {
            FileKind::Directory => sha256(&[]),
            FileKind::Regular | FileKind::Hardlink => {
                let fd = rfs::openat(
                    &parent,
                    name,
                    OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                    Mode::empty(),
                )
                .map_err(errno)?;
                let mut file = File::from(fd);
                let mut digest = sha2::Sha256::new();
                let mut buffer = [0_u8; 64 * 1024];
                loop {
                    let read = file.read(&mut buffer)?;
                    if read == 0 {
                        break;
                    }
                    use sha2::Digest;
                    digest.update(&buffer[..read]);
                }
                use sha2::Digest;
                digest.finalize().into()
            }
            FileKind::Symlink => {
                let target = rfs::readlinkat(&parent, name, Vec::new()).map_err(errno)?;
                sha256(target.to_bytes())
            }
        };
        if actual_hash != record.sha256 {
            return Err(PackageError::invalid(format!(
                "installed SHA-256 mismatch at {} (expected {}, got {})",
                record.path,
                hex(&record.sha256),
                hex(&actual_hash)
            )));
        }
        if record.kind == FileKind::Hardlink {
            let target = record.link_target.as_deref().ok_or_else(|| {
                PackageError::invalid(format!("hardlink has no target: {}", record.path))
            })?;
            let (target_parent, target_name) = self.open_parent(target, false)?;
            let target_stat = rfs::statat(&target_parent, target_name, AtFlags::SYMLINK_NOFOLLOW)
                .map_err(errno)?;
            if file_kind(target_stat.st_mode) != Some(FileKind::Regular)
                || stat.st_dev != target_stat.st_dev
                || stat.st_ino != target_stat.st_ino
            {
                return Err(PackageError::invalid(format!(
                    "installed hardlink identity mismatch at {} -> {}",
                    record.path, target
                )));
            }
        }
        Ok(Presence::Matches)
    }

    /// Report whether a leaf exists without accepting an unsafe parent path.
    ///
    /// Config protection uses this only after strict inspection reports that
    /// an expected object differs. Keeping the traversal descriptor-relative
    /// ensures a symlinked parent is still an error, not a reason to classify
    /// an arbitrary outside file as a local configuration change.
    pub(crate) fn path_exists(&self, path: &str) -> Result<bool> {
        validate_relative_path(path)?;
        let (parent, name) = match self.open_parent(path, false) {
            Ok(value) => value,
            Err(PackageError::Io(error)) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(false);
            }
            Err(error) => return Err(error),
        };
        match rfs::statat(&parent, name, AtFlags::SYMLINK_NOFOLLOW) {
            Ok(_) => Ok(true),
            Err(error) if error == rustix::io::Errno::NOENT => Ok(false),
            Err(error) => Err(errno(error).into()),
        }
    }

    pub(crate) fn open_regular(&self, path: &str) -> Result<File> {
        let (parent, name) = self.open_parent(path, false)?;
        let fd = rfs::openat(
            &parent,
            name,
            OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .map_err(errno)?;
        let stat = rfs::fstat(&fd).map_err(errno)?;
        if FileType::from_raw_mode(stat.st_mode) != FileType::RegularFile {
            return Err(PackageError::invalid(format!(
                "control path is not a regular file: {path}"
            )));
        }
        Ok(File::from(fd))
    }

    pub(crate) fn write_control(&self, path: &str, contents: &[u8], mode: u32) -> Result<()> {
        self.write_control_from(path, mode, |file| file.write_all(contents))
    }

    fn write_control_from(
        &self,
        path: &str,
        mode: u32,
        write: impl FnOnce(&mut File) -> std::io::Result<()>,
    ) -> Result<()> {
        let (parent, name) = self.open_parent(path, true)?;
        if let Ok(stat) = rfs::statat(&parent, name, AtFlags::SYMLINK_NOFOLLOW)
            && FileType::from_raw_mode(stat.st_mode) != FileType::RegularFile
        {
            return Err(PackageError::invalid(format!(
                "refusing to replace non-file control path {path}"
            )));
        }
        let temporary = format!(
            ".{name}.oxys-tmp-{}-{}",
            std::process::id(),
            TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed)
        );
        let fd = rfs::openat(
            &parent,
            temporary.as_str(),
            OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::from_raw_mode(mode),
        )
        .map_err(errno)?;
        let mut file = File::from(fd);
        let result = (|| {
            write(&mut file)?;
            rfs::fchmod(&file, Mode::from_raw_mode(mode)).map_err(errno)?;
            file.sync_all()?;
            rfs::renameat(&parent, temporary.as_str(), &parent, name).map_err(errno)?;
            File::from(rustix::io::dup(&parent).map_err(errno)?).sync_all()?;
            Ok(())
        })();
        if result.is_err() {
            let _ = rfs::unlinkat(&parent, temporary.as_str(), AtFlags::empty());
        }
        result
    }

    pub(crate) fn copy_control(&self, path: &str, source: &Path, mode: u32) -> Result<()> {
        let mut input = File::open(source)?;
        self.write_control_from(path, mode, |output| {
            std::io::copy(&mut input, output).map(|_| ())
        })
    }

    pub(crate) fn lock(&self, path: &str) -> Result<File> {
        let (parent, name) = self.open_parent(path, true)?;
        let fd = rfs::openat(
            &parent,
            name,
            OFlags::RDWR | OFlags::CREATE | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::from_raw_mode(0o600),
        )
        .map_err(errno)?;
        let file = File::from(fd);
        rfs::flock(&file, FlockOperation::LockExclusive).map_err(errno)?;
        Ok(file)
    }

    pub(crate) fn lock_portage(&self, path: &str) -> Result<File> {
        let (parent, name) = self.open_parent(path, true)?;
        let lock_name = format!(".{name}.portage_lockfile");
        loop {
            let fd = rfs::openat(
                &parent,
                lock_name.as_str(),
                OFlags::RDWR | OFlags::CREATE | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                Mode::from_raw_mode(0o660),
            )
            .map_err(errno)?;
            let stat = rfs::fstat(&fd).map_err(errno)?;
            if FileType::from_raw_mode(stat.st_mode) != FileType::RegularFile {
                return Err(PackageError::invalid(format!(
                    "Portage lock path is not a regular file: {lock_name}"
                )));
            }
            let file = File::from(fd);
            rfs::fcntl_lock(&file, FlockOperation::LockExclusive).map_err(errno)?;

            // Portage removes wantnewlockfile lock paths when it releases
            // them. If we opened the old inode while waiting, another process
            // can create and lock a new inode at the same path. Match
            // Portage's retry rule by accepting the lock only while the path
            // still names our locked inode.
            match rfs::statat(&parent, lock_name.as_str(), AtFlags::SYMLINK_NOFOLLOW) {
                Ok(current) if current.st_dev == stat.st_dev && current.st_ino == stat.st_ino => {
                    return Ok(file);
                }
                Ok(_) => continue,
                Err(error) if error == rustix::io::Errno::NOENT => continue,
                Err(error) => return Err(errno(error).into()),
            }
        }
    }

    pub(crate) fn remove_control(&self, path: &str) -> Result<()> {
        let (parent, name) = self.open_parent(path, false)?;
        rfs::unlinkat(&parent, name, AtFlags::empty()).map_err(errno)?;
        Ok(())
    }

    pub(crate) fn install_entry(&self, entry: &PayloadEntry) -> Result<()> {
        if self.inspect(&entry.record)? == Presence::Matches {
            return Ok(());
        }
        let record = &entry.record;
        let (parent, name) = self.open_parent(&record.path, true)?;
        match record.kind {
            FileKind::Directory => {
                rfs::mkdirat(&parent, name, Mode::from_raw_mode(0o755)).map_err(errno)?;
                let fd = rfs::openat(
                    &parent,
                    name,
                    OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                    Mode::empty(),
                )
                .map_err(errno)?;
                if let Err(error) = self.set_fd_metadata(&fd, record) {
                    let _ = rfs::unlinkat(&parent, name, AtFlags::REMOVEDIR);
                    return Err(error);
                }
            }
            FileKind::Regular => {
                let temporary = temporary_name(name);
                let fd = rfs::openat(
                    &parent,
                    temporary.as_str(),
                    OFlags::WRONLY
                        | OFlags::CREATE
                        | OFlags::EXCL
                        | OFlags::NOFOLLOW
                        | OFlags::CLOEXEC,
                    Mode::from_raw_mode(record.mode),
                )
                .map_err(errno)?;
                let mut file = File::from(fd);
                let result: Result<()> = (|| {
                    file.write_all(&entry.data)?;
                    self.set_fd_metadata(&file, record)?;
                    file.sync_all()?;
                    rfs::renameat_with(
                        &parent,
                        temporary.as_str(),
                        &parent,
                        name,
                        RenameFlags::NOREPLACE,
                    )
                    .map_err(errno)?;
                    Ok(())
                })();
                if result.is_err() {
                    let _ = rfs::unlinkat(&parent, temporary.as_str(), AtFlags::empty());
                }
                result?;
            }
            FileKind::Symlink => {
                let temporary = temporary_name(name);
                rfs::symlinkat(
                    record.link_target.as_deref().unwrap_or_default(),
                    &parent,
                    temporary.as_str(),
                )
                .map_err(errno)?;
                let result: Result<()> = (|| {
                    let stat = rfs::statat(&parent, temporary.as_str(), AtFlags::SYMLINK_NOFOLLOW)
                        .map_err(errno)?;
                    if stat.st_uid != record.uid || stat.st_gid != record.gid {
                        rfs::chownat(
                            &parent,
                            temporary.as_str(),
                            Some(Uid::from_raw(record.uid)),
                            Some(Gid::from_raw(record.gid)),
                            AtFlags::SYMLINK_NOFOLLOW,
                        )
                        .map_err(errno)?;
                    }
                    rfs::renameat_with(
                        &parent,
                        temporary.as_str(),
                        &parent,
                        name,
                        RenameFlags::NOREPLACE,
                    )
                    .map_err(errno)?;
                    Ok(())
                })();
                if result.is_err() {
                    let _ = rfs::unlinkat(&parent, temporary.as_str(), AtFlags::empty());
                }
                result?;
            }
            FileKind::Hardlink => {
                let target = record.link_target.as_deref().ok_or_else(|| {
                    PackageError::invalid(format!("hardlink has no target: {}", record.path))
                })?;
                let canonical = FileRecord {
                    kind: FileKind::Regular,
                    path: target.to_owned(),
                    link_target: None,
                    ..record.clone()
                };
                if self.inspect(&canonical)? != Presence::Matches {
                    return Err(PackageError::invalid(format!(
                        "hardlink target is missing: {target}"
                    )));
                }
                let (target_parent, target_name) = self.open_parent(target, false)?;
                let temporary = temporary_name(name);
                rfs::linkat(
                    &target_parent,
                    target_name,
                    &parent,
                    temporary.as_str(),
                    AtFlags::empty(),
                )
                .map_err(|error| self.hardlink_error(record, error))?;
                let result: Result<()> = (|| {
                    let target_stat =
                        rfs::statat(&target_parent, target_name, AtFlags::SYMLINK_NOFOLLOW)
                            .map_err(errno)?;
                    let alias_stat =
                        rfs::statat(&parent, temporary.as_str(), AtFlags::SYMLINK_NOFOLLOW)
                            .map_err(errno)?;
                    if target_stat.st_dev != alias_stat.st_dev
                        || target_stat.st_ino != alias_stat.st_ino
                    {
                        return Err(PackageError::invalid(format!(
                            "target filesystem did not preserve hardlink identity for {} -> {}",
                            record.path, target
                        )));
                    }
                    rfs::renameat_with(
                        &parent,
                        temporary.as_str(),
                        &parent,
                        name,
                        RenameFlags::NOREPLACE,
                    )
                    .map_err(errno)?;
                    Ok(())
                })();
                if result.is_err() {
                    let _ = rfs::unlinkat(&parent, temporary.as_str(), AtFlags::empty());
                }
                result?;
            }
        }
        Ok(())
    }

    pub(crate) fn finish_directory(&self, record: &FileRecord) -> Result<()> {
        let (parent, name) = self.open_parent(&record.path, false)?;
        let fd = rfs::openat(
            &parent,
            name,
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .map_err(errno)?;
        self.set_fd_metadata(&fd, record)
    }

    pub(crate) fn set_mtime(&self, record: &FileRecord, seconds: i64) -> Result<()> {
        let times = Timestamps {
            last_access: Timespec {
                tv_sec: 0,
                tv_nsec: UTIME_OMIT,
            },
            last_modification: Timespec {
                tv_sec: seconds,
                tv_nsec: 0,
            },
        };
        let (parent, name) = self.open_parent(&record.path, false)?;
        match record.kind {
            FileKind::Regular | FileKind::Hardlink => {
                let fd = rfs::openat(
                    &parent,
                    name,
                    OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                    Mode::empty(),
                )
                .map_err(errno)?;
                rfs::futimens(&fd, &times).map_err(errno)?;
            }
            FileKind::Symlink => {
                rfs::utimensat(&parent, name, &times, AtFlags::SYMLINK_NOFOLLOW).map_err(errno)?;
            }
            FileKind::Directory => {}
        }
        Ok(())
    }

    pub(crate) fn remove_verified(&self, record: &FileRecord, missing_ok: bool) -> Result<()> {
        match self.inspect(record)? {
            Presence::Matches => {}
            Presence::Missing if missing_ok => return Ok(()),
            Presence::Missing => {
                return Err(PackageError::invalid(format!(
                    "installed path is missing: {}",
                    record.path
                )));
            }
        }
        let (parent, name) = self.open_parent(&record.path, false)?;
        let parent_stat = rfs::fstat(&parent).map_err(errno)?;
        if parent_stat.st_uid != rustix::process::geteuid().as_raw()
            || Mode::from_raw_mode(parent_stat.st_mode).as_raw_mode() & 0o022 != 0
        {
            return Err(PackageError::invalid(format!(
                "refusing removal through an untrusted writable parent of {}",
                record.path
            )));
        }
        // Recheck the leaf through the same parent descriptor immediately
        // before unlinking. The ownership/mode check above prevents an
        // untrusted process from swapping it between verification and unlink.
        let stat = rfs::statat(&parent, name, AtFlags::SYMLINK_NOFOLLOW).map_err(errno)?;
        let expected_kind = if record.kind == FileKind::Hardlink {
            FileKind::Regular
        } else {
            record.kind
        };
        if file_kind(stat.st_mode) != Some(expected_kind) {
            return Err(PackageError::invalid(format!(
                "installed type changed before removal at {}",
                record.path
            )));
        }
        let flags = if record.kind == FileKind::Directory {
            AtFlags::REMOVEDIR
        } else {
            AtFlags::empty()
        };
        match rfs::unlinkat(&parent, name, flags) {
            Ok(()) => Ok(()),
            Err(error) if missing_ok && error == rustix::io::Errno::NOENT => Ok(()),
            Err(error)
                if record.kind == FileKind::Directory && error == rustix::io::Errno::NOTEMPTY =>
            {
                Ok(())
            }
            Err(error) => Err(errno(error).into()),
        }
    }

    fn set_fd_metadata(&self, fd: &impl std::os::fd::AsFd, record: &FileRecord) -> Result<()> {
        let stat = rfs::fstat(fd).map_err(errno)?;
        if stat.st_uid != record.uid || stat.st_gid != record.gid {
            rfs::fchown(
                fd,
                Some(Uid::from_raw(record.uid)),
                Some(Gid::from_raw(record.gid)),
            )
            .map_err(errno)?;
        }
        rfs::fchmod(fd, Mode::from_raw_mode(record.mode)).map_err(errno)?;
        Ok(())
    }

    fn open_nearest_existing_parent(&self, path: &str) -> Result<OwnedFd> {
        validate_relative_path(path)?;
        let mut parts = path.split('/').peekable();
        parts.next_back();
        let mut current = rustix::io::dup(&self.fd).map_err(errno)?;
        for component in parts {
            let flags = OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC;
            match rfs::openat(&current, component, flags, Mode::empty()) {
                Ok(next) => current = next,
                Err(error) if error == rustix::io::Errno::NOENT => break,
                Err(error) => return Err(errno(error).into()),
            }
        }
        Ok(current)
    }

    fn probe_hardlink_support(
        &self,
        target_parent: &OwnedFd,
        alias_parent: &OwnedFd,
        record: &FileRecord,
    ) -> Result<()> {
        let source = temporary_name("hardlink-probe-source");
        let alias = temporary_name("hardlink-probe-alias");
        let source_fd = rfs::openat(
            target_parent,
            source.as_str(),
            OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::from_raw_mode(0o600),
        )
        .map_err(|error| self.hardlink_error(record, error))?;
        drop(source_fd);
        let result: Result<()> = (|| {
            rfs::linkat(
                target_parent,
                source.as_str(),
                alias_parent,
                alias.as_str(),
                AtFlags::empty(),
            )
            .map_err(|error| self.hardlink_error(record, error))?;
            let source_stat =
                rfs::statat(target_parent, source.as_str(), AtFlags::SYMLINK_NOFOLLOW)
                    .map_err(errno)?;
            let alias_stat = rfs::statat(alias_parent, alias.as_str(), AtFlags::SYMLINK_NOFOLLOW)
                .map_err(errno)?;
            if source_stat.st_dev != alias_stat.st_dev || source_stat.st_ino != alias_stat.st_ino {
                return Err(PackageError::invalid(format!(
                    "target filesystem under {} did not preserve hardlink identity for {} -> {}; .oxys will not substitute a copy",
                    self.path.display(),
                    record.path,
                    record.link_target.as_deref().unwrap_or("<missing>")
                )));
            }
            Ok(())
        })();
        let _ = rfs::unlinkat(alias_parent, alias.as_str(), AtFlags::empty());
        let _ = rfs::unlinkat(target_parent, source.as_str(), AtFlags::empty());
        result
    }

    fn hardlink_error(&self, record: &FileRecord, error: rustix::io::Errno) -> PackageError {
        let target = record.link_target.as_deref().unwrap_or("<missing>");
        let reason = if error == rustix::io::Errno::NOTSUP || error == rustix::io::Errno::OPNOTSUPP
        {
            "target filesystem does not support hardlinks"
        } else if error == rustix::io::Errno::XDEV {
            "hardlink paths cross target filesystems"
        } else if error == rustix::io::Errno::PERM || error == rustix::io::Errno::ACCESS {
            "target filesystem or security policy denies hardlinks"
        } else {
            "target filesystem could not create the hardlink"
        };
        PackageError::invalid(format!(
            "cannot install hardlink {} -> {} under {}: {reason} ({error}); .oxys will not substitute a copy",
            record.path,
            target,
            self.path.display()
        ))
    }

    fn open_parent<'a>(&self, path: &'a str, create: bool) -> Result<(OwnedFd, &'a str)> {
        validate_relative_path(path)?;
        let mut parts = path.split('/').peekable();
        let name = parts.next_back().unwrap();
        let mut current = rustix::io::dup(&self.fd).map_err(errno)?;
        for component in parts {
            let flags = OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC;
            match rfs::openat(&current, component, flags, Mode::empty()) {
                Ok(next) => current = next,
                Err(error) if create && error == rustix::io::Errno::NOENT => {
                    match rfs::mkdirat(&current, component, Mode::from_raw_mode(0o755)) {
                        Ok(()) => {}
                        Err(error) if error == rustix::io::Errno::EXIST => {}
                        Err(error) => return Err(errno(error).into()),
                    }
                    current =
                        rfs::openat(&current, component, flags, Mode::empty()).map_err(errno)?;
                }
                Err(error) => return Err(errno(error).into()),
            }
        }
        Ok((current, name))
    }
}

fn temporary_name(name: &str) -> String {
    format!(
        ".{name}.oxys-install-{}-{}",
        std::process::id(),
        TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed)
    )
}

fn file_kind(mode: u32) -> Option<FileKind> {
    match FileType::from_raw_mode(mode) {
        FileType::RegularFile => Some(FileKind::Regular),
        FileType::Directory => Some(FileKind::Directory),
        FileType::Symlink => Some(FileKind::Symlink),
        _ => None,
    }
}

fn errno(error: rustix::io::Errno) -> std::io::Error {
    std::io::Error::from_raw_os_error(error.raw_os_error())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn hardlink_record() -> FileRecord {
        FileRecord {
            kind: FileKind::Hardlink,
            mode: 0o755,
            uid: 0,
            gid: 0,
            size: 1,
            sha256: sha256(b"x"),
            path: "usr/bin/b".into(),
            link_target: Some("usr/bin/a".into()),
        }
    }

    #[test]
    fn hardlink_failures_are_explicit_and_never_offer_copy_fallback() {
        let root_dir = TempDir::new().unwrap();
        let root = SafeRoot::open(root_dir.path()).unwrap();
        let unsupported = root
            .hardlink_error(&hardlink_record(), rustix::io::Errno::NOTSUP)
            .to_string();
        assert!(unsupported.contains("does not support hardlinks"));
        assert!(unsupported.contains("will not substitute a copy"));

        let cross_device = root
            .hardlink_error(&hardlink_record(), rustix::io::Errno::XDEV)
            .to_string();
        assert!(cross_device.contains("cross target filesystems"));
    }
}
