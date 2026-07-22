use std::{
    fs,
    os::unix::fs::{MetadataExt, PermissionsExt, symlink},
    path::Path,
};

use sha2::{Digest, Sha256};
use tempfile::TempDir;

const CATEGORY: &str = "gui-apps";
const PF: &str = "wl-clipboard-2.2.1";

#[test]
fn canonical_filename_includes_architecture_and_microarchitecture() {
    let reference = fixture_reference_root();
    fs::write(
        reference
            .path()
            .join(format!("var/db/pkg/{CATEGORY}/{PF}/CFLAGS")),
        "-O2 -pipe -march=x86-64-v3\n",
    )
    .unwrap();
    fs::write(
        reference
            .path()
            .join(format!("var/db/pkg/{CATEGORY}/{PF}/BUILD_ID")),
        "2\n",
    )
    .unwrap();
    let output_dir = TempDir::new().unwrap();

    let (metadata, artifact) =
        oxys::packages::build_named(reference.path(), "gui-apps/wl-clipboard", output_dir.path())
            .unwrap();

    assert_eq!(
        artifact.file_name().unwrap(),
        "wl-clipboard-2.2.1-r2-x86_64-v3.oxys"
    );
    assert!(artifact.is_file());
    assert_eq!(oxys::packages::verify(&artifact).unwrap(), metadata);
}

#[test]
fn artifact_round_trip_is_verified_idempotent_and_removable() {
    let reference = fixture_reference_root();
    let output_dir = TempDir::new().unwrap();
    let artifact = output_dir.path().join("wl-clipboard.oxys");
    let second = output_dir.path().join("wl-clipboard-second.oxys");

    let metadata =
        oxys::packages::build(reference.path(), "gui-apps/wl-clipboard", &artifact).unwrap();
    assert_eq!(metadata.portage.pf, PF);
    oxys::packages::build(reference.path(), "gui-apps/wl-clipboard", &second).unwrap();
    assert_eq!(
        fs::read(&artifact).unwrap(),
        fs::read(&second).unwrap(),
        "writer must be deterministic"
    );
    assert_eq!(oxys::packages::verify(&artifact).unwrap(), metadata);
    assert_eq!(
        u32::from_le_bytes(fs::read(&artifact).unwrap()[12..16].try_into().unwrap()),
        1,
        "hardlink artifacts must declare the hardlink capability flag"
    );

    let target = TempDir::new().unwrap();
    oxys::packages::install(&artifact, target.path()).unwrap();
    assert_eq!(
        fs::read(target.path().join("usr/bin/wl-copy")).unwrap(),
        b"wl-copy fixture\n"
    );
    let canonical = fs::metadata(target.path().join("usr/bin/wl-copy")).unwrap();
    let alias = fs::metadata(target.path().join("usr/bin/wl-copy-canonical")).unwrap();
    assert_eq!(
        (canonical.dev(), canonical.ino()),
        (alias.dev(), alias.ino())
    );
    assert_eq!(
        fs::metadata(target.path().join("usr/bin/wl-copy"))
            .unwrap()
            .mtime(),
        0,
        "installer must restore the timestamp captured by VDB CONTENTS"
    );
    assert_eq!(
        fs::read_link(target.path().join("usr/bin/wl-copy-link")).unwrap(),
        Path::new("wl-copy")
    );
    assert_eq!(
        fs::read(
            target
                .path()
                .join(format!("var/db/pkg/{CATEGORY}/{PF}/environment.bz2"))
        )
        .unwrap(),
        [0, 1, 2, 3, 255]
    );
    assert!(
        fs::read_to_string(target.path().join("var/lib/portage/world"))
            .unwrap()
            .lines()
            .any(|line| line == "gui-apps/wl-clipboard")
    );
    let hold_path = target.path().join("etc/portage/package.mask/oxys");
    let hold = fs::read_to_string(&hold_path).unwrap();
    assert!(
        hold.starts_with('#'),
        "hold fragment must begin with the managed-by-oxys header"
    );
    assert_eq!(
        hold.lines()
            .filter(|line| !line.trim().is_empty() && !line.trim_start().starts_with('#'))
            .collect::<Vec<_>>(),
        vec![format!(">{CATEGORY}/{PF}")],
        "install must register exactly one version hold"
    );

    let receipt = oxys::packages::receipt_path(target.path(), CATEGORY, PF);
    let original_receipt = fs::read(&receipt).unwrap();
    let original_hold = fs::read(&hold_path).unwrap();
    oxys::packages::install(&artifact, target.path()).unwrap();
    assert_eq!(
        fs::read(&receipt).unwrap(),
        original_receipt,
        "reinstall must be idempotent"
    );
    assert_eq!(
        fs::read(&hold_path).unwrap(),
        original_hold,
        "reinstall must not duplicate the version hold"
    );

    fs::write(target.path().join("usr/bin/wl-copy"), "locally modified\n").unwrap();
    let error = oxys::packages::remove(target.path(), &format!("{CATEGORY}/{PF}")).unwrap_err();
    assert!(error.to_string().contains("SHA-256 mismatch"));
    assert!(receipt.exists(), "failed removal must preserve receipt");
    assert!(
        target
            .path()
            .join(format!("var/db/pkg/{CATEGORY}/{PF}"))
            .exists()
    );

    fs::write(target.path().join("usr/bin/wl-copy"), "wl-copy fixture\n").unwrap();
    oxys::packages::remove(target.path(), &format!("{CATEGORY}/{PF}")).unwrap();
    assert!(!target.path().join("usr/bin/wl-copy").exists());
    assert!(
        !target
            .path()
            .join(format!("var/db/pkg/{CATEGORY}/{PF}"))
            .exists()
    );
    assert!(!receipt.exists());
    assert!(
        !fs::read_to_string(target.path().join("var/lib/portage/world"))
            .unwrap()
            .lines()
            .any(|line| line == "gui-apps/wl-clipboard")
    );
    assert!(
        !hold_path.exists(),
        "removing the last held package must drop the hold fragment"
    );
}

