use serde::{Deserialize, Serialize};
use thiserror::Error;

use super::InitSystem;

/// Portage atom that ships `/sbin/nft` and the OpenRC `nftables` service.
pub const NFTABLES_PACKAGE: &str = "net-firewall/nftables";
/// OpenRC service that loads `/var/lib/nftables/rules-save` at boot.
pub const NFTABLES_SERVICE: &str = "nftables";

/// Declarative host firewall policy. The runtime layer renders this into a
/// native nftables ruleset at `/var/lib/nftables/rules-save`, which Gentoo's
/// OpenRC `nftables` service loads at boot.
///
/// The DSL is deliberately restricted to chain policies and numeric ports —
/// no arbitrary nft snippets — so every manifest renders to a ruleset Oxys
/// can validate and regenerate.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Firewall {
    /// No Oxys-managed firewall. This is the default so manifests serialized
    /// before this field existed keep their behaviour; stock profiles opt in
    /// explicitly.
    #[default]
    Disabled,
    Nftables {
        incoming: FirewallPolicy,
        forwarding: FirewallPolicy,
        outgoing: FirewallPolicy,
        /// Accept ICMPv4 and ICMPv6. IPv6 neighbour discovery and path-MTU
        /// discovery depend on ICMPv6, so disabling this breaks IPv6.
        allow_icmp: bool,
        /// TCP ports accepted from any source (e.g. `vec![22]` for SSH).
        tcp_ports: Vec<u16>,
        /// UDP ports accepted from any source.
        udp_ports: Vec<u16>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FirewallPolicy {
    Accept,
    Drop,
}

impl FirewallPolicy {
    /// The nftables chain-policy keyword this policy renders to.
    pub fn keyword(self) -> &'static str {
        match self {
            Self::Accept => "accept",
            Self::Drop => "drop",
        }
    }
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum FirewallValidationError {
    #[error("firewall {protocol}_ports contains 0, which is not a valid port")]
    ZeroPort { protocol: &'static str },
    #[error("firewall is enabled but {NFTABLES_PACKAGE} is not in packages")]
    MissingPackage,
    #[error("firewall is enabled but \"{NFTABLES_SERVICE}\" is not in services.openrc.default")]
    MissingService,
    #[error("nftables firewall provisioning requires OpenRC (configured init system: {0:?})")]
    UnsupportedInit(InitSystem),
}

impl Firewall {
    pub fn enabled(&self) -> bool {
        !matches!(self, Self::Disabled)
    }

    /// Validate the policy itself (not its package/service context): every
    /// declared port must be a real port. Port numbers above 65535 are
    /// unrepresentable by construction (`u16`).
    pub fn validate(&self) -> Result<(), FirewallValidationError> {
        let Self::Nftables {
            tcp_ports,
            udp_ports,
            ..
        } = self
        else {
            return Ok(());
        };
        if tcp_ports.contains(&0) {
            return Err(FirewallValidationError::ZeroPort { protocol: "tcp" });
        }
        if udp_ports.contains(&0) {
            return Err(FirewallValidationError::ZeroPort { protocol: "udp" });
        }
        Ok(())
    }
}

impl super::SystemManifest {
    /// Validate that an enabled firewall is actually installable and bootable:
    /// the policy is well-formed, the init system is OpenRC (the only service
    /// model wired up), the nftables package is declared, and the OpenRC
    /// service is in the default runlevel so the rules load at boot.
    pub fn validate_firewall(&self) -> Result<(), FirewallValidationError> {
        if !self.firewall.enabled() {
            return Ok(());
        }
        self.firewall.validate()?;
        if self.init_system != InitSystem::Openrc {
            return Err(FirewallValidationError::UnsupportedInit(self.init_system));
        }
        if !self
            .packages
            .iter()
            .any(|package| package.package == NFTABLES_PACKAGE)
        {
            return Err(FirewallValidationError::MissingPackage);
        }
        if !self
            .services
            .openrc
            .default
            .iter()
            .any(|service| service == NFTABLES_SERVICE)
        {
            return Err(FirewallValidationError::MissingService);
        }
        Ok(())
    }
}
