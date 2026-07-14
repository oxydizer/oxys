use std::{fs, path::Path, sync::mpsc::Sender};

use crate::exec::{self, StepEvent, StepStream};

use super::{DiskError, DiskPlan, ProvisionEvent, ProvisionStream};

pub fn apply_disk_plan(plan: &DiskPlan) -> ProvisionStream {
    let plan = plan.clone();
    StepStream::spawn(move |sender| run_plan(plan, sender))
}

fn run_plan(plan: DiskPlan, sender: Sender<ProvisionEvent>) -> Result<(), DiskError> {
    for step in &plan.steps {
        if let Err(error) = exec::run_command_step(step, &sender).map_err(DiskError::from) {
            let _ = sender.send(StepEvent::Error {
                step: step.description.clone(),
                message: error.to_string(),
            });
            // A step may have already mounted the target; release it so the next
            // attempt isn't blocked by preflight's mounted-disk guard.
            release_target_mounts(&plan.target_mount);
            return Err(error);
        }
    }

    Ok(())
}

pub(super) fn partition_path(device: &str, number: usize) -> String {
    crate::util::partition_path(device, number)
}

fn partition_prefix(device: &str) -> String {
    if device
        .chars()
        .last()
        .is_some_and(|character| character.is_ascii_digit())
    {
        format!("{device}p")
    } else {
        device.to_owned()
    }
}

pub(super) fn mount_source_matches_device(
    source: &str,
    device: &str,
    canonical_device: &Path,
) -> bool {
    if source == device || is_partition_path(source, device) {
        return true;
    }

    let source_path = Path::new(source);
    let Ok(canonical_source) = fs::canonicalize(source_path) else {
        return false;
    };
    canonical_source == canonical_device
        || is_partition_path(
            &canonical_source.to_string_lossy(),
            &canonical_device.to_string_lossy(),
        )
}

fn is_partition_path(source: &str, device: &str) -> bool {
    let prefix = partition_prefix(device);
    let Some(rest) = source.strip_prefix(&prefix) else {
        return false;
    };
    !rest.is_empty() && rest.chars().all(|character| character.is_ascii_digit())
}

/// Best-effort release of any leftover mounts under the installer's own target
/// mount point.
///
/// An install that aborts before the final unmount (`Finalize`) leaves the new
/// root -- and the ESP mounted beneath it -- mounted at `target_mount`. On the
/// next attempt `preflight` sees those and refuses with "refusing to provision
/// mounted/live disk", forcing a manual `umount`. Clearing them here makes a
/// re-run just work. This only ever touches the installer's own mount point
/// (`/mnt/oxys`), never arbitrary user mounts, so a partition mounted somewhere
/// else still (correctly) trips the preflight guard.
pub fn release_target_mounts(target_mount: &Path) {
    let path = target_mount.display().to_string();
    let cleared = exec::capture_command("umount", ["-R", &path])
        .map(|output| output.status.success())
        .unwrap_or(false);
    if !cleared {
        // A busy mount (e.g. a lingering process) needs a lazy detach.
        let _ = exec::capture_command("umount", ["-Rl", &path]);
    }
}
