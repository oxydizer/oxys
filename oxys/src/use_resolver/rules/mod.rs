use std::collections::BTreeSet;

use super::{
    BlockerKind, ConditionalDep, Conflict, DecisionAction, DecisionScope, DecisionSource,
    PackageMetadata, PortageDecision, RequiredUseExpr, SlotOperator, Warning,
};

use super::resolver::{PackageState, is_l10n_flag, parse_manifest_flag};

mod binary;
mod dependencies;
mod metadata;
mod policy;
mod required_use;

pub use policy::{
    apply_audio_rule, apply_global_policy_rules, apply_init_system_rule, apply_libc_rule,
    apply_llvm_slot_rule, apply_wayland_x_rule, record_explicit_policy_notes,
};

pub use binary::collect_use_flags_binary_conflicts;
use binary::package_decision;
pub use dependencies::{
    apply_abi_consistency_rule, apply_blocker_rule, apply_required_use_rule,
    apply_slot_dependency_rule, apply_virtual_rule,
};
pub use metadata::{
    collect_keyword_acceptance, collect_license_acceptance, collect_local_conflicts,
    collect_metadata_warnings,
};
use required_use::{
    explain_required_use_violation, referenced_required_use_flags, render_required_use_expr,
};
