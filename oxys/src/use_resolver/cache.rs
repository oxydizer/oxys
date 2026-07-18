use std::{
    fs,
    path::{Path, PathBuf},
    time::Duration,
};

use chrono::{DateTime, Utc};

use super::{
    PackageMetadata, UseResolverError, parse::parse_md5_cache_metadata, util::sibling_temp_path,
};

const CACHE_TTL_DAYS: i64 = 7;

/// Returns the cache file path for a package metadata entry.
pub fn cache_path_for_metadata(
    cache_dir: &Path,
    package: &str,
    version: &str,
) -> Result<PathBuf, UseResolverError> {
    let (category, package_name) =
        package
            .split_once('/')
            .ok_or_else(|| UseResolverError::InvalidPackageIdentifier {
                package: package.to_owned(),
            })?;

    if category.is_empty() || package_name.is_empty() || version.is_empty() {
        return Err(UseResolverError::InvalidPackageIdentifier {
            package: package.to_owned(),
        });
    }

    Ok(cache_dir.join(format!("{category}--{package_name}--{version}.json")))
}

/// Loads package metadata from a fresh cache entry or reparses local md5-cache metadata and
/// refreshes the cache on miss or staleness.
pub fn load_or_parse_metadata(
    md5_cache_path: &Path,
    cache_dir: &Path,
) -> Result<PackageMetadata, UseResolverError> {
    let (package, version) = super::package_from_md5_cache_path(md5_cache_path)?;
    let cache_path = cache_path_for_metadata(cache_dir, &package, &version)?;

    if let Some(metadata) = read_cache_if_fresh(&cache_path)? {
        return Ok(metadata);
    }

    let contents = fs::read_to_string(md5_cache_path).map_err(|source| UseResolverError::Io {
        path: md5_cache_path.to_path_buf(),
        source,
    })?;

    let metadata = parse_md5_cache_metadata(md5_cache_path, &contents, Utc::now())?;
    write_cache_metadata(&cache_path, &metadata)?;
    Ok(metadata)
}

/// Invalidates cached metadata by removing generated JSON cache entries in `cache_dir`.
pub fn sync(cache_dir: &Path) -> Result<(), UseResolverError> {
    if !cache_dir.exists() {
        return Ok(());
    }

    for entry in fs::read_dir(cache_dir).map_err(|source| UseResolverError::Io {
        path: cache_dir.to_path_buf(),
        source,
    })? {
        let entry = entry.map_err(|source| UseResolverError::Io {
            path: cache_dir.to_path_buf(),
            source,
        })?;
        let path = entry.path();

        if path.is_file() && path.extension().and_then(|ext| ext.to_str()) == Some("json") {
            fs::remove_file(&path).map_err(|source| UseResolverError::Io {
                path: path.clone(),
                source,
            })?;
        }
    }

    Ok(())
}

fn read_cache_if_fresh(cache_path: &Path) -> Result<Option<PackageMetadata>, UseResolverError> {
    if !cache_path.exists() {
        return Ok(None);
    }

    let metadata = read_cache_metadata(cache_path)?;
    if is_stale(&metadata.cached_at) {
        return Ok(None);
    }

    Ok(Some(metadata))
}

fn read_cache_metadata(cache_path: &Path) -> Result<PackageMetadata, UseResolverError> {
    let contents = fs::read_to_string(cache_path).map_err(|source| UseResolverError::Io {
        path: cache_path.to_path_buf(),
        source,
    })?;

    serde_json::from_str(&contents).map_err(|source| UseResolverError::Json {
        path: cache_path.to_path_buf(),
        source,
    })
}

fn write_cache_metadata(
    cache_path: &Path,
    metadata: &PackageMetadata,
) -> Result<(), UseResolverError> {
    if let Some(parent) = cache_path.parent() {
        fs::create_dir_all(parent).map_err(|source| UseResolverError::Io {
            path: parent.to_path_buf(),
            source,
        })?;
    }

    let temp_path = sibling_temp_path(cache_path, "metadata.tmp");
    let json = serde_json::to_string_pretty(metadata).map_err(|source| UseResolverError::Json {
        path: cache_path.to_path_buf(),
        source,
    })?;

    fs::write(&temp_path, json).map_err(|source| UseResolverError::Io {
        path: temp_path.clone(),
        source,
    })?;

    fs::rename(&temp_path, cache_path).map_err(|source| UseResolverError::Io {
        path: cache_path.to_path_buf(),
        source,
    })?;

    Ok(())
}

