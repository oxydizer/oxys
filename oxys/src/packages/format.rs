use std::{
    collections::BTreeMap,
    fs::{self, File},
    io::{self, Read, Seek, SeekFrom, Write},
    path::Path,
    process::{Command, Stdio},
};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;

use super::parallel;

const MAGIC: &[u8; 8] = b"OXYS\0PKG";
const HEADER_SIZE: usize = 40;
const FORMAT_MAJOR: u16 = 1;
const FORMAT_MINOR: u16 = 0;
const FLAG_HARDLINKS: u32 = 1 << 0;
const KNOWN_FLAGS: u32 = FLAG_HARDLINKS;
const MAX_METADATA: u64 = 1024 * 1024;
const MAX_FILE_TABLE: u64 = 16 * 1024 * 1024;
const MAX_COMPRESSED_PAYLOAD: u64 = 64 * 1024 * 1024;
const MAX_UNCOMPRESSED_PAYLOAD: u64 = 256 * 1024 * 1024;
const MAX_FILES: u64 = 100_000;
pub(crate) const MAX_ARTIFACT_SIZE: u64 =
    HEADER_SIZE as u64 + MAX_METADATA + MAX_FILE_TABLE + MAX_COMPRESSED_PAYLOAD;
const EMPTY_SHA256: [u8; 32] = [
    0xe3, 0xb0, 0xc4, 0x42, 0x98, 0xfc, 0x1c, 0x14, 0x9a, 0xfb, 0xf4, 0xc8, 0x99, 0x6f, 0xb9, 0x24,
    0x27, 0xae, 0x41, 0xe4, 0x64, 0x9b, 0x93, 0x4c, 0xa4, 0x95, 0x99, 0x1b, 0x78, 0x52, 0xb8, 0x55,
];

