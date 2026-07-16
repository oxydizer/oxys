use super::*;

#[test]
fn users_add_a_setup_step_that_never_renders_secrets() {
    let temp = TempTree::new("users");
    let source = temp.path().join("source");
    let target = temp.path().join("target");
    fs::create_dir_all(source.join("boot")).unwrap();
    fs::create_dir_all(&target).unwrap();

    let manifest = SystemManifest {
        disk: Disk {
            device: "/dev/vda".to_owned(),
            layout: DiskLayout::Ext4,
            ..Disk::default()
        },
        users: vec![
            User::new("testuser")
                .wheel()
                .password(Password::Plain("super-secret".to_owned())),
            User::new("bot").password(Password::Prompt),
        ],
        ..SystemManifest::default()
    };

    let plan = plan_system_install(&manifest, &source, &target, None).unwrap();
    let setup = plan
        .steps
        .iter()
        .find(|step| matches!(step, SystemInstallStep::SetupUsers { .. }))
        .expect("plan should contain a SetupUsers step");

    // The plan carries the secret in memory but must never expose it when
    // rendered for the confirm screen or install log.
    let rendered = setup.render();
    assert!(rendered.contains("testuser"));
    assert!(rendered.contains("bot"));
    assert!(!rendered.contains("super-secret"));
    assert!(!plan.render().contains("super-secret"));
}

#[test]
fn unresolved_prompt_username_is_rejected_before_planning() {
    let temp = TempTree::new("unresolved-username");
    let source = temp.path().join("source");
    let target = temp.path().join("target");
    fs::create_dir_all(source.join("boot")).unwrap();
    fs::create_dir_all(&target).unwrap();

    let manifest = SystemManifest {
        disk: Disk {
            device: "/dev/vda".to_owned(),
            layout: DiskLayout::Ext4,
            ..Disk::default()
        },
        users: vec![User::prompt().password(Password::Plain("super-secret".to_owned()))],
        ..SystemManifest::default()
    };

    let error = plan_system_install(&manifest, &source, &target, None).unwrap_err();
    assert!(matches!(error, SystemInstallError::InvalidPlan(_)));
}

#[test]
fn users_are_omitted_when_none_configured() {
    let temp = TempTree::new("no-users");
    let source = temp.path().join("source");
    let target = temp.path().join("target");
    fs::create_dir_all(source.join("boot")).unwrap();
    fs::create_dir_all(&target).unwrap();

    let manifest = SystemManifest {
        disk: Disk {
            device: "/dev/vda".to_owned(),
            layout: DiskLayout::Ext4,
            ..Disk::default()
        },
        ..SystemManifest::default()
    };

    let plan = plan_system_install(&manifest, &source, &target, None).unwrap();
    assert!(
        !plan
            .steps
            .iter()
            .any(|step| matches!(step, SystemInstallStep::SetupUsers { .. }))
    );
}

#[test]
fn verify_target_layout_step_runs_immediately_after_copy() {
    let temp = TempTree::new("verify-step-order");
    let source = temp.path().join("source");
    let target = temp.path().join("target");
    fs::create_dir_all(source.join("boot")).unwrap();
    fs::create_dir_all(&target).unwrap();

    let plan = plan_system_install(&SystemManifest::default(), &source, &target, None).unwrap();
    let verify_at = plan
        .steps
        .iter()
        .position(|step| matches!(step, SystemInstallStep::VerifyTargetLayout { .. }))
        .expect("verify step present");
    // Sits right after the two rsync copies, before anything chroots in.
    assert!(verify_at >= 2);
    assert!(matches!(
        plan.steps[verify_at - 1],
        SystemInstallStep::Command { .. }
    ));
    let bind_at = plan
        .steps
        .iter()
        .position(|step| matches!(step, SystemInstallStep::BindMountPseudo { .. }))
        .expect("bind step present");
    assert!(verify_at < bind_at);
}

#[test]
fn verify_target_layout_flags_missing_dir_and_bad_owner() {
    let temp = TempTree::new("verify-layout");
    let target = temp.path().join("target");
    // A complete-looking tree EXCEPT var/db/pkg, to simulate a truncated copy.
    for dir in ["etc", "usr", "var/tmp", "bin", "sbin", "lib", "root"] {
        fs::create_dir_all(target.join(dir)).unwrap();
    }

    let (sender, _receiver) = mpsc::channel();
    let err = super::super::super::filesystem::verify_target_layout(&target, &sender)
        .expect_err("missing var/db/pkg must fail");
    let message = err.to_string();
    assert!(message.contains("var/db/pkg"), "got: {message}");
    // Dirs created by the (non-root) test user are not root-owned, so the
    // ownership pass also fires -- proving it detects a mis-owned /var.
    assert!(message.contains("expected root"), "got: {message}");
}