#[test]
fn install_preserves_pre_existing_hold_line() {
    let reference = fixture_reference_root();
    let output_dir = TempDir::new().unwrap();
    let artifact = output_dir.path().join("wl-clipboard.oxys");
    oxys::packages::build(reference.path(), "gui-apps/wl-clipboard", &artifact).unwrap();

    let target = TempDir::new().unwrap();
    let user_holds = format!(">{CATEGORY}/{PF}\n>app-misc/other-1.0\n");
    write_file(
        target.path(),
        "etc/portage/package.mask/oxys",
        user_holds.as_bytes(),
        0o644,
    );

    oxys::packages::install(&artifact, target.path()).unwrap();
    let receipt_path = oxys::packages::receipt_path(target.path(), CATEGORY, PF);
    let receipt: toml::Value = toml::from_str(&fs::read_to_string(&receipt_path).unwrap()).unwrap();
    assert_eq!(
        receipt["hold_added"].as_bool(),
        Some(false),
        "a pre-existing hold line must stay user-owned"
    );

    oxys::packages::remove(target.path(), &format!("{CATEGORY}/{PF}")).unwrap();
    assert_eq!(
        fs::read_to_string(target.path().join("etc/portage/package.mask/oxys")).unwrap(),
        user_holds,
        "removal must not prune hold lines it did not add"
    );
}

#[test]
fn install_fails_early_when_package_mask_is_a_regular_file() {
    let reference = fixture_reference_root();
    let output_dir = TempDir::new().unwrap();
    let artifact = output_dir.path().join("wl-clipboard.oxys");
    oxys::packages::build(reference.path(), "gui-apps/wl-clipboard", &artifact).unwrap();

    let target = TempDir::new().unwrap();
    write_file(
        target.path(),
        "etc/portage/package.mask",
        b">app-misc/user-managed-1.0\n",
        0o644,
    );

    let error = oxys::packages::install(&artifact, target.path()).unwrap_err();
    assert!(
        error
            .to_string()
            .contains("etc/portage/package.mask exists but is not a directory"),
        "unexpected error: {error}"
    );
    assert!(
        !target.path().join("usr/bin/wl-copy").exists(),
        "preflight must fail before any payload is written"
    );
    assert!(!oxys::packages::receipt_path(target.path(), CATEGORY, PF).exists());
    assert!(!target.path().join("var/lib/oxys/transactions").exists());
    assert!(!target.path().join("var/cache/oxys").exists());
}

