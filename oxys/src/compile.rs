//! Compile a Rust config into a validated `manifest.toml`.
//!
//! A config is a standalone Rust program that ends in `oxys::main!(config)`.
//! To turn it into a manifest we scaffold a tiny crate that depends on the
//! `oxys` crate, drop the user's file in as its `src/main.rs`, and run it with
//! cargo so it writes `manifest.toml`. This is
//! the shared engine behind both the `oxys compile` CLI command and the
//! installer's on-target config validation.

use std::{
    fmt, fs,
    path::{Path, PathBuf},
    process::Command,
};

use crate::{SystemManifest, parse_generated_manifest_toml};

/// Default location of the `oxys` crate source used to compile configs.
pub const DEFAULT_OXYS_CRATE_PATH: &str = "/usr/src/oxys";

const SCAFFOLD_CRATE_NAME: &str = "oxys-config-scaffold";
const LOCAL_MANIFEST: &str = "manifest.toml";

/// Which phase of compilation failed. Used to give the caller (CLI or TUI) a
/// short, human-readable label alongside the captured compiler output.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompileStage {
    /// The `oxys` crate source was not found.
    CrateMissing,
    /// Setting up the scaffold crate on disk failed.
    Scaffold,
    /// `cargo build` of the config failed (syntax/type errors live here).
    CargoBuild,
    /// The compiled config binary failed to run.
    Execute,
    /// The config ran but produced no `manifest.toml`.
    ManifestMissing,
    /// A `manifest.toml` was produced but did not validate.
    ManifestInvalid,
    /// The manifest declares packages that exist in no configured repository.
    UnknownPackages,
}

impl CompileStage {
    pub fn label(self) -> &'static str {
        match self {
            CompileStage::CrateMissing => "oxys crate not found",
            CompileStage::Scaffold => "scaffolding build crate",
            CompileStage::CargoBuild => "compiling config",
            CompileStage::Execute => "executing config",
            CompileStage::ManifestMissing => "manifest.toml not produced",
            CompileStage::ManifestInvalid => "validating manifest.toml",
            CompileStage::UnknownPackages => "unknown packages",
        }
    }
}

/// A structured compilation failure. `output` carries the captured cargo /
/// binary stdout+stderr so a caller can show the real compiler diagnostics.
#[derive(Debug)]
pub struct CompileError {
    pub stage: CompileStage,
    pub message: String,
    pub output: String,
}

impl CompileError {
    /// Construct a bare error with no captured compiler output — for
    /// precondition failures a caller hits before cargo is ever invoked.
    pub fn message(message: impl Into<String>) -> Self {
        Self::new(CompileStage::Scaffold, message)
    }

    fn new(stage: CompileStage, message: impl Into<String>) -> Self {
        Self {
            stage,
            message: message.into(),
            output: String::new(),
        }
    }

    fn with_output(mut self, output: String) -> Self {
        self.output = output;
        self
    }
}

impl fmt::Display for CompileError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.stage.label(), self.message)
    }
}

impl std::error::Error for CompileError {}

/// The `oxys` crate path, honouring the `OXYS_CRATE_PATH` override.
pub fn oxys_crate_path() -> String {
    std::env::var("OXYS_CRATE_PATH").unwrap_or_else(|_| DEFAULT_OXYS_CRATE_PATH.to_string())
}

/// Persistent scaffold build directory. Reused across runs so cargo only
/// recompiles the user's config, not the whole `oxys` dependency tree.
pub fn build_cache_dir() -> PathBuf {
    let base = std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".cache")))
        .unwrap_or_else(std::env::temp_dir);
    base.join("oxys").join("build")
}

/// A successful compilation: where the validated manifest landed, plus any
/// non-fatal notices the caller should surface (e.g. a skipped package check).
#[derive(Debug)]
pub struct CompileOutcome {
    pub manifest_path: PathBuf,
    pub notices: Vec<String>,
}

/// Compile a standalone config `.rs` file into a validated `manifest.toml`.
///
/// Builds the scaffold crate against the `oxys` crate at `oxys_crate_path`,
/// runs the resulting binary with its working directory set to `out_dir` so
/// `manifest.toml` lands there, validates it, and checks every declared
/// package atom against the Portage tree (skipped with a notice when no tree
/// is present). Returns a [`CompileError`] carrying the compiler output on
/// failure. This function blocks (it shells out to cargo); callers on an
/// event loop should run it off-thread.
pub fn compile_config_file(
    file: &Path,
    oxys_crate_path: &str,
    out_dir: &Path,
) -> Result<CompileOutcome, CompileError> {
    compile_config_file_in(
        file,
        oxys_crate_path,
        out_dir,
        &build_cache_dir(),
        &crate::package_check::portage_tree_path(),
    )
}