fn is_stale(cached_at: &DateTime<Utc>) -> bool {
    match Utc::now().signed_duration_since(*cached_at).to_std() {
        Ok(age) => age > Duration::from_secs((CACHE_TTL_DAYS as u64) * 24 * 60 * 60),
        Err(_) => false,
    }
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        path::{Path, PathBuf},
    };

    use chrono::{Duration, Utc};

    use super::{
        cache_path_for_metadata, load_or_parse_metadata, read_cache_metadata, sync,
        write_cache_metadata,
    };
    use crate::use_resolver::{ConditionalDep, PackageMetadata, UseFlag};

    #[test]
    fn populates_cache_on_miss() -> Result<(), Box<dyn std::error::Error>> {
        let root = test_root("cache_miss_population");
        let md5_cache_path = root.join("repo/metadata/md5-cache/gui-wm/niri-25.11-r1");
        let cache_dir = root.join(".oxys/cache/use-flags");

        write_md5_cache_file(
            &md5_cache_path,
            "IUSE=+screencast xwayland\nDEPEND=screencast? ( media-video/pipewire:= )\nRDEPEND=gui-libs/gtk:4\nKEYWORDS=~amd64\n",
        )?;

        let metadata = load_or_parse_metadata(&md5_cache_path, &cache_dir)?;
        let cache_path = cache_path_for_metadata(&cache_dir, "gui-wm/niri", "25.11-r1")?;

        assert_eq!(metadata.package, "gui-wm/niri");
        assert!(cache_path.exists());

        let cached = read_cache_metadata(&cache_path)?;
        assert_eq!(cached.package, metadata.package);

        cleanup(&root)?;
        Ok(())
    }

    #[test]
    fn returns_cache_hit_without_reading_source() -> Result<(), Box<dyn std::error::Error>> {
        let root = test_root("cache_hit");
        let md5_cache_path = root.join("repo/metadata/md5-cache/gui-wm/niri-25.11-r1");
        let cache_dir = root.join(".oxys/cache/use-flags");
        let cache_path = cache_path_for_metadata(&cache_dir, "gui-wm/niri", "25.11-r1")?;

        let cached_metadata = sample_metadata(Utc::now());
        write_cache_metadata(&cache_path, &cached_metadata)?;

        let metadata = load_or_parse_metadata(&md5_cache_path, &cache_dir)?;

        assert_eq!(metadata, cached_metadata);

        cleanup(&root)?;
        Ok(())
    }

    #[test]
    fn refreshes_stale_cache() -> Result<(), Box<dyn std::error::Error>> {
        let root = test_root("stale_cache_refresh");
        let md5_cache_path = root.join("repo/metadata/md5-cache/gui-wm/niri-25.11-r1");
        let cache_dir = root.join(".oxys/cache/use-flags");
        let cache_path = cache_path_for_metadata(&cache_dir, "gui-wm/niri", "25.11-r1")?;

        write_md5_cache_file(
            &md5_cache_path,
            "IUSE=+screencast\nDEPEND=media-video/pipewire:=\nRDEPEND=gui-libs/gtk:4\nKEYWORDS=~amd64\n",
        )?;

        let stale_metadata = sample_metadata(Utc::now() - Duration::days(8));
        write_cache_metadata(&cache_path, &stale_metadata)?;

        let refreshed = load_or_parse_metadata(&md5_cache_path, &cache_dir)?;

        assert_ne!(refreshed.cached_at, stale_metadata.cached_at);
        assert_eq!(
            refreshed.iuse,
            vec![UseFlag {
                name: "screencast".to_owned(),
                default_enabled: true,
            }]
        );

        let cached = read_cache_metadata(&cache_path)?;
        assert_eq!(cached, refreshed);

        cleanup(&root)?;
        Ok(())
    }

    #[test]
    fn sync_invalidates_cached_metadata_files() -> Result<(), Box<dyn std::error::Error>> {
        let root = test_root("sync_invalidation");
        let cache_dir = root.join(".oxys/cache/use-flags");
        let cache_path = cache_path_for_metadata(&cache_dir, "gui-wm/niri", "25.11-r1")?;
        let other_path = cache_dir.join("README.txt");

        write_cache_metadata(&cache_path, &sample_metadata(Utc::now()))?;
        if let Some(parent) = other_path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(&other_path, "keep")?;

        sync(&cache_dir)?;

        assert!(!cache_path.exists());
        assert!(other_path.exists());

        cleanup(&root)?;
        Ok(())
    }

    #[test]
    fn sync_is_noop_for_missing_directory() -> Result<(), Box<dyn std::error::Error>> {
        let root = test_root("sync_missing_dir");
        let cache_dir = root.join(".oxys/cache/use-flags");

        sync(&cache_dir)?;

        assert!(!cache_dir.exists());
        cleanup(&root)?;
        Ok(())
    }

    fn sample_metadata(cached_at: chrono::DateTime<Utc>) -> PackageMetadata {
        PackageMetadata {
            package: "gui-wm/niri".to_owned(),
            version: "25.11-r1".to_owned(),
            iuse: vec![UseFlag {
                name: "xwayland".to_owned(),
                default_enabled: false,
            }],
            depend: vec![ConditionalDep {
                condition: None,
                package: "gui-libs/gtk".to_owned(),
                blocker: None,
                slot: None,
                subslot: None,
                slot_operator: None,
            }],
            bdepend: Vec::new(),
            rdepend: Vec::new(),
            pdepend: Vec::new(),
            required_use: Vec::new(),
            keywords: vec!["~amd64".to_owned()],
            licenses: Vec::new(),
            properties: Vec::new(),
            restrict: Vec::new(),
            provides: Vec::new(),
            slot: None,
            subslot: None,
            cached_at,
        }
    }

    fn write_md5_cache_file(path: &Path, contents: &str) -> Result<(), Box<dyn std::error::Error>> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(path, contents)?;
        Ok(())
    }

    fn test_root(name: &str) -> PathBuf {
        let unique = format!(
            "oxys_use_resolver_{name}_{}_{}",
            std::process::id(),
            Utc::now().timestamp_nanos_opt().unwrap_or_default()
        );
        std::env::temp_dir().join(unique)
    }

    fn cleanup(path: &Path) -> Result<(), Box<dyn std::error::Error>> {
        if path.exists() {
            fs::remove_dir_all(path)?;
        }
        Ok(())
    }
}
