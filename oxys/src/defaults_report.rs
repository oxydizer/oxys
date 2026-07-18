//! Report which notable settings of a compiled manifest are the built-in
//! defaults, so opinionated values (`-fuse-ld=mold`, ccache, the binhost
//! march, ...) are never applied invisibly.
//!
//! Semantics are deliberately "equals the default", not "was omitted in the
//! config": a config that spells out the default value reads as default here,
//! which is fine — the point is showing the effective value.

use crate::manifest::{
    Bootloader, GB, LoginFrontend, MB, SwapConfig, SwapSize, SwapStrategy, SystemManifest,
};

/// One manifest setting whose effective value equals the built-in default.
#[derive(Debug, PartialEq, Eq)]
pub struct DefaultedSetting {
    /// Dotted path of the field, e.g. `compiler.ldflags`.
    pub path: &'static str,
    /// Human-readable effective value.
    pub value: String,
}

/// The notable settings of `manifest` that carry their default values.
///
/// Curated to the opinionated hand-written defaults; empty strings and empty
/// lists are not worth reporting.
pub fn defaulted_settings(manifest: &SystemManifest) -> Vec<DefaultedSetting> {
    let baseline = SystemManifest::default();
    let mut report = Vec::new();
    let mut note = |path: &'static str, is_default: bool, value: String| {
        if is_default {
            report.push(DefaultedSetting { path, value });
        }
    };

    let compiler = &manifest.compiler;
    let base = &baseline.compiler;
    note(
        "compiler.cflags",
        compiler.cflags == base.cflags,
        format!("\"{}\"", compiler.cflags),
    );
    note(
        "compiler.cxxflags",
        compiler.cxxflags == base.cxxflags,
        format!("\"{}\"", compiler.cxxflags),
    );
    note(
        "compiler.march",
        compiler.march == base.march,
        compiler.march.value().to_owned(),
    );
    note(
        "compiler.binhost",
        compiler.binhost == base.binhost,
        compiler
            .binhost
            .clone()
            .unwrap_or_else(|| "disabled".to_owned()),
    );
    note(
        "compiler.ldflags",
        compiler.ldflags == base.ldflags,
        format!("\"{}\"", compiler.ldflags),
    );
    note(
        "compiler.makeopts_jobs",
        compiler.makeopts_jobs == base.makeopts_jobs,
        format!("{} (detected CPU count)", compiler.makeopts_jobs),
    );
    note(
        "compiler.emerge_jobs",
        compiler.emerge_jobs == base.emerge_jobs,
        compiler.emerge_jobs.to_string(),
    );
    note(
        "compiler.ccache",
        compiler.ccache == base.ccache,
        compiler.ccache.to_string(),
    );
    note(
        "compiler.optimisation",
        compiler.optimisation == base.optimisation,
        lowercase_debug(&compiler.optimisation),
    );

    note(
        "init_system",
        manifest.init_system == baseline.init_system,
        lowercase_debug(&manifest.init_system),
    );
    // `None` means "the resolver applies the default bootloader", so both the
    // unset and the explicitly-default case report the same effective value.
    note(
        "bootloader",
        manifest.resolved_bootloader() == Bootloader::default(),
        lowercase_debug(&manifest.resolved_bootloader()),
    );
    note(
        "os.shell",
        manifest.os.shell == baseline.os.shell,
        lowercase_debug(&manifest.os.shell),
    );

    let session = &manifest.session;
    let base_session = &baseline.session;
    note(
        "session.mode",
        session.mode == base_session.mode,
        lowercase_debug(&session.mode),
    );
    note(
        "session.login",
        session.login == base_session.login,
        render_login(&session.login),
    );
    note(
        "session.compositor",
        session.compositor == base_session.compositor,
        lowercase_debug(&session.compositor),
    );
    note(
        "session.seat",
        session.seat == base_session.seat,
        lowercase_debug(&session.seat),
    );
    note(
        "session.session_tracker",
        session.session_tracker == base_session.session_tracker,
        lowercase_debug(&session.session_tracker),
    );

    note(
        "disk.layout",
        manifest.disk.layout == baseline.disk.layout,
        lowercase_debug(&manifest.disk.layout),
    );
    note(
        "swap",
        manifest.disk.partitions.swap.is_unspecified() && manifest.swap == baseline.swap,
        if manifest.disk.partitions.swap.is_unspecified() {
            render_swap(&manifest.swap.strategy)
        } else {
            render_legacy_swap(&manifest.disk.partitions.swap)
        },
    );

    report
}