#[derive(Debug, Error)]
pub enum PackageError {
    #[error("{0}")]
    Invalid(String),
    #[error("I/O error: {0}")]
    Io(#[from] io::Error),
    #[error("invalid metadata TOML: {0}")]
    TomlDecode(#[from] toml::de::Error),
    #[error("cannot encode metadata TOML: {0}")]
    TomlEncode(#[from] toml::ser::Error),
}

impl PackageError {
    pub(crate) fn invalid(message: impl Into<String>) -> Self {
        Self::Invalid(message.into())
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Metadata {
    pub format: u16,
    pub kind: String,
    pub namespace: String,
    pub name: String,
    pub version: String,
    pub version_scheme: String,
    pub revision: u32,
    pub slot: String,
    pub build_id: String,
    pub target: TargetMetadata,
    pub payload: PayloadMetadata,
    pub build: BuildMetadata,
    pub portage: PortageMetadata,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct TargetMetadata {
    pub triple: String,
    pub cpu: String,
    pub libc: String,
    pub libc_min: String,
    pub init: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct PayloadMetadata {
    pub compression: String,
    pub uncompressed_size: u64,
    pub file_count: u64,
    pub sha256: String,
    pub file_table_sha256: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct BuildMetadata {
    pub builder: String,
    pub method: String,
    pub repo: String,
    pub repo_commit: String,
    pub ebuild: String,
    pub profile: String,
    pub use_flags: Vec<String>,
    pub compiler: String,
    pub cflags: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct PortageMetadata {
    pub category: String,
    pub pf: String,
    pub repository: String,
    pub vdb_payload: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum FileKind {
    Regular,
    Directory,
    Symlink,
    Hardlink,
}

impl FileKind {
    fn code(self) -> u8 {
        match self {
            Self::Regular => 1,
            Self::Directory => 2,
            Self::Symlink => 3,
            Self::Hardlink => 4,
        }
    }

    fn from_code(code: u8) -> Result<Self, PackageError> {
        match code {
            1 => Ok(Self::Regular),
            2 => Ok(Self::Directory),
            3 => Ok(Self::Symlink),
            4 => Ok(Self::Hardlink),
            _ => Err(PackageError::invalid(format!(
                "unknown file-table type {code}"
            ))),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileRecord {
    pub kind: FileKind,
    pub mode: u32,
    pub uid: u32,
    pub gid: u32,
    pub size: u64,
    pub sha256: [u8; 32],
    pub path: String,
    /// Symlink text or the package-relative canonical path for a hardlink.
    pub link_target: Option<String>,
}

#[derive(Clone, Debug)]
pub(crate) struct PayloadEntry {
    pub record: FileRecord,
    pub data: Vec<u8>,
}

#[derive(Debug)]
pub struct Artifact {
    pub metadata: Metadata,
    pub files: Vec<FileRecord>,
    pub(crate) entries: Vec<PayloadEntry>,
}

pub(crate) fn sha256(bytes: &[u8]) -> [u8; 32] {
    Sha256::digest(bytes).into()
}

pub(crate) fn hex(hash: &[u8; 32]) -> String {
    hash.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn prefixed_hash(hash: &[u8; 32]) -> String {
    format!("sha256:{}", hex(hash))
}

pub(crate) fn parse_prefixed_hash(value: &str, field: &str) -> Result<[u8; 32], PackageError> {
    let raw = value
        .strip_prefix("sha256:")
        .ok_or_else(|| PackageError::invalid(format!("{field} must start with sha256:")))?;
    if raw.len() != 64 || !raw.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(PackageError::invalid(format!(
            "{field} is not a SHA-256 digest"
        )));
    }
    let mut result = [0_u8; 32];
    for (index, byte) in result.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&raw[index * 2..index * 2 + 2], 16)
            .map_err(|_| PackageError::invalid(format!("{field} is not a SHA-256 digest")))?;
    }
    Ok(result)
}

pub(crate) fn sha256_file(path: &Path) -> Result<[u8; 32], PackageError> {
    sha256_reader(File::open(path)?)
}

pub(crate) fn sha256_reader(mut file: File) -> Result<[u8; 32], PackageError> {
    let mut digest = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        digest.update(&buffer[..read]);
    }
    Ok(digest.finalize().into())
}

pub(crate) fn validate_relative_path(path: &str) -> Result<(), PackageError> {
    if path.is_empty()
        || path.starts_with('/')
        || path.ends_with('/')
        || path.contains('\0')
        || path
            .split('/')
            .any(|part| part.is_empty() || part == "." || part == "..")
    {
        return Err(PackageError::invalid(format!(
            "unsafe package path {path:?}"
        )));
    }
    Ok(())
}

fn validate_metadata(metadata: &Metadata) -> Result<(), PackageError> {
    if metadata.format != 1 || metadata.kind != "package" || metadata.namespace != "gentoo" {
        return Err(PackageError::invalid(
            "unsupported package metadata identity",
        ));
    }
    if metadata.payload.compression != "zstd" {
        return Err(PackageError::invalid("payload compression must be zstd"));
    }
    for (field, value) in [
        ("name", metadata.name.as_str()),
        ("version", metadata.version.as_str()),
        ("slot", metadata.slot.as_str()),
        ("target.triple", metadata.target.triple.as_str()),
        ("portage.category", metadata.portage.category.as_str()),
        ("portage.pf", metadata.portage.pf.as_str()),
    ] {
        if value.is_empty() || value.contains('\0') || value.contains('\n') {
            return Err(PackageError::invalid(format!(
                "metadata field {field} is invalid"
            )));
        }
    }
    if metadata.name != format!("{}/{}", metadata.portage.category, package_name(metadata)?) {
        return Err(PackageError::invalid(
            "metadata name/category/PF do not agree",
        ));
    }
    if !metadata.portage.vdb_payload {
        return Err(PackageError::invalid("Portage VDB payload is required"));
    }
    if metadata.payload.file_count > MAX_FILES
        || metadata.payload.uncompressed_size > MAX_UNCOMPRESSED_PAYLOAD
    {
        return Err(PackageError::invalid(
            "declared payload limits are too large",
        ));
    }
    parse_prefixed_hash(&metadata.payload.sha256, "payload.sha256")?;
    parse_prefixed_hash(
        &metadata.payload.file_table_sha256,
        "payload.file_table_sha256",
    )?;
    parse_prefixed_hash(&metadata.build_id, "build_id")?;
    validate_component(&metadata.portage.category, "category")?;
    validate_component(&metadata.portage.pf, "PF")?;
    Ok(())
}

fn validate_component(value: &str, field: &str) -> Result<(), PackageError> {
    if value == "." || value == ".." || value.contains('/') || value.contains('\0') {
        return Err(PackageError::invalid(format!(
            "invalid Portage {field} {value:?}"
        )));
    }
    Ok(())
}

fn package_name(metadata: &Metadata) -> Result<&str, PackageError> {
    let prefix = metadata
        .portage
        .pf
        .strip_suffix(&format!("-{}", metadata.version))
        .ok_or_else(|| PackageError::invalid("PF does not end in the declared version"))?;
    Ok(prefix)
}

pub(crate) fn write_artifact(
    output: &Path,
    mut metadata: Metadata,
    mut entries: Vec<PayloadEntry>,
) -> Result<Metadata, PackageError> {
    entries.sort_by(|left, right| {
        left.record
            .path
            .as_bytes()
            .cmp(right.record.path.as_bytes())
    });
    validate_entries(&entries, true)?;
    let table = encode_file_table(&entries)?;
    let tar = encode_tar(&entries)?;
    if tar.len() as u64 > MAX_UNCOMPRESSED_PAYLOAD {
        return Err(PackageError::invalid(
            "uncompressed payload exceeds format limit",
        ));
    }
    let compressed = run_zstd(&tar, false, MAX_COMPRESSED_PAYLOAD)?;
    metadata.payload = PayloadMetadata {
        compression: "zstd".into(),
        uncompressed_size: tar.len() as u64,
        file_count: entries.len() as u64,
        sha256: prefixed_hash(&sha256(&tar)),
        file_table_sha256: prefixed_hash(&sha256(&table)),
    };
    metadata.build_id = prefixed_hash(&sha256(&tar));
    validate_metadata(&metadata)?;
    let metadata_bytes = toml::to_string(&metadata)?.into_bytes();
    check_length("metadata", metadata_bytes.len() as u64, MAX_METADATA)?;
    check_length("file table", table.len() as u64, MAX_FILE_TABLE)?;
    check_length("payload", compressed.len() as u64, MAX_COMPRESSED_PAYLOAD)?;

    if let Some(parent) = output
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        fs::create_dir_all(parent)?;
    }
    let temp = output.with_extension("oxys.tmp");
    let mut file = File::create(&temp)?;
    file.write_all(MAGIC)?;
    file.write_all(&FORMAT_MAJOR.to_le_bytes())?;
    file.write_all(&FORMAT_MINOR.to_le_bytes())?;
    let flags = if entries
        .iter()
        .any(|entry| entry.record.kind == FileKind::Hardlink)
    {
        FLAG_HARDLINKS
    } else {
        0
    };
    file.write_all(&flags.to_le_bytes())?;
    file.write_all(&(metadata_bytes.len() as u64).to_le_bytes())?;
    file.write_all(&(table.len() as u64).to_le_bytes())?;
    file.write_all(&(compressed.len() as u64).to_le_bytes())?;
    file.write_all(&metadata_bytes)?;
    file.write_all(&table)?;
    file.write_all(&compressed)?;
    file.sync_all()?;
    fs::rename(temp, output)?;
    Ok(metadata)
}

pub(crate) fn read_artifact(path: &Path) -> Result<Artifact, PackageError> {
    read_artifact_file(File::open(path)?)
}

pub(crate) fn read_artifact_file(mut file: File) -> Result<Artifact, PackageError> {
    file.seek(SeekFrom::Start(0))?;
    let actual_len = file.metadata()?.len();
    let mut header = [0_u8; HEADER_SIZE];
    file.read_exact(&mut header)
        .map_err(|_| PackageError::invalid("truncated .oxys header"))?;
    if &header[..8] != MAGIC {
        return Err(PackageError::invalid("invalid .oxys magic"));
    }
    let major = u16::from_le_bytes(header[8..10].try_into().unwrap());
    let minor = u16::from_le_bytes(header[10..12].try_into().unwrap());
    let flags = u32::from_le_bytes(header[12..16].try_into().unwrap());
    if major != FORMAT_MAJOR || minor != FORMAT_MINOR || flags & !KNOWN_FLAGS != 0 {
        return Err(PackageError::invalid(format!(
            "unsupported .oxys version {major}.{minor} or flags 0x{flags:x}"
        )));
    }
    let metadata_len = u64::from_le_bytes(header[16..24].try_into().unwrap());
    let table_len = u64::from_le_bytes(header[24..32].try_into().unwrap());
    let payload_len = u64::from_le_bytes(header[32..40].try_into().unwrap());
    check_length("metadata", metadata_len, MAX_METADATA)?;
    check_length("file table", table_len, MAX_FILE_TABLE)?;
    check_length("payload", payload_len, MAX_COMPRESSED_PAYLOAD)?;
    let framed_len = 40_u64
        .checked_add(metadata_len)
        .and_then(|value| value.checked_add(table_len))
        .and_then(|value| value.checked_add(payload_len))
        .ok_or_else(|| PackageError::invalid("container lengths overflow"))?;
    if actual_len != framed_len {
        return Err(PackageError::invalid(
            "container lengths do not match file size",
        ));
    }
    let metadata_bytes = read_exact_vec(&mut file, metadata_len)?;
    let metadata_text = std::str::from_utf8(&metadata_bytes)
        .map_err(|_| PackageError::invalid("metadata is not UTF-8"))?;
    let metadata: Metadata = toml::from_str(metadata_text)?;
    validate_metadata(&metadata)?;
    let table = read_exact_vec(&mut file, table_len)?;
    if sha256(&table)
        != parse_prefixed_hash(&metadata.payload.file_table_sha256, "file table hash")?
    {
        return Err(PackageError::invalid("file table SHA-256 mismatch"));
    }
    let files = decode_file_table(&table)?;
    let has_hardlinks = files.iter().any(|record| record.kind == FileKind::Hardlink);
    if has_hardlinks != (flags & FLAG_HARDLINKS != 0) {
        return Err(PackageError::invalid(
            "container hardlink flag does not match its file table",
        ));
    }
    if files.len() as u64 != metadata.payload.file_count {
        return Err(PackageError::invalid(
            "file table count does not match metadata",
        ));
    }
    let compressed = read_exact_vec(&mut file, payload_len)?;
    let tar = run_zstd(
        &compressed,
        true,
        metadata
            .payload
            .uncompressed_size
            .min(MAX_UNCOMPRESSED_PAYLOAD),
    )?;
    if tar.len() as u64 != metadata.payload.uncompressed_size {
        return Err(PackageError::invalid(
            "decompressed payload length mismatch",
        ));
    }
    if sha256(&tar) != parse_prefixed_hash(&metadata.payload.sha256, "payload hash")? {
        return Err(PackageError::invalid("payload SHA-256 mismatch"));
    }
    let entries = decode_tar(&tar)?;
    validate_entries(&entries, false)?;
    if entries.len() != files.len() {
        return Err(PackageError::invalid(
            "payload/file-table entry count mismatch",
        ));
    }
    for (table_record, payload_entry) in files.iter().zip(&entries) {
        if table_record != &payload_entry.record {
            return Err(PackageError::invalid(format!(
                "payload does not match file table at {:?}",
                table_record.path
            )));
        }
    }
    let vdb_prefix = format!(
        "var/db/pkg/{}/{}/",
        metadata.portage.category, metadata.portage.pf
    );
    let vdb_directory = vdb_prefix.trim_end_matches('/');
    if !files
        .iter()
        .any(|record| record.path == format!("{vdb_prefix}CONTENTS"))
    {
        return Err(PackageError::invalid(
            "artifact does not contain its declared VDB entry",
        ));
    }
    for record in &files {
        if record.path.starts_with("var/lib/oxys/")
            || record.path.starts_with("var/cache/oxys/")
            || record.path == "var/lib/portage/world"
        {
            return Err(PackageError::invalid(format!(
                "artifact contains protected Oxys state path {}",
                record.path
            )));
        }
        if record.path.starts_with("var/db/pkg/")
            && record.path != vdb_directory
            && !record.path.starts_with(&vdb_prefix)
        {
            return Err(PackageError::invalid(format!(
                "artifact contains a foreign VDB path {}",
                record.path
            )));
        }
    }
    Ok(Artifact {
        metadata,
        files,
        entries,
    })
}

fn check_length(name: &str, length: u64, maximum: u64) -> Result<(), PackageError> {
    if length > maximum || usize::try_from(length).is_err() {
        return Err(PackageError::invalid(format!(
            "{name} exceeds format limit"
        )));
    }
    Ok(())
}

fn read_exact_vec(reader: &mut impl Read, length: u64) -> Result<Vec<u8>, PackageError> {
    let length =
        usize::try_from(length).map_err(|_| PackageError::invalid("length exceeds host limits"))?;
    let mut result = vec![0_u8; length];
    reader.read_exact(&mut result)?;
    Ok(result)
}

fn validate_entries(entries: &[PayloadEntry], verify_hashes: bool) -> Result<(), PackageError> {
    if entries.len() as u64 > MAX_FILES {
        return Err(PackageError::invalid("too many payload files"));
    }
    let mut previous: Option<&str> = None;
    let mut kinds = BTreeMap::new();
    let mut records = BTreeMap::new();
    for entry in entries {
        let record = &entry.record;
        validate_relative_path(&record.path)?;
        if previous.is_some_and(|path| path.as_bytes() >= record.path.as_bytes()) {
            return Err(PackageError::invalid(
                "file table is not strictly bytewise sorted",
            ));
        }
        previous = Some(&record.path);
        if record.mode & 0o6000 != 0 {
            return Err(PackageError::invalid(format!(
                "setuid/setgid entry is not allowed: {}",
                record.path
            )));
        }
        if record.mode & !0o7777 != 0 {
            return Err(PackageError::invalid(format!(
                "invalid permission bits for {}",
                record.path
            )));
        }
        match record.kind {
            FileKind::Regular => {
                if record.link_target.is_some() || record.size != entry.data.len() as u64 {
                    return Err(PackageError::invalid(format!(
                        "invalid file record {}",
                        record.path
                    )));
                }
            }
            FileKind::Directory => {
                if record.link_target.is_some()
                    || record.size != 0
                    || !entry.data.is_empty()
                    || record.sha256 != EMPTY_SHA256
                {
                    return Err(PackageError::invalid(format!(
                        "invalid directory record {}",
                        record.path
                    )));
                }
            }
            FileKind::Symlink => {
                let target = record.link_target.as_deref().ok_or_else(|| {
                    PackageError::invalid(format!("symlink has no target: {}", record.path))
                })?;
                if target.contains('\0')
                    || record.size != target.len() as u64
                    || !entry.data.is_empty()
                {
                    return Err(PackageError::invalid(format!(
                        "invalid symlink record {}",
                        record.path
                    )));
                }
            }
            FileKind::Hardlink => {
                let target = record.link_target.as_deref().ok_or_else(|| {
                    PackageError::invalid(format!("hardlink has no target: {}", record.path))
                })?;
                validate_relative_path(target)?;
                if target.as_bytes() >= record.path.as_bytes() || !entry.data.is_empty() {
                    return Err(PackageError::invalid(format!(
                        "invalid hardlink target for {}",
                        record.path
                    )));
                }
                let canonical: &&FileRecord = records.get(target).ok_or_else(|| {
                    PackageError::invalid(format!(
                        "hardlink target is not in the payload: {target}"
                    ))
                })?;
                if canonical.kind != FileKind::Regular
                    || canonical.mode != record.mode
                    || canonical.uid != record.uid
                    || canonical.gid != record.gid
                    || canonical.size != record.size
                    || canonical.sha256 != record.sha256
                {
                    return Err(PackageError::invalid(format!(
                        "hardlink metadata does not match target for {}",
                        record.path
                    )));
                }
            }
        }
        kinds.insert(record.path.as_str(), record.kind);
        records.insert(record.path.as_str(), record);
    }
    if verify_hashes {
        parallel::try_for_each(entries, |entry| {
            let expected = match entry.record.kind {
                FileKind::Regular => sha256(&entry.data),
                FileKind::Directory => EMPTY_SHA256,
                FileKind::Symlink => sha256(
                    entry
                        .record
                        .link_target
                        .as_deref()
                        .unwrap_or_default()
                        .as_bytes(),
                ),
                // The target record is checked above, and its inline bytes are
                // independently hashed as a regular file.
                FileKind::Hardlink => entry.record.sha256,
            };
            if entry.record.sha256 == expected {
                Ok(())
            } else {
                Err(PackageError::invalid(format!(
                    "payload SHA-256 mismatch for {}",
                    entry.record.path
                )))
            }
        })?;
    }
    for record in entries.iter().map(|entry| &entry.record) {
        let mut prefix = String::new();
        let parts: Vec<_> = record.path.split('/').collect();
        for part in &parts[..parts.len() - 1] {
            if !prefix.is_empty() {
                prefix.push('/');
            }
            prefix.push_str(part);
            if kinds.get(prefix.as_str()) == Some(&FileKind::Symlink) {
                return Err(PackageError::invalid(format!(
                    "payload path traverses packaged symlink: {}",
                    record.path
                )));
            }
        }
    }
    Ok(())
}

fn encode_file_table(entries: &[PayloadEntry]) -> Result<Vec<u8>, PackageError> {
    let mut output = Vec::new();
    output.extend_from_slice(&(entries.len() as u64).to_le_bytes());
    for entry in entries {
        let record = &entry.record;
        let path = record.path.as_bytes();
        let target = record.link_target.as_deref().unwrap_or("").as_bytes();
        let path_len =
            u32::try_from(path.len()).map_err(|_| PackageError::invalid("path too long"))?;
        let target_len = u32::try_from(target.len())
            .map_err(|_| PackageError::invalid("link target too long"))?;
        output.push(record.kind.code());
        output.extend_from_slice(&record.mode.to_le_bytes());
        output.extend_from_slice(&record.uid.to_le_bytes());
        output.extend_from_slice(&record.gid.to_le_bytes());
        output.extend_from_slice(&record.size.to_le_bytes());
        output.extend_from_slice(&record.sha256);
        output.extend_from_slice(&path_len.to_le_bytes());
        output.extend_from_slice(&target_len.to_le_bytes());
        output.extend_from_slice(path);
        output.extend_from_slice(target);
    }
    Ok(output)
}

fn decode_file_table(bytes: &[u8]) -> Result<Vec<FileRecord>, PackageError> {
    let mut cursor = Cursor::new(bytes);
    let count = cursor.u64()?;
    if count > MAX_FILES {
        return Err(PackageError::invalid(
            "file table declares too many entries",
        ));
    }
    let mut records = Vec::with_capacity(count as usize);
    for _ in 0..count {
        let kind = FileKind::from_code(cursor.u8()?)?;
        let mode = cursor.u32()?;
        let uid = cursor.u32()?;
        let gid = cursor.u32()?;
        let size = cursor.u64()?;
        let sha256: [u8; 32] = cursor.bytes(32)?.try_into().unwrap();
        let path_len = cursor.u32()? as usize;
        let target_len = cursor.u32()? as usize;
        let path = cursor.string(path_len)?;
        let target = cursor.string(target_len)?;
        validate_relative_path(&path)?;
        match kind {
            FileKind::Regular | FileKind::Directory if target_len != 0 => {
                return Err(PackageError::invalid(format!(
                    "non-link file-table record has a target: {path}"
                )));
            }
            FileKind::Symlink | FileKind::Hardlink if target_len == 0 => {
                return Err(PackageError::invalid(format!(
                    "link file-table record has an empty target: {path}"
                )));
            }
            _ => {}
        }
        records.push(FileRecord {
            kind,
            mode,
            uid,
            gid,
            size,
            sha256,
            path,
            link_target: matches!(kind, FileKind::Symlink | FileKind::Hardlink).then_some(target),
        });
    }
    if cursor.remaining() != 0 {
        return Err(PackageError::invalid("trailing bytes in file table"));
    }
    Ok(records)
}

struct Cursor<'a> {
    bytes: &'a [u8],
    offset: usize,
}

impl<'a> Cursor<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, offset: 0 }
    }
    fn bytes(&mut self, length: usize) -> Result<&'a [u8], PackageError> {
        let end = self
            .offset
            .checked_add(length)
            .ok_or_else(|| PackageError::invalid("file table overflow"))?;
        let result = self
            .bytes
            .get(self.offset..end)
            .ok_or_else(|| PackageError::invalid("truncated file table"))?;
        self.offset = end;
        Ok(result)
    }
    fn u8(&mut self) -> Result<u8, PackageError> {
        Ok(self.bytes(1)?[0])
    }
    fn u32(&mut self) -> Result<u32, PackageError> {
        Ok(u32::from_le_bytes(self.bytes(4)?.try_into().unwrap()))
    }
    fn u64(&mut self) -> Result<u64, PackageError> {
        Ok(u64::from_le_bytes(self.bytes(8)?.try_into().unwrap()))
    }
    fn string(&mut self, length: usize) -> Result<String, PackageError> {
        String::from_utf8(self.bytes(length)?.to_vec())
            .map_err(|_| PackageError::invalid("file table path is not UTF-8"))
    }
    fn remaining(&self) -> usize {
        self.bytes.len() - self.offset
    }
}

fn encode_tar(entries: &[PayloadEntry]) -> Result<Vec<u8>, PackageError> {
    let mut output = Vec::new();
    for entry in entries {
        let record = &entry.record;
        let mut header = [0_u8; 512];
        write_tar_path(&mut header, &record.path)?;
        write_octal(&mut header[100..108], record.mode as u64)?;
        write_octal(&mut header[108..116], record.uid as u64)?;
        write_octal(&mut header[116..124], record.gid as u64)?;
        let data_size = if record.kind == FileKind::Regular {
            record.size
        } else {
            0
        };
        write_octal(&mut header[124..136], data_size)?;
        write_octal(&mut header[136..148], 0)?;
        header[148..156].fill(b' ');
        header[156] = match record.kind {
            FileKind::Regular => b'0',
            FileKind::Directory => b'5',
            FileKind::Symlink => b'2',
            FileKind::Hardlink => b'1',
        };
        if let Some(target) = &record.link_target {
            if target.len() > 100 {
                return Err(PackageError::invalid(format!(
                    "link target exceeds POSIX ustar limit: {}",
                    record.path
                )));
            }
            header[157..157 + target.len()].copy_from_slice(target.as_bytes());
        }
        header[257..263].copy_from_slice(b"ustar\0");
        header[263..265].copy_from_slice(b"00");
        let checksum: u64 = header.iter().map(|byte| *byte as u64).sum();
        write_checksum(&mut header[148..156], checksum)?;
        output.extend_from_slice(&header);
        if record.kind == FileKind::Regular {
            output.extend_from_slice(&entry.data);
            let padding = (512 - entry.data.len() % 512) % 512;
            output.resize(output.len() + padding, 0);
        }
    }
    output.resize(output.len() + 1024, 0);
    Ok(output)
}

fn write_tar_path(header: &mut [u8; 512], path: &str) -> Result<(), PackageError> {
    let bytes = path.as_bytes();
    if bytes.len() <= 100 {
        header[..bytes.len()].copy_from_slice(bytes);
        return Ok(());
    }
    let split = path
        .char_indices()
        .filter(|(_, ch)| *ch == '/')
        .map(|(index, _)| index)
        .rev()
        .find(|index| *index <= 155 && bytes.len() - index - 1 <= 100)
        .ok_or_else(|| PackageError::invalid(format!("path exceeds POSIX ustar limit: {path}")))?;
    header[..bytes.len() - split - 1].copy_from_slice(&bytes[split + 1..]);
    header[345..345 + split].copy_from_slice(&bytes[..split]);
    Ok(())
}

fn write_octal(field: &mut [u8], value: u64) -> Result<(), PackageError> {
    let rendered = format!("{value:o}");
    if rendered.len() + 1 > field.len() {
        return Err(PackageError::invalid("tar numeric field overflow"));
    }
    field.fill(b'0');
    let start = field.len() - 1 - rendered.len();
    field[start..start + rendered.len()].copy_from_slice(rendered.as_bytes());
    field[field.len() - 1] = 0;
    Ok(())
}

fn write_checksum(field: &mut [u8], value: u64) -> Result<(), PackageError> {
    let rendered = format!("{value:06o}");
    if rendered.len() != 6 {
        return Err(PackageError::invalid("tar checksum overflow"));
    }
    field[..6].copy_from_slice(rendered.as_bytes());
    field[6] = 0;
    field[7] = b' ';
    Ok(())
}

fn decode_tar(bytes: &[u8]) -> Result<Vec<PayloadEntry>, PackageError> {
    let mut offset = 0_usize;
    let mut entries: Vec<PayloadEntry> = Vec::new();
    let mut entry_indexes = BTreeMap::new();
    let mut saw_end = false;
    while offset
        .checked_add(512)
        .is_some_and(|end| end <= bytes.len())
    {
        let header = &bytes[offset..offset + 512];
        offset += 512;
        if header.iter().all(|byte| *byte == 0) {
            if bytes
                .get(offset..offset + 512)
                .is_none_or(|next| !next.iter().all(|byte| *byte == 0))
            {
                return Err(PackageError::invalid("tar has only one end marker"));
            }
            if bytes[offset..].iter().any(|byte| *byte != 0) {
                return Err(PackageError::invalid("nonzero data follows tar end marker"));
            }
            saw_end = true;
            break;
        }
        if &header[257..263] != b"ustar\0" || &header[263..265] != b"00" {
            return Err(PackageError::invalid("payload is not POSIX ustar"));
        }
        if parse_octal(&header[136..148])? != 0 {
            return Err(PackageError::invalid("tar mtime is not normalized"));
        }
        let expected = parse_octal(&header[148..156])?;
        let actual: u64 = header
            .iter()
            .enumerate()
            .map(|(index, byte)| {
                if (148..156).contains(&index) {
                    b' ' as u64
                } else {
                    *byte as u64
                }
            })
            .sum();
        if expected != actual {
            return Err(PackageError::invalid("tar header checksum mismatch"));
        }
        let name = tar_string(&header[..100])?;
        let prefix = tar_string(&header[345..500])?;
        let path = if prefix.is_empty() {
            name
        } else {
            format!("{prefix}/{name}")
        };
        validate_relative_path(&path)?;
        let mode = u32::try_from(parse_octal(&header[100..108])?)
            .map_err(|_| PackageError::invalid("tar mode overflow"))?;
        let uid = u32::try_from(parse_octal(&header[108..116])?)
            .map_err(|_| PackageError::invalid("tar uid overflow"))?;
        let gid = u32::try_from(parse_octal(&header[116..124])?)
            .map_err(|_| PackageError::invalid("tar gid overflow"))?;
        let archive_size = parse_octal(&header[124..136])?;
        let kind = match header[156] {
            0 | b'0' => FileKind::Regular,
            b'5' => FileKind::Directory,
            b'2' => FileKind::Symlink,
            b'1' => FileKind::Hardlink,
            other => {
                return Err(PackageError::invalid(format!(
                    "tar special type {other} is not allowed"
                )));
            }
        };
        let data_size = usize::try_from(archive_size)
            .map_err(|_| PackageError::invalid("tar entry too large"))?;
        let end = offset
            .checked_add(data_size)
            .ok_or_else(|| PackageError::invalid("tar size overflow"))?;
        let data = bytes
            .get(offset..end)
            .ok_or_else(|| PackageError::invalid("truncated tar entry"))?
            .to_vec();
        let padded = data_size
            .checked_add((512 - data_size % 512) % 512)
            .ok_or_else(|| PackageError::invalid("tar padding overflow"))?;
        offset = offset
            .checked_add(padded)
            .filter(|end| *end <= bytes.len())
            .ok_or_else(|| PackageError::invalid("truncated tar padding"))?;
        let target = matches!(kind, FileKind::Symlink | FileKind::Hardlink)
            .then(|| tar_string(&header[157..257]))
            .transpose()?;
        if kind != FileKind::Regular && archive_size != 0 {
            return Err(PackageError::invalid("non-file tar entry has data"));
        }
        let (logical_size, record_hash) = match kind {
            FileKind::Regular => (archive_size, sha256(&data)),
            FileKind::Directory => (0, EMPTY_SHA256),
            FileKind::Symlink => {
                let target = target.as_deref().unwrap_or_default();
                (target.len() as u64, sha256(target.as_bytes()))
            }
            FileKind::Hardlink => {
                let target_path = target.as_deref().unwrap_or_default();
                validate_relative_path(target_path)?;
                let canonical = entry_indexes
                    .get(target_path)
                    .and_then(|index: &usize| entries.get(*index))
                    .ok_or_else(|| {
                        PackageError::invalid(format!(
                            "tar hardlink target is not an earlier entry: {target_path}"
                        ))
                    })?;
                if canonical.record.kind != FileKind::Regular {
                    return Err(PackageError::invalid(format!(
                        "tar hardlink target is not a regular file: {target_path}"
                    )));
                }
                (canonical.record.size, canonical.record.sha256)
            }
        };
        entries.push(PayloadEntry {
            record: FileRecord {
                kind,
                mode,
                uid,
                gid,
                size: logical_size,
                sha256: record_hash,
                path,
                link_target: target,
            },
            data,
        });
        entry_indexes.insert(
            entries.last().unwrap().record.path.clone(),
            entries.len() - 1,
        );
        if entries.len() as u64 > MAX_FILES {
            return Err(PackageError::invalid("tar has too many entries"));
        }
    }
    if !saw_end {
        return Err(PackageError::invalid("tar end marker is missing"));
    }
    Ok(entries)
}

fn tar_string(field: &[u8]) -> Result<String, PackageError> {
    let end = field
        .iter()
        .position(|byte| *byte == 0)
        .unwrap_or(field.len());
    if field[end..].iter().any(|byte| *byte != 0) {
        return Err(PackageError::invalid("tar string has data after NUL"));
    }
    std::str::from_utf8(&field[..end])
        .map(str::to_owned)
        .map_err(|_| PackageError::invalid("tar path is not UTF-8"))
}

fn parse_octal(field: &[u8]) -> Result<u64, PackageError> {
    let text = std::str::from_utf8(field)
        .map_err(|_| PackageError::invalid("invalid tar numeric field"))?
        .trim_matches(|ch| ch == '\0' || ch == ' ');
    if text.is_empty() {
        return Ok(0);
    }
    if !text.bytes().all(|byte| (b'0'..=b'7').contains(&byte)) {
        return Err(PackageError::invalid("invalid tar octal field"));
    }
    u64::from_str_radix(text, 8).map_err(|_| PackageError::invalid("tar octal field overflow"))
}

fn run_zstd(input: &[u8], decompress: bool, maximum_output: u64) -> Result<Vec<u8>, PackageError> {
    check_length(
        "zstd output",
        maximum_output,
        MAX_UNCOMPRESSED_PAYLOAD.max(MAX_COMPRESSED_PAYLOAD),
    )?;
    let mut command = Command::new("zstd");
    command.args(["-q", "-c"]);
    if decompress {
        command.arg("-d");
    } else {
        command.args(["-19", "--threads=1"]);
    }
    let mut child = command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| PackageError::invalid(format!("cannot run zstd: {error}")))?;
    let mut stdin = child.stdin.take().unwrap();
    let mut output = Vec::new();
    let writer_result = std::thread::scope(|scope| {
        let writer = scope.spawn(move || {
            let result = stdin.write_all(input);
            drop(stdin);
            result
        });
        child
            .stdout
            .take()
            .unwrap()
            .take(maximum_output + 1)
            .read_to_end(&mut output)?;
        if output.len() as u64 > maximum_output {
            let _ = child.kill();
        }
        writer
            .join()
            .map_err(|_| PackageError::invalid("zstd input thread failed"))?
            .map_err(PackageError::from)
    });
    writer_result?;
    let status = child.wait()?;
    if output.len() as u64 > maximum_output {
        return Err(PackageError::invalid("zstd output exceeds declared limit"));
    }
    if !status.success() {
        return Err(PackageError::invalid(format!("zstd exited with {status}")));
    }
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_unsafe_paths() {
        for path in ["", "/usr/bin/x", "../x", "usr/../x", "usr//x", "./x", "x/"] {
            assert!(validate_relative_path(path).is_err(), "accepted {path:?}");
        }
        assert!(validate_relative_path("usr/bin/wl-copy").is_ok());
    }

    #[test]
    fn validates_direct_earlier_hardlink_targets() {
        let canonical = PayloadEntry {
            record: FileRecord {
                kind: FileKind::Regular,
                mode: 0o755,
                uid: 0,
                gid: 0,
                size: 1,
                sha256: sha256(b"x"),
                path: "usr/bin/a".into(),
                link_target: None,
            },
            data: b"x".to_vec(),
        };
        let alias = PayloadEntry {
            record: FileRecord {
                kind: FileKind::Hardlink,
                path: "usr/bin/b".into(),
                link_target: Some("usr/bin/a".into()),
                ..canonical.record.clone()
            },
            data: Vec::new(),
        };
        assert!(validate_entries(&[canonical.clone(), alias.clone()], true).is_ok());

        let mut self_link = alias;
        self_link.record.link_target = Some("usr/bin/b".into());
        assert!(validate_entries(&[canonical, self_link], true).is_err());
    }

    #[test]
    fn file_table_rejects_targets_on_non_link_records() {
        let entry = PayloadEntry {
            record: FileRecord {
                kind: FileKind::Regular,
                mode: 0o644,
                uid: 0,
                gid: 0,
                size: 1,
                sha256: sha256(b"x"),
                path: "usr/share/x".into(),
                link_target: Some("hidden".into()),
            },
            data: b"x".to_vec(),
        };
        let table = encode_file_table(&[entry]).unwrap();
        assert!(decode_file_table(&table).is_err());
    }
}
