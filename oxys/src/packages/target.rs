use std::path::Path;

use super::{PackageError, Result, format::TargetMetadata};

pub(crate) fn validate(root: &Path, target: &TargetMetadata) -> Result<()> {
    let artifact_arch = target.triple.split('-').next().unwrap_or_default();
    let host_arch = std::env::consts::ARCH;
    if normalize_arch(artifact_arch) != normalize_arch(host_arch) {
        return Err(PackageError::invalid(format!(
            "artifact architecture {artifact_arch:?} is incompatible with {host_arch:?}"
        )));
    }
    match target.libc.as_str() {
        "glibc" if !cfg!(target_env = "gnu") => return incompatible("glibc"),
        "musl" if !cfg!(target_env = "musl") => return incompatible("musl"),
        "glibc" | "musl" => {}
        other => {
            return Err(PackageError::invalid(format!(
                "unsupported target libc {other:?}"
            )));
        }
    }
    validate_cpu(&target.cpu)?;

    let detected_init = if root.join("etc/init.d").is_dir() || root.join("sbin/openrc-run").exists()
    {
        Some("openrc")
    } else if root.join("run/systemd/system").is_dir()
        || root.join("usr/lib/systemd/systemd").exists()
    {
        Some("systemd")
    } else {
        None
    };
    if let Some(actual) = detected_init
        && target.init != actual
    {
        return Err(PackageError::invalid(format!(
            "artifact init system {:?} is incompatible with target {actual:?}",
            target.init
        )));
    }
    Ok(())
}

fn incompatible(libc: &str) -> Result<()> {
    Err(PackageError::invalid(format!(
        "artifact libc {libc:?} is incompatible with this installer"
    )))
}

fn normalize_arch(value: &str) -> &str {
    match value {
        "amd64" => "x86_64",
        "arm64" => "aarch64",
        other => other,
    }
}

fn validate_cpu(cpu: &str) -> Result<()> {
    #[cfg(target_arch = "x86_64")]
    {
        let supported = match cpu {
            "x86-64" | "x86_64" | "generic" => true,
            "x86-64-v2" => supports_x86_64_v2(),
            "x86-64-v3" => supports_x86_64_v2() && supports_x86_64_v3(),
            "x86-64-v4" => supports_x86_64_v2() && supports_x86_64_v3() && supports_x86_64_v4(),
            "native" | "unknown" => false,
            _ => false,
        };
        if !supported {
            return Err(PackageError::invalid(format!(
                "artifact CPU baseline {cpu:?} is unsupported or unavailable"
            )));
        }
    }
    #[cfg(not(target_arch = "x86_64"))]
    if !matches!(cpu, "generic" | "unknown") && cpu != std::env::consts::ARCH {
        return Err(PackageError::invalid(format!(
            "artifact CPU baseline {cpu:?} is unsupported"
        )));
    }
    Ok(())
}

#[cfg(target_arch = "x86_64")]
fn supports_x86_64_v2() -> bool {
    std::arch::is_x86_feature_detected!("sse3")
        && std::arch::is_x86_feature_detected!("ssse3")
        && std::arch::is_x86_feature_detected!("sse4.1")
        && std::arch::is_x86_feature_detected!("sse4.2")
        && std::arch::is_x86_feature_detected!("popcnt")
}

#[cfg(target_arch = "x86_64")]
fn supports_x86_64_v3() -> bool {
    std::arch::is_x86_feature_detected!("avx")
        && std::arch::is_x86_feature_detected!("avx2")
        && std::arch::is_x86_feature_detected!("bmi1")
        && std::arch::is_x86_feature_detected!("bmi2")
        && std::arch::is_x86_feature_detected!("f16c")
        && std::arch::is_x86_feature_detected!("fma")
        && std::arch::is_x86_feature_detected!("lzcnt")
        && std::arch::is_x86_feature_detected!("movbe")
        && std::arch::is_x86_feature_detected!("xsave")
}

#[cfg(target_arch = "x86_64")]
fn supports_x86_64_v4() -> bool {
    std::arch::is_x86_feature_detected!("avx512f")
        && std::arch::is_x86_feature_detected!("avx512bw")
        && std::arch::is_x86_feature_detected!("avx512cd")
        && std::arch::is_x86_feature_detected!("avx512dq")
        && std::arch::is_x86_feature_detected!("avx512vl")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn metadata() -> TargetMetadata {
        TargetMetadata {
            triple: format!("{}-pc-linux-gnu", std::env::consts::ARCH),
            cpu: if cfg!(target_arch = "x86_64") {
                "x86-64".into()
            } else {
                "generic".into()
            },
            libc: if cfg!(target_env = "musl") {
                "musl".into()
            } else {
                "glibc".into()
            },
            libc_min: "unknown".into(),
            init: "openrc".into(),
        }
    }

    #[test]
    fn accepts_compatible_target() {
        let root = tempfile::tempdir().unwrap();
        validate(root.path(), &metadata()).unwrap();
    }

    #[test]
    fn rejects_wrong_architecture_and_detected_init() {
        let root = tempfile::tempdir().unwrap();
        let mut target = metadata();
        target.triple = "definitely-wrong-linux-gnu".into();
        assert!(validate(root.path(), &target).is_err());

        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(root.path().join("run/systemd/system")).unwrap();
        assert!(validate(root.path(), &metadata()).is_err());
    }
}