/// Render report lines like `compiler.ldflags = "-fuse-ld=mold" (default)`.
pub fn render_defaults_report(settings: &[DefaultedSetting]) -> Vec<String> {
    settings
        .iter()
        .map(|setting| format!("{} = {} (default)", setting.path, setting.value))
        .collect()
}

fn lowercase_debug(value: &impl std::fmt::Debug) -> String {
    format!("{value:?}").to_lowercase()
}

fn render_login(login: &LoginFrontend) -> String {
    match login {
        LoginFrontend::Tty { tty } => format!("tty (tty {tty})"),
        LoginFrontend::OxysLogin {
            tty,
            fallback_tty_login,
        } => format!("oxys_login (tty {tty}, fallback_tty_login {fallback_tty_login})"),
    }
}

fn render_legacy_swap(swap: &SwapConfig) -> String {
    match swap {
        SwapConfig::Unspecified => "top-level policy".to_owned(),
        SwapConfig::Partition { size } => format!("partition ({})", human_size(*size)),
        SwapConfig::File { size } => format!("file ({})", human_size(*size)),
        SwapConfig::Zram { size } => format!("zram ({})", human_size(*size)),
        SwapConfig::None => "none".to_owned(),
    }
}

fn render_swap(strategy: &SwapStrategy) -> String {
    match strategy {
        SwapStrategy::Disk { size } => format!("disk ({})", render_swap_size(size)),
        SwapStrategy::Hybrid { zram, disk } => format!(
            "hybrid (zram {}/{}, {}, disk {})",
            zram.fraction.numerator,
            zram.fraction.denominator,
            zram.algorithm.kernel_name(),
            render_swap_size(&disk.size)
        ),
        SwapStrategy::ZramOnly {
            algorithm,
            fraction,
        } => format!(
            "zram-only ({}/{}, {})",
            fraction.numerator,
            fraction.denominator,
            algorithm.kernel_name()
        ),
        SwapStrategy::Disabled => "disabled".to_owned(),
    }
}

fn render_swap_size(size: &SwapSize) -> String {
    match size {
        SwapSize::MatchRam => "match RAM".to_owned(),
        SwapSize::Fixed(bytes) => human_size(*bytes),
    }
}

fn human_size(bytes: u64) -> String {
    if bytes >= GB && bytes % GB == 0 {
        format!("{} GiB", bytes / GB)
    } else if bytes >= MB {
        format!("{} MiB", bytes / MB)
    } else {
        format!("{bytes} B")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manifest::{Compiler, InitSystem};

    fn paths(settings: &[DefaultedSetting]) -> Vec<&'static str> {
        settings.iter().map(|setting| setting.path).collect()
    }

    #[test]
    fn default_manifest_reports_the_notable_set() {
        let report = defaulted_settings(&SystemManifest::default());
        let paths = paths(&report);
        for expected in [
            "compiler.cflags",
            "compiler.march",
            "compiler.binhost",
            "compiler.ldflags",
            "compiler.makeopts_jobs",
            "compiler.emerge_jobs",
            "compiler.ccache",
            "compiler.optimisation",
            "init_system",
            "bootloader",
            "os.shell",
            "session.mode",
            "session.login",
            "session.compositor",
            "disk.layout",
            "swap",
        ] {
            assert!(paths.contains(&expected), "missing {expected} in {paths:?}");
        }
    }

    #[test]
    fn overridden_values_drop_out() {
        let manifest = SystemManifest {
            compiler: Compiler {
                ldflags: "-fuse-ld=lld".to_owned(),
                ..Compiler::default()
            },
            init_system: InitSystem::Systemd,
            ..SystemManifest::default()
        };
        let report = defaulted_settings(&manifest);
        let paths = paths(&report);
        assert!(!paths.contains(&"compiler.ldflags"));
        assert!(!paths.contains(&"init_system"));
        assert!(paths.contains(&"compiler.ccache"));
    }

    #[test]
    fn explicit_grub_still_reports_as_default_bootloader() {
        let manifest = SystemManifest {
            bootloader: Some(Bootloader::Grub),
            ..SystemManifest::default()
        };
        assert!(paths(&defaulted_settings(&manifest)).contains(&"bootloader"));
        let manifest = SystemManifest {
            bootloader: Some(Bootloader::SystemdBoot),
            ..SystemManifest::default()
        };
        assert!(!paths(&defaulted_settings(&manifest)).contains(&"bootloader"));
    }

    #[test]
    fn report_lines_carry_value_and_marker() {
        let report = defaulted_settings(&SystemManifest::default());
        let lines = render_defaults_report(&report);
        assert!(
            lines
                .iter()
                .any(|line| line == "compiler.ldflags = \"-fuse-ld=mold\" (default)")
        );
    }
}
