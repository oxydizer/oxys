use std::{
    io::Write,
    path::Path,
    process::{Command, Stdio},
    sync::mpsc::Sender,
};

use crate::{
    exec::ExecError,
    manifest::{Password, User},
};

use super::{SystemInstallError, SystemInstallEvent, run_chroot, write_wheel_sudoers};

pub(super) fn setup_users(
    users: &[User],
    target_mount: &Path,
    sender: &Sender<SystemInstallEvent>,
) -> Result<(), SystemInstallError> {
    let target = target_mount.display().to_string();
    let mut any_wheel = false;

    for user in users {
        let mut args = vec![
            "useradd".to_owned(),
            "-m".to_owned(),
            "-s".to_owned(),
            user.shell.path().to_owned(),
        ];
        if !user.groups.is_empty() {
            args.push("-G".to_owned());
            args.push(user.groups.join(","));
        }
        args.push(user.name.as_str().to_owned());
        run_chroot(&target, &args, sender)?;
        let _ = sender.send(SystemInstallEvent::StepOutput {
            line: format!("created user {}", user.name.as_str()),
        });

        apply_password(&target, user, sender)?;

        any_wheel |= user.is_wheel();
    }

    if any_wheel {
        write_wheel_sudoers(target_mount)?;
        let _ = sender.send(SystemInstallEvent::StepOutput {
            line: "granted wheel group sudo via /etc/sudoers.d/wheel".to_owned(),
        });
    }

    // Lock the root account on the installed target. The live ISO sets a
    // throwaway password on root for tty recovery, but the installed system
    // should be hardened: root is inaccessible directly, and the first
    // wheel user reaches it via sudo. `passwd -l` writes '!' into /etc/shadow.
    run_chroot(
        &target,
        &["passwd".to_owned(), "-l".to_owned(), "root".to_owned()],
        sender,
    )?;
    let _ = sender.send(SystemInstallEvent::StepOutput {
        line: "locked root account (! in /etc/shadow); use sudo from a wheel user".to_owned(),
    });

    Ok(())
}

fn apply_password(
    target: &str,
    user: &User,
    sender: &Sender<SystemInstallEvent>,
) -> Result<(), SystemInstallError> {
    let note = match &user.password {
        Password::None => {
            run_chroot(
                target,
                &[
                    "passwd".to_owned(),
                    "-l".to_owned(),
                    user.name.as_str().to_owned(),
                ],
                sender,
            )?;
            format!("locked password for {}", user.name.as_str())
        }
        Password::Hashed(hash) => {
            chpasswd(target, user.name.as_str(), hash, true)?;
            format!("set hashed password for {}", user.name.as_str())
        }
        Password::Plain(secret) => {
            chpasswd(target, user.name.as_str(), secret, false)?;
            format!("set password for {}", user.name.as_str())
        }
        // Prompt passwords must be resolved to a concrete value before planning;
        // reaching install with one still pending is a bug in the caller.
        Password::Prompt => {
            return Err(SystemInstallError::InvalidPlan(format!(
                "password for user '{}' was not collected before install",
                user.name.as_str()
            )));
        }
    };
    let _ = sender.send(SystemInstallEvent::StepOutput { line: note });
    Ok(())
}

/// Feed `name:secret` into `chpasswd` inside the target chroot. With
/// `encrypted`, the secret is treated as a pre-hashed value (`chpasswd -e`).
fn chpasswd(
    target: &str,
    name: &str,
    secret: &str,
    encrypted: bool,
) -> Result<(), SystemInstallError> {
    let mut command = Command::new("chroot");
    command.arg(target).arg("chpasswd");
    if encrypted {
        command.arg("-e");
    }
    command
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped());

    let mut child = command.spawn()?;
    let mut stdin = child.stdin.take().ok_or_else(|| {
        SystemInstallError::InvalidPlan("failed to open chpasswd stdin".to_owned())
    })?;
    // A trailing newline terminates the single chpasswd record.
    stdin.write_all(format!("{name}:{secret}\n").as_bytes())?;
    drop(stdin);

    let output = child.wait_with_output()?;
    if !output.status.success() {
        return Err(ExecError::StepFailed {
            step: format!("set password for {name}"),
            status: output.status,
        }
        .into());
    }
    Ok(())
}
