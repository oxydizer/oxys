use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
};

use crate::manifest::{Package, SystemManifest};

use super::{PortagePlan, UseResolverError, plan_portage};

/// A package operation parsed from `emerge -uDNp --columns @world`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PretendPackage {
    pub package: String,
    pub version: String,
    pub source: PretendPackageSource,
    pub operation: PretendOperation,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PretendPackageSource {
    Ebuild,
    Binary,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PretendOperation {
    Merge,
    Uninstall,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorldUpdatePlan {
    pub packages: Vec<PretendPackage>,
    pub sync_ran: bool,
    pub warnings: Vec<WorldUpdateWarning>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorldUpdateWarning {
    NotInManifest { package: String },
    RemovedByUpdate { package: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PretendParseError {
    pub message: String,
}

impl std::fmt::Display for PretendParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl std::error::Error for PretendParseError {}

/// Parses the package rows emitted by Portage pretend output.
///
/// The CLI runs emerge with `--columns --color=n` because Portage documents
/// `--columns` as the most copy/paste-friendly pretend format. This parser is
/// still intentionally conservative: any bracketed package row that does not
/// match a known merge/uninstall shape is an error.
pub fn parse_pretend_world_update(output: &str) -> Result<Vec<PretendPackage>, PretendParseError> {
    let mut packages = Vec::new();
    let mut saw_package_section = false;

    for (line_no, line) in output.lines().enumerate() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }

        if trimmed.contains("These are the packages that would be merged")
            || trimmed.contains("These are the packages that would be unmerged")
            || trimmed.contains("These are the packages that would be installed")
        {
            saw_package_section = true;
            continue;
        }

        if trimmed.contains("Nothing to merge") {
            return Ok(packages);
        }

        if !trimmed.starts_with('[') {
            continue;
        }

        let parsed = parse_pretend_package_line(trimmed).map_err(|message| PretendParseError {
            message: format!(
                "cannot verify emerge pretend output at line {}: {} ({})",
                line_no + 1,
                message,
                trimmed
            ),
        })?;
        packages.push(parsed);
    }

    if packages.is_empty() && saw_package_section {
        return Err(PretendParseError {
            message: "cannot verify emerge pretend output: package section did not contain any parseable package rows".to_owned(),
        });
    }

    if packages.is_empty() {
        return Err(PretendParseError {
            message: "cannot verify emerge pretend output: no package rows found".to_owned(),
        });
    }

    Ok(packages)
}

pub fn plan_update_preflight(
    current_manifest: &SystemManifest,
    pretend_packages: &[PretendPackage],
    portage_tree: &Path,
    cache_dir: &Path,
) -> Result<PortagePlan, UseResolverError> {
    let update_manifest = manifest_for_update_preflight(current_manifest, pretend_packages);
    plan_portage(&update_manifest, portage_tree, cache_dir)
}

pub fn build_world_update_plan(
    current_manifest: Option<&SystemManifest>,
    pretend_packages: Vec<PretendPackage>,
    sync_ran: bool,
) -> WorldUpdatePlan {
    let manifest_packages = current_manifest
        .map(|manifest| {
            manifest
                .packages
                .iter()
                .map(|package| package.package.as_str())
                .collect::<BTreeSet<_>>()
        })
        .unwrap_or_default();
    let mut warnings = Vec::new();

    for pretend in &pretend_packages {
        if current_manifest.is_some() && !manifest_packages.contains(pretend.package.as_str()) {
            warnings.push(WorldUpdateWarning::NotInManifest {
                package: pretend.package.clone(),
            });
        }

        if pretend.operation == PretendOperation::Uninstall {
            warnings.push(WorldUpdateWarning::RemovedByUpdate {
                package: pretend.package.clone(),
            });
        }
    }

    WorldUpdatePlan {
        packages: pretend_packages,
        sync_ran,
        warnings,
    }
}

pub fn manifest_for_update_preflight(
    current_manifest: &SystemManifest,
    pretend_packages: &[PretendPackage],
) -> SystemManifest {
    let mut manifest = current_manifest.clone();
    let mut packages = manifest
        .packages
        .into_iter()
        .map(|package| (package.package.clone(), package))
        .collect::<BTreeMap<_, _>>();

    for pretend in pretend_packages {
        if pretend.operation == PretendOperation::Uninstall {
            packages.remove(&pretend.package);
            continue;
        }

        let mut package = packages
            .remove(&pretend.package)
            .unwrap_or_else(|| Package::new(&pretend.package));
        package.version = Some(pretend.version.clone());

        match pretend.source {
            PretendPackageSource::Binary => {
                package.binary = true;
                package.from_source = false;
            }
            PretendPackageSource::Ebuild => {
                package.binary = false;
                package.from_source = true;
            }
        }

        packages.insert(package.package.clone(), package);
    }

    manifest.packages = packages.into_values().collect();
    manifest
}

fn parse_pretend_package_line(line: &str) -> Result<PretendPackage, String> {
    let (header, rest) = line
        .strip_prefix('[')
        .and_then(|value| value.split_once(']'))
        .ok_or_else(|| "missing bracketed emerge operation".to_owned())?;

    let mut header_tokens = header.split_whitespace();
    let type_token = header_tokens
        .next()
        .ok_or_else(|| "missing emerge operation type".to_owned())?;

    let (source, operation) = match type_token {
        "ebuild" => (PretendPackageSource::Ebuild, PretendOperation::Merge),
        "binary" => (PretendPackageSource::Binary, PretendOperation::Merge),
        "uninstall" => (PretendPackageSource::Ebuild, PretendOperation::Uninstall),
        "blocks" => return Err("blocker rows require manual resolution before update".to_owned()),
        other => return Err(format!("unsupported emerge operation type '{other}'")),
    };

    let tokens = rest.split_whitespace().collect::<Vec<_>>();
    let package_index = tokens
        .iter()
        .position(|token| token.contains('/'))
        .ok_or_else(|| "missing category/package token".to_owned())?;

    let package_token = clean_package_token(tokens[package_index]);
    let (package, version) =
        if let Some((package, version)) = split_versioned_package_token(&package_token) {
            (package, version)
        } else {
            let version = tokens
                .iter()
                .skip(package_index + 1)
                .find_map(|token| bracketed_version(token))
                .ok_or_else(|| "missing package version in columns output".to_owned())?;
            (strip_slot_and_repo(&package_token).to_owned(), version)
        };

    if package.split_once('/').is_none() {
        return Err("invalid package token".to_owned());
    }
    if version.is_empty() {
        return Err("empty package version".to_owned());
    }

    Ok(PretendPackage {
        package,
        version,
        source,
        operation,
    })
}

fn clean_package_token(token: &str) -> String {
    token
        .trim_matches(|ch: char| matches!(ch, '(' | ')' | ',' | ';' | '\''))
        .to_owned()
}

fn bracketed_version(token: &str) -> Option<String> {
    let trimmed = token.trim();
    let value = trimmed.strip_prefix('[')?.strip_suffix(']')?;
    if value.contains('/') || value.is_empty() {
        return None;
    }
    Some(value.to_owned())
}

fn split_versioned_package_token(token: &str) -> Option<(String, String)> {
    let token = strip_slot_and_repo(token);
    let (category, rest) = token.split_once('/')?;
    let split_at = rest.char_indices().rev().find_map(|(idx, ch)| {
        if ch != '-' {
            return None;
        }
        rest.get(idx + 1..)
            .and_then(|suffix| suffix.chars().next())
            .filter(|ch| ch.is_ascii_digit())
            .map(|_| idx)
    })?;

    let package_name = &rest[..split_at];
    let version = &rest[split_at + 1..];
    if package_name.is_empty() || version.is_empty() {
        return None;
    }

    Some((format!("{category}/{package_name}"), version.to_owned()))
}

fn strip_slot_and_repo(token: &str) -> &str {
    let without_repo = token
        .split_once("::")
        .map(|(left, _)| left)
        .unwrap_or(token);
    without_repo
        .split_once(':')
        .map(|(left, _)| left)
        .unwrap_or(without_repo)
}

#[cfg(test)]
mod tests {
    use super::{
        PretendOperation, PretendPackageSource, WorldUpdateWarning, build_world_update_plan,
        manifest_for_update_preflight, parse_pretend_world_update,
    };
    use crate::manifest::{Package, SystemManifest};

    #[test]
    fn parses_columns_pretend_rows() {
        let output = r#"
These are the packages that would be merged, in order:

[ebuild     U ] sys-devel/gcc           [15.1.1] [14.3.0]
[binary     U ] sys-libs/glibc          [2.41-r3] [2.40-r5]
"#;

        let parsed = parse_pretend_world_update(output).unwrap();

        assert_eq!(parsed.len(), 2);
        assert_eq!(parsed[0].package, "sys-devel/gcc");
        assert_eq!(parsed[0].version, "15.1.1");
        assert_eq!(parsed[0].source, PretendPackageSource::Ebuild);
        assert_eq!(parsed[1].package, "sys-libs/glibc");
        assert_eq!(parsed[1].version, "2.41-r3");
        assert_eq!(parsed[1].source, PretendPackageSource::Binary);
    }

    #[test]
    fn parses_non_columns_pretend_rows() {
        let output = r#"
These are the packages that would be merged, in order:

[ebuild     U ] sys-devel/gcc-15.1.1::gentoo [14.3.0]
"#;

        let parsed = parse_pretend_world_update(output).unwrap();

        assert_eq!(parsed[0].package, "sys-devel/gcc");
        assert_eq!(parsed[0].version, "15.1.1");
    }

    #[test]
    fn malformed_pretend_output_fails_closed() {
        let err = parse_pretend_world_update(
            "These are the packages that would be merged, in order:\n[ebuild U] ???\n",
        )
        .unwrap_err();

        assert!(err.message.contains("cannot verify emerge pretend output"));
    }

    #[test]
    fn nothing_to_merge_returns_empty_plan() {
        let parsed = parse_pretend_world_update(
            "Calculating dependencies... done!\nNothing to merge; quitting.\n",
        )
        .unwrap();

        assert!(parsed.is_empty());
    }

    #[test]
    fn blocker_rows_fail_closed() {
        let err = parse_pretend_world_update(
            "These are the packages that would be merged, in order:\n[blocks B      ] app-misc/new (\"app-misc/new\" is blocking app-misc/old)\n",
        )
        .unwrap_err();

        assert!(
            err.message
                .contains("blocker rows require manual resolution")
        );
    }

    #[test]
    fn world_update_plan_warns_for_packages_outside_manifest() {
        let current = SystemManifest {
            packages: vec![Package::new("app-admin/managed").version("1.0.0")],
            ..SystemManifest::default()
        };
        let pretend = parse_pretend_world_update(
            "These are the packages that would be merged, in order:\n[ebuild U] app-admin/unmanaged [2.0.0] [1.0.0]\n",
        )
        .unwrap();

        let plan = build_world_update_plan(Some(&current), pretend, false);

        assert_eq!(
            plan.warnings,
            vec![WorldUpdateWarning::NotInManifest {
                package: "app-admin/unmanaged".to_owned()
            }]
        );
        assert!(!plan.sync_ran);
    }

    #[test]
    fn update_manifest_overlays_pretend_versions_and_install_source() {
        let current = SystemManifest {
            prefer_binary: true,
            packages: vec![
                Package::new("sys-devel/gcc").version("14.3.0"),
                Package::new("app-misc/binconsumer").binary(true),
            ],
            ..SystemManifest::default()
        };
        let pretend = parse_pretend_world_update(
            "These are the packages that would be merged, in order:\n[ebuild U] sys-devel/gcc [15.1.1] [14.3.0]\n",
        )
        .unwrap();

        let manifest = manifest_for_update_preflight(&current, &pretend);
        let gcc = manifest
            .packages
            .iter()
            .find(|package| package.package == "sys-devel/gcc")
            .unwrap();

        assert_eq!(gcc.version.as_deref(), Some("15.1.1"));
        assert!(gcc.from_source);
        assert!(!gcc.binary);
        assert!(
            manifest
                .packages
                .iter()
                .any(|package| package.package == "app-misc/binconsumer" && package.binary)
        );
    }

    #[test]
    fn uninstall_rows_remove_packages_from_preflight_manifest() {
        let current = SystemManifest {
            packages: vec![Package::new("app-misc/old").version("1.0.0")],
            ..SystemManifest::default()
        };
        let pretend = parse_pretend_world_update(
            "These are the packages that would be merged, in order:\n[uninstall ] app-misc/old [1.0.0]\n",
        )
        .unwrap();

        assert_eq!(pretend[0].operation, PretendOperation::Uninstall);
        let manifest = manifest_for_update_preflight(&current, &pretend);
        assert!(manifest.packages.is_empty());
    }
}
