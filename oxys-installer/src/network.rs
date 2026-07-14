use std::net::{TcpStream, ToSocketAddrs};
use std::path::Path;
use std::process::Command;
use std::time::Duration;

use tokio::sync::mpsc::UnboundedSender;

/// Host pinged to decide whether we have a working internet connection. Port
/// 443 so the probe is a plain TCP connect, not a real HTTPS request.
const PING_HOST: &str = "oxysos.org:443";

/// Upper bound on the whole probe, DNS lookup included. `to_socket_addrs`
/// (getaddrinfo under the hood) has no timeout of its own and can block far
/// longer than any per-connection timeout if the resolver is unreachable, so
/// this wraps the entire blocking probe rather than just the TCP connect.
const PROBE_TIMEOUT: Duration = Duration::from_secs(2);

/// Runs the connectivity probe on a blocking thread and reports the result.
/// The channel is expected to receive exactly one value before closing. If
/// the probe hasn't finished within [`PROBE_TIMEOUT`] this reports offline
/// and moves on -- the blocking thread is left to finish (or hang) on its
/// own; its result is simply discarded.
pub(crate) async fn check_connectivity(tx: UnboundedSender<bool>) {
    let online = tokio::time::timeout(PROBE_TIMEOUT, tokio::task::spawn_blocking(probe))
        .await
        .ok()
        .and_then(Result::ok)
        .unwrap_or(false);
    let _ = tx.send(online);
}

/// DNS resolve + TCP connect probe. A successful resolve alone doesn't prove
/// the host is reachable (captive portals resolve everything), so this only
/// reports online once a socket actually opens.
fn probe() -> bool {
    let Ok(addrs) = PING_HOST.to_socket_addrs() else {
        return false;
    };

    addrs
        .into_iter()
        .any(|addr| TcpStream::connect_timeout(&addr, Duration::from_millis(800)).is_ok())
}

/// Bound on the whole `curl` download below, DNS through transfer. Generous
/// -- a config source is a small `.fe2o3` file -- but still finite so a stalled
/// or malicious server can't hang the picker screen forever.
const FETCH_TIMEOUT_SECS: u64 = 15;

/// Downloads a custom config source into `dest` via `curl`. `curl` is a
/// deliberate member of the live ISO's package set (see
/// `installcd-stage1.spec`: "git + curl fetch configs/assets"), so shelling
/// out to it avoids pulling a TLS-capable HTTP client crate into the
/// installer just for this one-shot download.
///
/// Only called with `http://`/`https://` URLs -- the caller gates on scheme
/// before reaching here, so `curl` never sees a `file://` URL that could read
/// arbitrary local paths.
pub(crate) fn fetch_config(url: &str, dest: &Path) -> Result<(), String> {
    let output = Command::new("curl")
        .args(["--fail", "--silent", "--show-error", "--location"])
        .arg("--max-time")
        .arg(FETCH_TIMEOUT_SECS.to_string())
        .arg("--output")
        .arg(dest)
        .arg(url)
        .output()
        .map_err(|err| format!("failed to run curl: {err}"))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let reason = stderr.trim();
        return Err(if reason.is_empty() {
            "curl exited with an error".to_string()
        } else {
            reason.to_string()
        });
    }

    Ok(())
}
