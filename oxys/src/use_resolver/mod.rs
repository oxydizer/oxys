mod cache;
mod emerge;
mod error;
mod generate;
mod parse;
mod path;
mod repo;
mod resolver;
mod rules;
mod types;
mod update;
mod util;
mod version;

pub use crate::manifest::{
    AudioStack, DisplayStack, InitSystem, Libc, ManifestPackage, SystemManifest,
};
/// Cache helpers for parsed package metadata.
pub use cache::{cache_path_for_metadata, load_or_parse_metadata, sync};
/// Streams structured events from an `emerge` subprocess.
pub use emerge::{
    emerge_chroot_command_for_test, emerge_command_for_test, run_emerge, run_emerge_chroot,
    EmergeLine, EmergeStream,
};
/// Error type returned by all `use_resolver` operations.
pub use error::UseResolverError;
/// Writes generated Portage configuration files.
pub use generate::{
    generate_make_conf, gpu_to_video_cards, should_enable_pgo, write_portage_config,
    write_portage_plan_config, MakeConfOutput,
};
/// Extracts a `category/package` atom and version from an md5-cache file path.
pub use path::package_from_md5_cache_path;
/// Resolves Portage state from manifest input and offline metadata.
pub use resolver::{apply_portage_plan, plan_portage, resolve, resolve_latest_version};
/// Public data model used by metadata loading, planning, config generation, and emerge execution.
pub use types::{
    BlockerKind, ConditionalDep, Conflict, DecisionAction, DecisionScope, DecisionSource,
    PackageMetadata, PortageDecision, PortagePlan, RequiredUseExpr, SlotOperator, UseFlag,
    UseResolution, Warning,
};
/// Parses and plans guarded world updates from Portage pretend output.
pub use update::{
    build_world_update_plan, manifest_for_update_preflight, parse_pretend_world_update,
    plan_update_preflight, PretendOperation, PretendPackage, PretendPackageSource,
    PretendParseError, WorldUpdatePlan, WorldUpdateWarning,
};
