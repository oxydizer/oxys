use serde::{Deserialize, Serialize};

use crate::detect::detect_cpu_count;

/// Controls the tradeoff between compile time and runtime performance.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BuildOptimisation {
    /// Fastest installs: mold linker, PGO disabled everywhere.
    Fast,
    /// Recommended default: mold linker, PGO only for known high-value packages.
    Balanced,
    /// Maximum runtime performance: mold linker, PGO enabled everywhere.
    Performance,
}

impl Default for BuildOptimisation {
    fn default() -> Self {
        Self::Balanced
    }
}

/// Target CPU microarchitecture level, compiled into `-march=` in make.conf.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum March {
    /// `-march=native` — tune for the machine performing the build.
    Native,
    /// `-march=x86-64` — baseline, maximum compatibility.
    X86_64,
    /// `-march=x86-64-v2` — SSE4.2-era CPUs and newer.
    X86_64V2,
    /// `-march=x86-64-v3` — AVX2-era CPUs and newer.
    X86_64V3,
    /// `-march=x86-64-v4` — AVX-512-era CPUs and newer.
    X86_64V4,
}

impl Default for March {
    fn default() -> Self {
        // x86-64-v3 is the widest baseline Gentoo's official binhost ships as
        // stable, so binary packages are available for it out of the box.
        // `Native` opts out of the binhost entirely (see `binhost_url`) and
        // forces everything to build from source.
        Self::X86_64V3
    }
}

impl March {
    /// The `-march=` value passed to the compiler.
    pub fn value(&self) -> &'static str {
        match self {
            March::Native => "native",
            March::X86_64 => "x86-64",
            March::X86_64V2 => "x86-64-v2",
            March::X86_64V3 => "x86-64-v3",
            March::X86_64V4 => "x86-64-v4",
        }
    }

    /// The full `-march=<value>` compiler flag.
    pub fn flag(&self) -> String {
        format!("-march={}", self.value())
    }

    /// Gentoo's official binhost URI for this baseline, or `None` for
    /// `Native` — there is no official binhost for a machine-specific march,
    /// so only local compilation makes sense.
    pub fn binhost_url(&self) -> Option<String> {
        let path = match self {
            March::Native => return None,
            March::X86_64 => "x86-64",
            March::X86_64V2 => "x86-64-v2",
            March::X86_64V3 => "x86-64-v3",
            March::X86_64V4 => "x86-64-v4",
        };
        Some(format!(
            "https://distfiles.gentoo.org/releases/amd64/binpackages/23.0/{path}/"
        ))
    }
}

/// Compiler and build system configuration — maps to /etc/portage/make.conf
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Compiler {
    /// Base C compiler flags, before `-march`. Default: "-O2 -pipe"
    pub cflags: String,
    /// Base C++ compiler flags, before `-march`. Mirrors CFLAGS by default.
    pub cxxflags: String,
    /// Target CPU microarchitecture level, appended as `-march=` to
    /// CFLAGS/CXXFLAGS. Default: `March::X86_64V3`.
    pub march: March,
    /// Binary package host queried with `--getbinpkg` before building from
    /// source. `None` disables binpkg fetching entirely. Default: Gentoo's
    /// official binhost matching `march`.
    pub binhost: Option<String>,
    /// Linker flags. Default includes "-fuse-ld=mold"
    pub ldflags: String,
    /// Number of parallel make jobs. Default: number of logical CPUs.
    pub makeopts_jobs: usize,
    /// Number of parallel emerge jobs. Default: 2
    pub emerge_jobs: usize,
    /// Enable ccache for faster rebuilds.
    pub ccache: bool,
    /// PGO / optimisation strategy.
    pub optimisation: BuildOptimisation,
}

impl Default for Compiler {
    fn default() -> Self {
        let jobs = detect_cpu_count();
        let march = March::default();
        Self {
            cflags: "-O2 -pipe".to_owned(),
            cxxflags: "-O2 -pipe".to_owned(),
            binhost: march.binhost_url(),
            march,
            ldflags: "-fuse-ld=mold".to_owned(),
            makeopts_jobs: jobs,
            emerge_jobs: 2,
            ccache: true,
            optimisation: BuildOptimisation::default(),
        }
    }
}

impl Compiler {
    /// Full CFLAGS with the configured `-march` appended (any `-march` already
    /// present in the base flags is dropped so `march` stays authoritative).
    pub fn resolved_cflags(&self) -> String {
        compose_march(&self.cflags, &self.march)
    }

    /// Full CXXFLAGS with the configured `-march` appended.
    pub fn resolved_cxxflags(&self) -> String {
        compose_march(&self.cxxflags, &self.march)
    }
}

/// Strips any existing `-march=` token from `base` and appends the flag for
/// `march`, keeping the manifest's `march` field the single source of truth.
fn compose_march(base: &str, march: &March) -> String {
    let mut parts: Vec<&str> = base
        .split_whitespace()
        .filter(|token| !token.starts_with("-march="))
        .collect();
    let flag = march.flag();
    parts.push(&flag);
    parts.join(" ")
}