/// Like [`compile_config_file`] but with an explicit scaffold build directory.
///
/// The build directory is a shared, reused resource (cargo target cache), so a
/// single process must not run two compiles against the same `build_dir`
/// concurrently. The public entry point uses [`build_cache_dir`]; tests inject
/// a private one for isolation.
fn compile_config_file_in(
    file: &Path,
    oxys_crate_path: &str,
    out_dir: &Path,
    build_dir: &Path,
    portage_tree: &Path,
) -> Result<CompileOutcome, CompileError> {
    if !file.exists() {
        return Err(CompileError::new(
            CompileStage::Scaffold,
            format!("config file not found: {}", file.display()),
        ));
    }
    if !Path::new(oxys_crate_path).join("Cargo.toml").exists() {
        return Err(CompileError::new(
            CompileStage::CrateMissing,
            format!("oxys crate not found at {oxys_crate_path} (set OXYS_CRATE_PATH to override)"),
        ));
    }

    let src_dir = build_dir.join("src");
    fs::create_dir_all(&src_dir)
        .map_err(|err| CompileError::new(CompileStage::Scaffold, err.to_string()))?;

    let cargo_toml = format!(
        "[package]\nname = \"{SCAFFOLD_CRATE_NAME}\"\nversion = \"0.0.0\"\nedition = \"2021\"\n\n[dependencies]\noxys = {{ path = {oxys_crate_path:?} }}\n"
    );
    fs::write(build_dir.join("Cargo.toml"), cargo_toml)
        .map_err(|err| CompileError::new(CompileStage::Scaffold, err.to_string()))?;
    fs::copy(file, src_dir.join("main.rs"))
        .map_err(|err| CompileError::new(CompileStage::Scaffold, err.to_string()))?;

    let manifest = fs::canonicalize(build_dir.join("Cargo.toml"))
        .map_err(|err| CompileError::new(CompileStage::Scaffold, err.to_string()))?;
    let run = Command::new("cargo")
        .arg("run")
        .arg("--manifest-path")
        .arg(manifest)
        .arg("--quiet")
        .current_dir(out_dir)
        .output()
        .map_err(|err| {
            CompileError::new(
                CompileStage::CargoBuild,
                format!("failed to run cargo: {err}"),
            )
        })?;
    if !run.status.success() {
        return Err(
            CompileError::new(CompileStage::CargoBuild, "cargo run failed")
                .with_output(combine_output(&run.stdout, &run.stderr)),
        );
    }

    let manifest_path = out_dir.join(LOCAL_MANIFEST);
    if !manifest_path.exists() {
        return Err(CompileError::new(
            CompileStage::ManifestMissing,
            "compilation completed but manifest.toml was not created",
        ));
    }
    let manifest = load_and_validate(&manifest_path)?;

    let mut notices = Vec::new();
    match crate::package_check::check_packages(&manifest, portage_tree) {
        crate::package_check::PackageCheckOutcome::NoPortageTree { tree } => {
            notices.push(format!(
                "note: no Portage tree found at {}; skipping package name check",
                tree.display()
            ));
        }
        crate::package_check::PackageCheckOutcome::Checked(unknown) if !unknown.is_empty() => {
            return Err(CompileError::new(
                CompileStage::UnknownPackages,
                format!("{} unknown package(s) in config", unknown.len()),
            )
            .with_output(crate::package_check::render_unknown_packages(&unknown)));
        }
        crate::package_check::PackageCheckOutcome::Checked(_) => {}
    }

    Ok(CompileOutcome {
        manifest_path,
        notices,
    })
}

/// Read and checksum-validate a generated `manifest.toml`.
pub fn load_manifest(path: &Path) -> Result<SystemManifest, CompileError> {
    load_and_validate(path)
}

fn load_and_validate(path: &Path) -> Result<SystemManifest, CompileError> {
    let text = fs::read_to_string(path)
        .map_err(|err| CompileError::new(CompileStage::ManifestInvalid, err.to_string()))?;
    parse_generated_manifest_toml(&text).map_err(|err| {
        CompileError::new(
            CompileStage::ManifestInvalid,
            format!("{}: {err}", path.display()),
        )
    })
}