#[test]
fn remove_fails_early_when_package_mask_became_a_regular_file() {
    let reference = fixture_reference_root();
    let output_dir = TempDir::new().unwrap();
    let artifact = output_dir.path().join("wl-clipboard.oxys");
    oxys::packages::build(reference.path(), "gui-apps/wl-clipboard", &artifact).unwrap();
    let target = TempDir::new().unwrap();
    oxys::packages::install(&artifact, target.path()).unwrap();

    let mask_dir = target.path().join("etc/portage/package.mask");
    fs::remove_dir_all(&mask_dir).unwrap();
    fs::write(&mask_dir, b">app-misc/user-managed-1.0\n").unwrap();

    let error = oxys::packages::remove(target.path(), &format!("{CATEGORY}/{PF}")).unwrap_err();
    assert!(
        error
            .to_string()
            .contains("etc/portage/package.mask exists but is not a directory"),
        "unexpected error: {error}"
    );
    assert!(
        target.path().join("usr/bin/wl-copy").exists(),
        "preflight must fail before any payload deletion"
    );
    assert!(oxys::packages::receipt_path(target.path(), CATEGORY, PF).exists());
}

#[test]
fn remove_handles_receipts_without_hold_field() {
    let reference = fixture_reference_root();
    let output_dir = TempDir::new().unwrap();
    let artifact = output_dir.path().join("wl-clipboard.oxys");
    oxys::packages::build(reference.path(), "gui-apps/wl-clipboard", &artifact).unwrap();
    let target = TempDir::new().unwrap();
    oxys::packages::install(&artifact, target.path()).unwrap();

    // Simulate a receipt written before the hold feature existed.
    let receipt_path = oxys::packages::receipt_path(target.path(), CATEGORY, PF);
    let stripped = fs::read_to_string(&receipt_path)
        .unwrap()
        .lines()
        .filter(|line| !line.starts_with("hold_added"))
        .map(|line| format!("{line}\n"))
        .collect::<String>();
    fs::write(&receipt_path, stripped).unwrap();

    oxys::packages::remove(target.path(), &format!("{CATEGORY}/{PF}")).unwrap();
    assert!(!receipt_path.exists());
    assert!(
        fs::read_to_string(target.path().join("etc/portage/package.mask/oxys"))
            .unwrap()
            .lines()
            .any(|line| line == format!(">{CATEGORY}/{PF}")),
        "a pre-feature receipt must leave the hold fragment untouched"
    );
}

#[test]
fn install_rejects_existing_symlink_parent() {
    let reference = fixture_reference_root();
    let output_dir = TempDir::new().unwrap();
    let artifact = output_dir.path().join("wl-clipboard.oxys");
    oxys::packages::build(reference.path(), "gui-apps/wl-clipboard", &artifact).unwrap();

    let target = TempDir::new().unwrap();
    let outside = TempDir::new().unwrap();
    symlink(outside.path(), target.path().join("usr")).unwrap();
    assert!(oxys::packages::install(&artifact, target.path()).is_err());
    assert!(fs::read_dir(outside.path()).unwrap().next().is_none());
}

#[test]
fn portage_vdb_shared_file_survives_oxys_package_removal() {
    let reference = fixture_reference_root();
    let output_dir = TempDir::new().unwrap();
    let artifact = output_dir.path().join("wl-clipboard.oxys");
    oxys::packages::build(reference.path(), "gui-apps/wl-clipboard", &artifact).unwrap();

    let target = TempDir::new().unwrap();
    write_file(
        target.path(),
        "usr/bin/wl-paste",
        b"wl-paste fixture\n",
        0o755,
    );
    write_file(
        target.path(),
        "var/db/pkg/app-misc/existing-1/CONTENTS",
        b"obj /usr/bin/wl-paste 8ccde65a0aa53b22a9a35f1b37046222 0\n",
        0o644,
    );

    oxys::packages::install(&artifact, target.path()).unwrap();
    oxys::packages::remove(target.path(), &format!("{CATEGORY}/{PF}")).unwrap();

    assert_eq!(
        fs::read(target.path().join("usr/bin/wl-paste")).unwrap(),
        b"wl-paste fixture\n",
        "a file still owned by another Portage VDB package must be retained"
    );
    assert!(
        target
            .path()
            .join("var/db/pkg/app-misc/existing-1/CONTENTS")
            .exists()
    );
}

