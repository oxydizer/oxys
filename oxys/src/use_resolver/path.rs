use std::path::Path;

use super::{util::version_split_index, UseResolverError};

/// Derives the `category/package` atom and version from a Portage md5-cache file path.
pub fn package_from_md5_cache_path(path: &Path) -> Result<(String, String), UseResolverError> {
    let file_name = path
        .file_name()
        .and_then(|value| value.to_str())
        .ok_or_else(|| UseResolverError::InvalidCachePath {
            path: path.to_path_buf(),
        })?;

    let category = path
        .parent()
        .and_then(Path::file_name)
        .and_then(|value| value.to_str())
        .ok_or_else(|| UseResolverError::InvalidCachePath {
            path: path.to_path_buf(),
        })?;

    let split_at =
        version_split_index(file_name).ok_or_else(|| UseResolverError::InvalidPackageFileName {
            file_name: file_name.to_owned(),
        })?;

    let package_name = &file_name[..split_at];
    let version = &file_name[split_at + 1..];

    if package_name.is_empty() || version.is_empty() {
        return Err(UseResolverError::InvalidPackageFileName {
            file_name: file_name.to_owned(),
        });
    }

    Ok((format!("{category}/{package_name}"), version.to_owned()))
}

pub(crate) fn split_versioned_package(package: &str) -> Result<(String, String), UseResolverError> {
    let (category, package_name) =
        package
            .split_once('/')
            .ok_or_else(|| UseResolverError::InvalidPackageIdentifier {
                package: package.to_owned(),
            })?;

    if category.is_empty() || package_name.is_empty() {
        return Err(UseResolverError::InvalidPackageIdentifier {
            package: package.to_owned(),
        });
    }

    let split_at = version_split_index(package_name).ok_or_else(|| {
        UseResolverError::InvalidPackageIdentifier {
            package: package.to_owned(),
        }
    })?;

    let atom = &package_name[..split_at];
    let version = &package_name[split_at + 1..];

    if atom.is_empty() || version.is_empty() {
        return Err(UseResolverError::InvalidPackageIdentifier {
            package: package.to_owned(),
        });
    }

    Ok((format!("{category}/{atom}"), version.to_owned()))
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::package_from_md5_cache_path;

    #[test]
    fn extracts_package_and_version_from_md5_cache_path() -> Result<(), Box<dyn std::error::Error>>
    {
        let path = Path::new("/var/db/repos/guru/metadata/md5-cache/gui-wm/niri-25.11-r1");

        let (package, version) = package_from_md5_cache_path(path)?;

        assert_eq!(package, "gui-wm/niri");
        assert_eq!(version, "25.11-r1");
        Ok(())
    }

    #[test]
    fn supports_package_names_with_hyphens() -> Result<(), Box<dyn std::error::Error>> {
        let path = Path::new("/var/db/repos/gentoo/metadata/md5-cache/dev-util/cargo-c-0.10.11");

        let (package, version) = package_from_md5_cache_path(path)?;

        assert_eq!(package, "dev-util/cargo-c");
        assert_eq!(version, "0.10.11");
        Ok(())
    }

    #[test]
    fn rejects_path_without_filename() {
        let path = Path::new("/var/db/repos/gentoo/metadata/md5-cache/gui-wm/");

        let result = package_from_md5_cache_path(path);

        assert!(result.is_err());
    }
}
