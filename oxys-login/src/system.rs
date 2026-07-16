use super::*;

const LOGOS: &[&str] = &[
    r#"   _  __
  / |/ /__  ____ ___
 /    / _ \/ __ `__ \
/_/|_/\___/_/ /_/ /_/
"#,
    r#" _ __  _ _ __ _
| '_ \| | '_ \ |
| | | | | |_)| |
|_| |_|_| .__/|_|
        |_|
"#,
    r#" .----------------.
 |   O X I S      |
 |  login shell   |
 '----------------'
"#,
];

pub(super) fn current_hostname() -> String {
    env::var("HOSTNAME")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .or_else(|| {
            fs::read_to_string("/etc/hostname")
                .ok()
                .map(|value| value.trim().to_string())
                .filter(|value| !value.is_empty())
        })
        .unwrap_or_else(|| "unknown-host".to_string())
}

pub(super) fn formatted_uptime() -> String {
    let raw = fs::read_to_string("/proc/uptime").unwrap_or_default();
    let seconds = raw
        .split_whitespace()
        .next()
        .and_then(|value| value.split('.').next())
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(0);

    let days = seconds / 86_400;
    let hours = (seconds % 86_400) / 3_600;
    let minutes = (seconds % 3_600) / 60;

    if days > 0 {
        format!("{days}d {hours}h {minutes}m")
    } else if hours > 0 {
        format!("{hours}h {minutes}m")
    } else {
        format!("{minutes}m")
    }
}

pub(super) fn formatted_clock() -> String {
    Local::now().format("%Y-%m-%d %H:%M:%S").to_string()
}

pub(super) fn formatted_boot_time() -> String {
    let output = Command::new("systemd-analyze").output();
    let stdout = match output {
        Ok(output) if output.status.success() => {
            String::from_utf8_lossy(&output.stdout).to_string()
        }
        _ => return "unavailable".to_string(),
    };

    stdout
        .lines()
        .find_map(parse_systemd_analyze_total)
        .unwrap_or_else(|| "unavailable".to_string())
}

fn parse_systemd_analyze_total(line: &str) -> Option<String> {
    let total = line.split('=').nth(1)?.trim();
    let value = total.split_whitespace().next()?.trim();
    if value.is_empty() {
        None
    } else if let Some(seconds) = value.strip_suffix('s') {
        let parsed = seconds.parse::<f64>().ok()?;
        Some(format!("{parsed:.2}s"))
    } else {
        Some(value.to_string())
    }
}

pub(super) fn random_logo() -> &'static str {
    let seed = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos() as usize)
        .unwrap_or(0);
    LOGOS[seed % LOGOS.len()]
}
