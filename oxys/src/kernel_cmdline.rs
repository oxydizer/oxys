use std::{collections::HashMap, fmt};

use thiserror::Error;

use crate::{
    graphics::{GraphicsResolveError, ResolvedGraphics},
    manifest::SystemManifest,
    session::DecisionSource,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedKernelArg {
    pub value: String,
    pub source: DecisionSource,
    pub source_field: String,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KernelDecision {
    pub value: String,
    pub source: DecisionSource,
    pub source_field: String,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ResolvedKernelCmdline {
    pub args: Vec<ResolvedKernelArg>,
    pub decisions: Vec<KernelDecision>,
}

impl ResolvedKernelCmdline {
    pub fn values(&self) -> impl Iterator<Item = &str> {
        self.args.iter().map(|arg| arg.value.as_str())
    }

    pub fn render(&self) -> String {
        if self.args.is_empty() {
            return "kernel command line: no policy arguments".to_owned();
        }

        let mut lines = self
            .args
            .iter()
            .map(|arg| {
                format!(
                    "kernel argument: {}\n      source: {} ({})\n      reason: {}",
                    arg.value, arg.source, arg.source_field, arg.reason
                )
            })
            .collect::<Vec<_>>();
        lines.extend(
            self.decisions
                .iter()
                .filter(|decision| decision.reason.starts_with("deduplicated"))
                .map(|decision| {
                    format!(
                        "kernel merge: {}\n      source: {} ({})\n      reason: {}",
                        decision.value,
                        decision.source,
                        decision.source_field,
                        decision.reason
                    )
                }),
        );
        lines.join("\n    ")
    }
}

impl fmt::Display for ResolvedKernelCmdline {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.render())
    }
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum KernelCmdlineResolveError {
    #[error(transparent)]
    Graphics(#[from] GraphicsResolveError),
    #[error("invalid kernel command line: argument from {source_field} is empty")]
    EmptyArgument { source_field: String },
    #[error(
        "conflicting kernel arguments for `{key}`: `{existing}` from {existing_source} conflicts with `{incoming}` from {incoming_source}"
    )]
    Conflict {
        key: String,
        existing: String,
        existing_source: String,
        incoming: String,
        incoming_source: String,
    },
}

impl SystemManifest {
    pub fn resolved_kernel_cmdline(
        &self,
    ) -> Result<ResolvedKernelCmdline, KernelCmdlineResolveError> {
        resolve_kernel_cmdline(self)
    }
}

pub fn resolve_kernel_cmdline(
    manifest: &SystemManifest,
) -> Result<ResolvedKernelCmdline, KernelCmdlineResolveError> {
    let graphics = manifest.resolved_graphics()?;
    resolve_kernel_cmdline_with_graphics(manifest, &graphics)
}

pub fn resolve_kernel_cmdline_with_graphics(
    manifest: &SystemManifest,
    graphics: &ResolvedGraphics,
) -> Result<ResolvedKernelCmdline, KernelCmdlineResolveError> {
    let mut resolved = ResolvedKernelCmdline::default();
    let mut keys = HashMap::<String, usize>::new();

    for value in &manifest.kernel.cmdline {
        merge_arg(
            &mut resolved,
            &mut keys,
            value,
            DecisionSource::Explicit,
            "kernel.cmdline",
            "declared explicitly in kernel.cmdline",
        )?;
    }

    for argument in &graphics.requirements.kernel_args {
        merge_arg(
            &mut resolved,
            &mut keys,
            &argument.value,
            DecisionSource::Dependency,
            &argument.source_field,
            &argument.reason,
        )?;
    }

    Ok(resolved)
}

fn merge_arg(
    resolved: &mut ResolvedKernelCmdline,
    keys: &mut HashMap<String, usize>,
    value: &str,
    source: DecisionSource,
    source_field: &str,
    reason: &str,
) -> Result<(), KernelCmdlineResolveError> {
    let value = value.trim();
    if value.is_empty() {
        return Err(KernelCmdlineResolveError::EmptyArgument {
            source_field: source_field.to_owned(),
        });
    }

    let key = argument_key(value);
    if let Some(&index) = keys.get(key) {
        let existing = &resolved.args[index];
        if existing.value != value {
            return Err(KernelCmdlineResolveError::Conflict {
                key: key.to_owned(),
                existing: existing.value.clone(),
                existing_source: existing.source_field.clone(),
                incoming: value.to_owned(),
                incoming_source: source_field.to_owned(),
            });
        }

        resolved.decisions.push(KernelDecision {
            value: value.to_owned(),
            source,
            source_field: source_field.to_owned(),
            reason: format!(
                "deduplicated identical argument; first supplied by {}",
                existing.source_field
            ),
        });
        return Ok(());
    }

    keys.insert(key.to_owned(), resolved.args.len());
    resolved.args.push(ResolvedKernelArg {
        value: value.to_owned(),
        source,
        source_field: source_field.to_owned(),
        reason: reason.to_owned(),
    });
    resolved.decisions.push(KernelDecision {
        value: value.to_owned(),
        source,
        source_field: source_field.to_owned(),
        reason: reason.to_owned(),
    });
    Ok(())
}

fn argument_key(value: &str) -> &str {
    value.split_once('=').map_or(value, |(key, _)| key)
}

#[cfg(test)]
mod tests {
    use crate::manifest::{Graphics, Hardware, Kernel, Nvidia};

    use super::*;

    #[test]
    fn explicit_arguments_are_merged_and_identical_values_are_deduplicated() {
        let manifest = SystemManifest {
            kernel: Kernel {
                cmdline: vec!["quiet".into(), "loglevel=3".into(), "quiet".into()],
            },
            ..SystemManifest::default()
        };

        let resolved = resolve_kernel_cmdline(&manifest).unwrap();
        assert_eq!(
            resolved.values().collect::<Vec<_>>(),
            ["quiet", "loglevel=3"]
        );
        assert!(resolved.decisions.iter().any(|decision| {
            decision.value == "quiet" && decision.reason.contains("deduplicated")
        }));
        assert!(resolved.render().contains("kernel merge: quiet"));
    }

    #[test]
    fn contradictory_values_for_the_same_key_are_rejected() {
        let manifest = SystemManifest {
            kernel: Kernel {
                cmdline: vec!["console=tty0".into(), "console=ttyS0".into()],
            },
            ..SystemManifest::default()
        };

        let error = resolve_kernel_cmdline(&manifest).unwrap_err();
        assert!(matches!(
            error,
            KernelCmdlineResolveError::Conflict { ref key, .. } if key == "console"
        ));
    }

    #[test]
    fn nvidia_modesetting_is_derived_and_conflicts_with_explicit_disable() {
        let graphics = Graphics {
            nvidia: Some(Nvidia::default()),
            ..Graphics::default()
        };
        let manifest = SystemManifest {
            hardware: Hardware {
                graphics,
                ..Hardware::default()
            },
            kernel: Kernel {
                cmdline: vec!["nvidia_drm.modeset=0".into()],
            },
            ..SystemManifest::default()
        };

        let error = resolve_kernel_cmdline(&manifest).unwrap_err();
        assert!(matches!(
            error,
            KernelCmdlineResolveError::Conflict {
                ref key,
                ref incoming_source,
                ..
            } if key == "nvidia_drm.modeset"
                && incoming_source == "hardware.graphics.nvidia.modeset"
        ));
    }

    #[test]
    fn nvidia_modesetting_decision_records_provenance() {
        let manifest = SystemManifest {
            hardware: Hardware {
                graphics: Graphics {
                    nvidia: Some(Nvidia::default()),
                    ..Graphics::default()
                },
                ..Hardware::default()
            },
            ..SystemManifest::default()
        };

        let resolved = resolve_kernel_cmdline(&manifest).unwrap();
        let arg = &resolved.args[0];
        assert_eq!(arg.value, "nvidia_drm.modeset=1");
        assert_eq!(arg.source, DecisionSource::Dependency);
        assert_eq!(arg.source_field, "hardware.graphics.nvidia.modeset");
    }
}
