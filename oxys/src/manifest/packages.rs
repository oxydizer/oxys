use serde::{Deserialize, Serialize};

use super::{AudioStack, DisplayStack, InitSystem, Libc, SystemManifest};

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Package {
    pub package: String,
    #[serde(default)]
    pub version: Option<String>,
    #[serde(default)]
    pub use_flags: Vec<String>,
    #[serde(default)]
    pub keywords: Vec<String>,
    #[serde(default)]
    pub accept_licenses: Vec<String>,
    #[serde(default)]
    pub binary: bool,
    #[serde(default)]
    pub from_source: bool,
}

impl Package {
    pub fn new(atom: impl Into<String>) -> Self {
        Self {
            package: atom.into(),
            version: None,
            use_flags: Vec::new(),
            keywords: Vec::new(),
            accept_licenses: Vec::new(),
            binary: false,
            from_source: false,
        }
    }

    pub fn binary(mut self, binary: bool) -> Self {
        self.binary = binary;
        self
    }

    /// Force this package to build from source, even if the global
    /// `prefer_binary` default would otherwise select a binary package.
    /// This is required (and the only way) to apply custom `use_flags()`.
    pub fn from_source(mut self) -> Self {
        self.from_source = true;
        self
    }

    pub fn version(mut self, version: impl Into<String>) -> Self {
        self.version = Some(version.into());
        self
    }

    pub fn use_flags<I, S>(mut self, flags: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.use_flags = flags.into_iter().map(Into::into).collect();
        self
    }

    pub fn keywords<I, S>(mut self, keywords: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.keywords = keywords.into_iter().map(Into::into).collect();
        self
    }

    pub fn accept_licenses<I, S>(mut self, licenses: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.accept_licenses = licenses.into_iter().map(Into::into).collect();
        self
    }
}

impl From<&str> for Package {
    fn from(value: &str) -> Self {
        Self::new(value)
    }
}

impl From<String> for Package {
    fn from(value: String) -> Self {
        Self::new(value)
    }
}

/// Internal planner manifest with exact package selections and derived policy hints.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub(crate) struct PlannerManifest {
    #[serde(default)]
    pub packages: Vec<ManifestPackage>,
    pub init_system: Option<InitSystem>,
    pub libc: Option<Libc>,
    pub prefer_binary: bool,
    pub display_stack: Option<DisplayStack>,
    pub audio_stack: Option<AudioStack>,
}

/// Internal exact package request for Portage planning.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ManifestPackage {
    pub package: String,
    #[serde(default)]
    pub version: Option<String>,
    #[serde(default)]
    pub use_flags: Vec<String>,
    #[serde(default)]
    pub keywords: Vec<String>,
    #[serde(default)]
    pub accept_licenses: Vec<String>,
    #[serde(default)]
    pub binary: bool,
    #[serde(default)]
    pub from_source: bool,
}

impl ManifestPackage {
    pub fn versioned_atom(&self, version: &str) -> String {
        format!("={}-{}", self.package, version)
    }
}

impl From<Package> for ManifestPackage {
    fn from(value: Package) -> Self {
        let auto_binary = value
            .package
            .rsplit('/')
            .next()
            .is_some_and(|name| name.ends_with("-bin"));
        let binary = value.binary || auto_binary;
        Self {
            package: value.package,
            version: value.version,
            use_flags: value.use_flags,
            keywords: value.keywords,
            accept_licenses: value.accept_licenses,
            binary,
            from_source: value.from_source,
        }
    }
}

impl From<SystemManifest> for PlannerManifest {
    fn from(value: SystemManifest) -> Self {
        Self {
            packages: value
                .packages
                .into_iter()
                .map(ManifestPackage::from)
                .collect(),
            init_system: Some(value.init_system),
            libc: Some(value.os.libc),
            prefer_binary: value.prefer_binary,
            display_stack: value.display_stack,
            audio_stack: value.audio_stack,
        }
    }
}