#[test]
fn interrupted_removal_is_resumable_from_journal() {
    let reference = fixture_reference_root();
    let output_dir = TempDir::new().unwrap();
    let artifact = output_dir.path().join("wl-clipboard.oxys");
    oxys::packages::build(reference.path(), "gui-apps/wl-clipboard", &artifact).unwrap();
    let target = TempDir::new().unwrap();
    oxys::packages::install(&artifact, target.path()).unwrap();

    let receipt_path = oxys::packages::receipt_path(target.path(), CATEGORY, PF);
    let receipt: toml::Value = toml::from_str(&fs::read_to_string(receipt_path).unwrap()).unwrap();
    let journal = format!(
        "operation = \"remove\"\npackage = {:?}\nartifact = {:?}\nphase = \"removing\"\nstarted_at = \"2026-01-01T00:00:00Z\"\nworld_added = true\nhold_added = true\n",
        receipt["package"].as_str().unwrap(),
        receipt["artifact"].as_str().unwrap(),
    );
    write_file(
        target.path(),
        &format!("var/lib/oxys/transactions/{CATEGORY}/{PF}.toml"),
        journal.as_bytes(),
        0o600,
    );
    fs::remove_file(target.path().join("usr/bin/wl-copy-canonical")).unwrap();

    oxys::packages::remove(target.path(), &format!("{CATEGORY}/{PF}")).unwrap();
    assert!(!oxys::packages::receipt_path(target.path(), CATEGORY, PF).exists());
    assert!(
        !target
            .path()
            .join(format!("var/db/pkg/{CATEGORY}/{PF}"))
            .exists()
    );
    assert!(
        !target.path().join("etc/portage/package.mask/oxys").exists(),
        "resumed removal must prune the version hold"
    );

    // Simulate a crash after the receipt commit but before the final journal
    // unlink. Removal recovery must clear it, and it must not block reinstall.
    let journal_path = format!("var/lib/oxys/transactions/{CATEGORY}/{PF}.toml");
    write_file(target.path(), &journal_path, journal.as_bytes(), 0o600);
    oxys::packages::remove(target.path(), &format!("{CATEGORY}/{PF}")).unwrap();
    assert!(!target.path().join(&journal_path).exists());
    oxys::packages::install(&artifact, target.path()).unwrap();
}

#[test]
fn interrupted_install_with_partial_vdb_is_resumable_from_journal() {
    let reference = fixture_reference_root();
    let output_dir = TempDir::new().unwrap();
    let artifact = output_dir.path().join("wl-clipboard.oxys");
    oxys::packages::build(reference.path(), "gui-apps/wl-clipboard", &artifact).unwrap();

    let target = TempDir::new().unwrap();
    fs::create_dir_all(target.path().join(format!("var/db/pkg/{CATEGORY}/{PF}"))).unwrap();
    write_file(
        target.path(),
        "usr/bin/wl-paste",
        b"wl-paste fixture\n",
        0o755,
    );
    let digest = Sha256::digest(fs::read(&artifact).unwrap());
    let digest = digest
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    let journal = format!(
        "operation = \"install\"\npackage = \"gentoo/gui-apps/wl-clipboard@2.2.1#0:0\"\nartifact = \"sha256:{digest}\"\nphase = \"installing\"\nstarted_at = \"2026-01-01T00:00:00Z\"\nworld_added = false\nhold_added = false\n"
    );
    write_file(
        target.path(),
        &format!("var/lib/oxys/transactions/{CATEGORY}/{PF}.toml"),
        journal.as_bytes(),
        0o600,
    );

    oxys::packages::install(&artifact, target.path()).unwrap();
    assert!(
        target
            .path()
            .join(format!("var/db/pkg/{CATEGORY}/{PF}/CONTENTS"))
            .exists()
    );
    let receipt_path = oxys::packages::receipt_path(target.path(), CATEGORY, PF);
    let receipt: toml::Value = toml::from_str(&fs::read_to_string(&receipt_path).unwrap()).unwrap();
    assert_eq!(
        receipt["hold_added"].as_bool(),
        Some(true),
        "a resumed install must still register and record the version hold"
    );
    assert!(
        fs::read_to_string(target.path().join("etc/portage/package.mask/oxys"))
            .unwrap()
            .lines()
            .any(|line| line == format!(">{CATEGORY}/{PF}"))
    );
}

