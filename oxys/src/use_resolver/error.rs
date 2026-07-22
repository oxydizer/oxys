use std::path::PathBuf;

use thiserror::Error;

use crate::exec::ExecError;

/// Errors returned by parsing, caching, resolution, generation, and emerge streaming.
#[derive(Debug, Error)]
pub enum UseResolverError {
    #[error("failed to read md5-cache file {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("md5-cache path is invalid: {path}")]
    InvalidCachePath { path: PathBuf },
    #[error("invalid package filename in md5-cache path: {file_name}")]
    InvalidPackageFileName { file_name: String },
    #[error("package identifier is invalid: {package}")]
    InvalidPackageIdentifier { package: String },
    #[error("metadata file not found for package {package} at {path}{suggestion_note}")]
    MetadataNotFound {
        package: String,
        path: PathBuf,
        /// Preformatted `; did you mean '...'?` hint, empty when nothing close
        /// is known (or the failure is about versions rather than spelling).
        suggestion_note: String,
    },
    #[error(
        "no stable amd64 version is available for package {package}; add a package-scoped ~amd64 or ** keyword override to opt in"
    )]
    NoStableVersion { package: String },
    #[error("missing required metadata field: {field}")]
    MissingField { field: &'static str },
    #[error("invalid metadata field {field}: {message}")]
    InvalidField {
        field: &'static str,
        message: String,
    },
    #[error("failed to parse json cache file {path}: {source}")]
    Json {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },
    #[error(transparent)]
    Exec(#[from] ExecError),
    #[error("emerge exited unsuccessfully{package}{message}: {status}")]
    EmergeExit {
        status: String,
        package: String,
        message: String,
    },
}