fn combine_output(stdout: &[u8], stderr: &[u8]) -> String {
    let mut combined = String::from_utf8_lossy(stdout).into_owned();
    if !stderr.is_empty() {
        if !combined.is_empty() && !combined.ends_with('\n') {
            combined.push('\n');
        }
        combined.push_str(&String::from_utf8_lossy(stderr));
    }
    combined
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn oxys_crate_dir() -> PathBuf {
        // The crate under test lives at <this crate>/; CARGO_MANIFEST_DIR points
        // right at it, which is exactly the path a scaffold should depend on.
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
    }

    fn write_config(dir: &Path, body: &str) -> PathBuf {
        let path = dir.join("config.rs");
        let mut file = fs::File::create(&path).unwrap();
        file.write_all(body.as_bytes()).unwrap();
        path
    }

    /// Build a minimal Portage repo (metadata/md5-cache) with the given atoms.
    fn write_fixture_tree(dir: &Path, atoms: &[&str]) {
        for atom in atoms {
            let (category, name) = atom.split_once('/').unwrap();
            let category_dir = dir.join("metadata").join("md5-cache").join(category);
            fs::create_dir_all(&category_dir).unwrap();
            fs::write(category_dir.join(format!("{name}-1.0")), "").unwrap();
        }
    }

    #[test]
    fn valid_config_compiles_to_a_loadable_manifest() {
        let tmp = tempfile::tempdir().unwrap();
        let config = write_config(
            tmp.path(),
            r#"use oxys::prelude::*;
fn config() -> Oxys { Oxys::default() }
oxys::main!(config);
"#,
        );
        let out = tempfile::tempdir().unwrap();
        let build = tempfile::tempdir().unwrap();
        let tree = tempfile::tempdir().unwrap();
        let outcome = compile_config_file_in(
            &config,
            oxys_crate_dir().to_str().unwrap(),
            out.path(),
            build.path(),
            tree.path(),
        )
        .expect("valid config should compile");
        assert!(outcome.manifest_path.exists());
        load_manifest(&outcome.manifest_path).expect("produced manifest should validate");
        // The empty temp dir holds no repository: the package check is
        // skipped and reported as a notice, not a failure.
        assert_eq!(outcome.notices.len(), 1);
        assert!(outcome.notices[0].contains("skipping package name check"));
    }

    #[test]
    fn broken_config_returns_error_with_output() {
        let tmp = tempfile::tempdir().unwrap();
        let config = write_config(tmp.path(), "this is not valid rust at all;\n");
        let out = tempfile::tempdir().unwrap();
        let build = tempfile::tempdir().unwrap();
        let tree = tempfile::tempdir().unwrap();
        let err = compile_config_file_in(
            &config,
            oxys_crate_dir().to_str().unwrap(),
            out.path(),
            build.path(),
            tree.path(),
        )
        .expect_err("broken config should fail");
        assert_eq!(err.stage, CompileStage::CargoBuild);
        assert!(!err.output.is_empty(), "compiler output should be captured");
    }

    #[test]
    fn attribute_style_config_compiles_without_spreads_or_main() {
        let tmp = tempfile::tempdir().unwrap();
        let config = write_config(
            tmp.path(),
            r#"use oxys::prelude::*;

#[oxys::config]
pub fn config() -> Oxys {
    Oxys {
        os: Os {
            hostname: "test-host".into(),
        },
        packages: vec![Package::new("net-misc/curl")],
    }
}
"#,
        );
        let out = tempfile::tempdir().unwrap();
        let build = tempfile::tempdir().unwrap();
        let tree = tempfile::tempdir().unwrap();
        write_fixture_tree(tree.path(), &["net-misc/curl"]);
        let outcome = compile_config_file_in(
            &config,
            oxys_crate_dir().to_str().unwrap(),
            out.path(),
            build.path(),
            tree.path(),
        )
        .expect("attribute-style config should compile");
        let manifest =
            load_manifest(&outcome.manifest_path).expect("produced manifest should validate");
        assert_eq!(manifest.os.hostname, "test-host");
        assert_eq!(manifest.packages.len(), 1);
    }

    #[test]
    fn unknown_package_fails_with_suggestion() {
        let tmp = tempfile::tempdir().unwrap();
        let config = write_config(
            tmp.path(),
            r#"use oxys::prelude::*;
fn config() -> Oxys {
    Oxys {
        packages: vec![Package::new("gui-apps/wl-clipbord")],
        ..Default::default()
    }
}
oxys::main!(config);
"#,
        );
        let out = tempfile::tempdir().unwrap();
        let build = tempfile::tempdir().unwrap();
        let tree = tempfile::tempdir().unwrap();
        write_fixture_tree(tree.path(), &["gui-apps/wl-clipboard", "net-misc/curl"]);
        let err = compile_config_file_in(
            &config,
            oxys_crate_dir().to_str().unwrap(),
            out.path(),
            build.path(),
            tree.path(),
        )
        .expect_err("unknown package should fail the compile");
        assert_eq!(err.stage, CompileStage::UnknownPackages);
        assert!(
            err.output.contains("did you mean 'gui-apps/wl-clipboard'"),
            "output should carry the suggestion, got: {}",
            err.output
        );
    }

    #[test]
    fn known_packages_pass_the_check() {
        let tmp = tempfile::tempdir().unwrap();
        let config = write_config(
            tmp.path(),
            r#"use oxys::prelude::*;
fn config() -> Oxys {
    Oxys {
        packages: vec![Package::new("net-misc/curl")],
        ..Default::default()
    }
}
oxys::main!(config);
"#,
        );
        let out = tempfile::tempdir().unwrap();
        let build = tempfile::tempdir().unwrap();
        let tree = tempfile::tempdir().unwrap();
        write_fixture_tree(tree.path(), &["net-misc/curl"]);
        let outcome = compile_config_file_in(
            &config,
            oxys_crate_dir().to_str().unwrap(),
            out.path(),
            build.path(),
            tree.path(),
        )
        .expect("known packages should compile");
        assert!(outcome.notices.is_empty());
    }
}
