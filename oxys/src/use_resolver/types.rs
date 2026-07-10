use std::collections::HashMap;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::manifest::SystemManifest;
/// Parsed md5-cache metadata for a specific package version.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PackageMetadata {
    /// Fully-qualified package atom, for example `gui-wm/niri`.
    pub package: String,
    /// Package version derived from the md5-cache filename.
    pub version: String,
    /// Declared USE flags from `IUSE`.
    pub iuse: Vec<UseFlag>,
    /// Simplified parsed `DEPEND` atoms.
    pub depend: Vec<ConditionalDep>,
    /// Simplified parsed `BDEPEND` atoms.
    pub bdepend: Vec<ConditionalDep>,
    /// Simplified parsed `RDEPEND` atoms.
    pub rdepend: Vec<ConditionalDep>,
    /// Simplified parsed `PDEPEND` atoms.
    pub pdepend: Vec<ConditionalDep>,
    /// Parsed `REQUIRED_USE` constraints.
    pub required_use: Vec<RequiredUseExpr>,
    /// Raw `KEYWORDS` tokens such as `amd64` or `~amd64`.
    pub keywords: Vec<String>,
    /// Package license tokens from `LICENSE`.
    pub licenses: Vec<String>,
    /// Package properties from `PROPERTIES`.
    pub properties: Vec<String>,
    /// Package restrictions from `RESTRICT`.
    pub restrict: Vec<String>,
    /// Virtuals or atoms exported by this package's `PROVIDE`.
    pub provides: Vec<String>,
    /// Parsed `SLOT`, if present.
    pub slot: Option<String>,
    /// Parsed `subslot` from `SLOT`, if present.
    pub subslot: Option<String>,
    /// Timestamp recorded when the metadata was parsed or refreshed into cache.
    pub cached_at: DateTime<Utc>,
}

/// A single USE flag declaration from `IUSE`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UseFlag {
    /// The normalized USE flag name.
    pub name: String,
    /// Whether the flag defaults to enabled (`+flag` in `IUSE`).
    pub default_enabled: bool,
}

/// Blocker strength declared by a dependency atom.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum BlockerKind {
    /// `!category/pkg`
    Soft,
    /// `!!category/pkg`
    Hard,
}

/// A simplified dependency atom optionally guarded by a USE condition.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConditionalDep {
    /// The controlling USE flag when the dependency is conditional.
    pub condition: Option<String>,
    /// The dependency package atom stripped down to `category/package`.
    pub package: String,
    /// The blocker marker attached to this dependency atom, if any.
    pub blocker: Option<BlockerKind>,
    /// Slot requested by the atom, excluding any subslot/operator suffix.
    pub slot: Option<String>,
    /// Subslot requested by the atom, when specified.
    pub subslot: Option<String>,
    /// Slot operator semantics attached to the atom.
    pub slot_operator: Option<SlotOperator>,
}

/// Slot operator semantics attached to a dependency atom.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum SlotOperator {
    /// `:*`
    Any,
    /// `:=`
    Equal,
}

/// Parsed REQUIRED_USE constraint expressions.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum RequiredUseExpr {
    /// `flag`
    Flag(String),
    /// `!flag`
    Not(String),
    /// `|| ( ... )`
    AnyOf(Vec<RequiredUseExpr>),
    /// `^^ ( ... )`
    ExactlyOne(Vec<RequiredUseExpr>),
    /// `?? ( ... )`
    AtMostOne(Vec<RequiredUseExpr>),
    /// `flag? ( ... )`
    IfThen(String, Vec<RequiredUseExpr>),
    /// Bare `( ... )`
    AllOf(Vec<RequiredUseExpr>),
}

/// A resolver conflict that could not be auto-resolved safely.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Conflict {
    /// Packages participating in the conflict.
    pub packages: Vec<String>,
    /// Logical flag or rule name associated with the conflict.
    pub flag: String,
    /// Human-readable explanation for the conflict.
    pub reason: String,
}

/// Non-fatal resolver information that should be surfaced to the caller.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Warning {
    /// Package associated with the warning.
    pub package: String,
    /// Human-readable warning message.
    pub message: String,
}

/// The Portage planner action performed by a resolver decision.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DecisionAction {
    Enable,
    Disable,
    Add,
    Note,
}

/// The data source that justified a resolver decision.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DecisionSource {
    ManifestPolicy,
    PackageInference,
    Metadata,
}

/// The Portage planner area affected by a resolver decision.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DecisionScope {
    PackageUse,
    GlobalUse,
    AcceptKeywords,
    PlannerPolicy,
}

/// A deterministic explanation record for why the planner made a change.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PortageDecision {
    /// The affected planner area.
    pub scope: DecisionScope,
    /// Package associated with the decision when applicable.
    pub package: Option<String>,
    /// The subject being changed, for example `wayland` or `~amd64`.
    pub subject: String,
    /// The action taken for the subject.
    pub action: DecisionAction,
    /// The origin of the decision.
    pub source: DecisionSource,
    /// Human-readable explanation for the decision.
    pub reason: String,
}

/// Final USE resolution output ready for Portage config generation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UseResolution {
    /// Final per-package USE settings.
    pub package_use: HashMap<String, Vec<String>>,
    /// Final global USE settings.
    pub global_use: Vec<String>,
    /// Additional keyword acceptance entries required by the selection.
    pub accept_keywords: Vec<String>,
    /// Additional license acceptance entries required by the selection.
    pub accept_licenses: Vec<String>,
    /// Conflicts that need user or higher-level policy intervention.
    pub conflicts: Vec<Conflict>,
    /// Non-fatal warnings emitted while resolving.
    pub warnings: Vec<Warning>,
    /// Deterministic planner decisions for diff/apply explanations.
    pub decisions: Vec<PortageDecision>,
}

/// Thin apply-facing Portage execution plan.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PortagePlan {
    /// Exact package atoms to pass to `emerge`.
    pub targets: Vec<String>,
    /// Resolved manifest used to generate apply-facing Portage configuration.
    pub manifest: SystemManifest,
    /// Planned USE and keyword configuration derived from the manifest and metadata.
    pub resolution: UseResolution,
    /// Whether this plan will pass --getbinpkg/--usepkg to emerge (any package resolved to binary).
    pub use_binpkgs: bool,
}
