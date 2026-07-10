use std::{
    fs,
    path::{Path, PathBuf},
};

pub fn discover_repo_roots(portage_tree: &Path) -> Vec<PathBuf> {
    let mut repo_roots = Vec::new();

    if is_repo_root(portage_tree) {
        repo_roots.push(portage_tree.to_path_buf());
        return repo_roots;
    }

    for repo_root in configured_repo_roots() {
        if is_repo_root(&repo_root) {
            push_unique_path(&mut repo_roots, repo_root);
        }
    }

    if let Ok(entries) = fs::read_dir(portage_tree) {
        for entry in entries.filter_map(Result::ok) {
            let path = entry.path();
            if is_repo_root(&path) {
                push_unique_path(&mut repo_roots, path);
            }
        }
    }

    if repo_roots.is_empty() {
        repo_roots.push(portage_tree.to_path_buf());
    }

    repo_roots
}

fn configured_repo_roots() -> Vec<PathBuf> {
    let repos_conf_dir = Path::new("/etc/portage/repos.conf");
    let mut repo_roots = Vec::new();

    if repos_conf_dir.is_file() {
        if let Ok(contents) = fs::read_to_string(repos_conf_dir) {
            extend_repo_roots_from_config(&mut repo_roots, &contents);
        }
        return repo_roots;
    }

    if let Ok(entries) = fs::read_dir(repos_conf_dir) {
        let mut config_paths = entries
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|path| path.extension().and_then(|ext| ext.to_str()) == Some("conf"))
            .collect::<Vec<_>>();
        config_paths.sort();

        for config_path in config_paths {
            if let Ok(contents) = fs::read_to_string(&config_path) {
                extend_repo_roots_from_config(&mut repo_roots, &contents);
            }
        }
    }

    repo_roots
}

fn extend_repo_roots_from_config(repo_roots: &mut Vec<PathBuf>, contents: &str) {
    for line in contents.lines() {
        let line = line.split('#').next().unwrap_or_default().trim();
        if let Some((key, value)) = line.split_once('=') {
            if key.trim() == "location" {
                let value = value.trim();
                if !value.is_empty() {
                    push_unique_path(repo_roots, PathBuf::from(value));
                }
            }
        }
    }
}

fn is_repo_root(path: &Path) -> bool {
    path.join("metadata").join("md5-cache").is_dir()
}

fn push_unique_path(paths: &mut Vec<PathBuf>, path: PathBuf) {
    if !paths.iter().any(|existing| existing == &path) {
        paths.push(path);
    }
}

pub fn list_package_versions(repo_root: &Path, category: &str, package_name: &str) -> Vec<String> {
    let package_dir = repo_root.join("metadata").join("md5-cache").join(category);
    let prefix = format!("{package_name}-");

    match fs::read_dir(&package_dir) {
        Ok(entries) => entries
            .filter_map(Result::ok)
            .filter_map(|entry| {
                let file_name = entry.file_name().into_string().ok()?;
                file_name
                    .strip_prefix(&prefix)
                    .filter(|version| !version.is_empty())
                    .map(ToOwned::to_owned)
            })
            .collect(),
        Err(_) => Vec::new(),
    }
}

pub fn find_metadata_path(
    portage_tree: &Path,
    category: &str,
    package_name: &str,
    version: &str,
) -> Option<PathBuf> {
    let relative_path = Path::new("metadata")
        .join("md5-cache")
        .join(category)
        .join(format!("{package_name}-{version}"));

    discover_repo_roots(portage_tree)
        .into_iter()
        .map(|repo_root| repo_root.join(&relative_path))
        .find(|path| path.exists())
}