#[test]
fn reinstall_and_remove_reject_a_copy_that_broke_hardlink_identity() {
    let reference = fixture_reference_root();
    let output_dir = TempDir::new().unwrap();
    let artifact = output_dir.path().join("wl-clipboard.oxys");
    oxys::packages::build(reference.path(), "gui-apps/wl-clipboard", &artifact).unwrap();
    let target = TempDir::new().unwrap();
    oxys::packages::install(&artifact, target.path()).unwrap();

    let alias = target.path().join("usr/bin/wl-copy-canonical");
    fs::remove_file(&alias).unwrap();
    fs::write(&alias, b"wl-copy fixture\n").unwrap();
    fs::set_permissions(&alias, fs::Permissions::from_mode(0o755)).unwrap();

    let error = oxys::packages::install(&artifact, target.path()).unwrap_err();
    assert!(error.to_string().contains("hardlink identity mismatch"));
    let error = oxys::packages::remove(target.path(), &format!("{CATEGORY}/{PF}")).unwrap_err();
    assert!(error.to_string().contains("hardlink identity mismatch"));
}

#[test]
fn capture_rejects_an_incomplete_hardlink_group() {
    let reference = fixture_reference_root();
    fs::hard_link(
        reference.path().join("usr/bin/wl-copy"),
        reference.path().join("unowned-copy"),
    )
    .unwrap();
    let output_dir = TempDir::new().unwrap();
    let error = oxys::packages::build(
        reference.path(),
        "gui-apps/wl-clipboard",
        &output_dir.path().join("wl-clipboard.oxys"),
    )
    .unwrap_err();
    assert!(error.to_string().contains("filesystem links but only"));
}

#[test]
fn verify_rejects_tampered_container() {
    let reference = fixture_reference_root();
    let output_dir = TempDir::new().unwrap();
    let artifact = output_dir.path().join("wl-clipboard.oxys");
    oxys::packages::build(reference.path(), "gui-apps/wl-clipboard", &artifact).unwrap();
    let mut bytes = fs::read(&artifact).unwrap();
    let last = bytes.len() - 1;
    bytes[last] ^= 0x40;
    fs::write(&artifact, bytes).unwrap();
    assert!(oxys::packages::verify(&artifact).is_err());
}

fn fixture_reference_root() -> TempDir {
    let root = TempDir::new().unwrap();
    write_file(root.path(), "usr/bin/wl-copy", b"wl-copy fixture\n", 0o755);
    fs::hard_link(
        root.path().join("usr/bin/wl-copy"),
        root.path().join("usr/bin/wl-copy-canonical"),
    )
    .unwrap();
    write_file(
        root.path(),
        "usr/bin/wl-paste",
        b"wl-paste fixture\n",
        0o755,
    );
    symlink("wl-copy", root.path().join("usr/bin/wl-copy-link")).unwrap();
    fs::create_dir_all(root.path().join("usr/share/doc/wl-clipboard-2.2.1")).unwrap();
    fs::set_permissions(
        root.path().join("usr/share/doc/wl-clipboard-2.2.1"),
        fs::Permissions::from_mode(0o755),
    )
    .unwrap();

    let vdb = root.path().join(format!("var/db/pkg/{CATEGORY}/{PF}"));
    fs::create_dir_all(&vdb).unwrap();
    write_file(
        root.path(),
        &format!("var/db/pkg/{CATEGORY}/{PF}/CONTENTS"),
        b"obj /usr/bin/wl-copy 830f867f12bb62f69ef6c406b3cb68d4 0\n\
          obj /usr/bin/wl-copy-canonical 830f867f12bb62f69ef6c406b3cb68d4 0\n\
          obj /usr/bin/wl-paste 8ccde65a0aa53b22a9a35f1b37046222 0\n\
          sym /usr/bin/wl-copy-link -> wl-copy 0\n\
          dir /usr/share/doc/wl-clipboard-2.2.1\n",
        0o644,
    );
    for (name, contents) in [
        ("SLOT", "0\n"),
        ("repository", "gentoo\n"),
        ("CHOST", "x86_64-pc-linux-gnu\n"),
        ("CFLAGS", "-O2 -pipe -march=x86-64\n"),
        ("USE", "man wayland\n"),
        ("RDEPEND", "dev-libs/wayland\n"),
        ("DEPEND", "dev-libs/wayland\n"),
        ("EAPI", "8\n"),
    ] {
        write_file(
            root.path(),
            &format!("var/db/pkg/{CATEGORY}/{PF}/{name}"),
            contents.as_bytes(),
            0o644,
        );
    }
    write_file(
        root.path(),
        &format!("var/db/pkg/{CATEGORY}/{PF}/environment.bz2"),
        &[0, 1, 2, 3, 255],
        0o644,
    );
    root
}

fn write_file(root: &Path, relative: &str, contents: &[u8], mode: u32) {
    let path = root.join(relative);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(&path, contents).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(mode)).unwrap();
}
