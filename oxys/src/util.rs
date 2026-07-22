use sha2::{Digest, Sha256};
use std::{
    io::{BufReader, Read},
    path::PathBuf,
    sync::mpsc::Sender,
    thread::JoinHandle,
};

/// Default on-disk cache for Portage metadata used by the USE resolver.
///
/// Resolution order:
/// 1. `OXYS_CACHE_DIR` when set and non-empty (tests and operators)
/// 2. `/var/cache/oxys/use-resolver` when running as root (matches install layout)
/// 3. `$XDG_CACHE_HOME/oxys/use-resolver`, else `~/.cache/oxys/use-resolver`,
///    else a tempdir-based fallback
pub fn default_use_resolver_cache_dir() -> PathBuf {
    if let Some(path) = std::env::var_os("OXYS_CACHE_DIR").filter(|path| !path.is_empty()) {
        return PathBuf::from(path);
    }

    if is_effectively_root() {
        return PathBuf::from("/var/cache/oxys/use-resolver");
    }

    let base = std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".cache")))
        .unwrap_or_else(std::env::temp_dir);
    base.join("oxys").join("use-resolver")
}

fn is_effectively_root() -> bool {
    #[cfg(unix)]
    {
        if let Ok(status) = std::fs::read_to_string("/proc/self/status") {
            for line in status.lines() {
                if let Some(rest) = line.strip_prefix("Uid:")
                    && let Some(euid) = rest.split_whitespace().nth(1)
                {
                    return euid == "0";
                }
            }
        }
        false
    }
    #[cfg(not(unix))]
    {
        false
    }
}

pub fn shell_quote(value: &str) -> String {
    if value
        .chars()
        .all(|character| character.is_ascii_alphanumeric() || "-_./:=+".contains(character))
    {
        return value.to_owned();
    }

    format!("'{}'", value.replace('\'', "'\\''"))
}

pub fn portage_quote(value: &str) -> String {
    let mut quoted = String::from("\"");
    for ch in value.chars() {
        match ch {
            '\\' | '"' | '$' | '`' => {
                quoted.push('\\');
                quoted.push(ch);
            }
            _ => quoted.push(ch),
        }
    }
    quoted.push('"');
    quoted
}

pub fn partition_path(device: &str, number: usize) -> String {
    if device
        .chars()
        .last()
        .is_some_and(|character| character.is_ascii_digit())
    {
        format!("{device}p{number}")
    } else {
        format!("{device}{number}")
    }
}

pub fn zfs_dataset_name(name: &str) -> Result<String, String> {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return Err("ZFS dataset name is empty".to_owned());
    }

    let normalized = match trimmed {
        "@" => "ROOT".to_owned(),
        value => value.trim_start_matches('@').to_owned(),
    };

    if normalized.is_empty()
        || normalized.contains('@')
        || normalized.starts_with('/')
        || normalized.ends_with('/')
        || normalized.split('/').any(str::is_empty)
    {
        return Err(format!(
            "invalid ZFS dataset name from subvolume {trimmed:?}"
        ));
    }

    Ok(normalized)
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

pub fn spawn_reader<R, T, F>(reader: R, sender: Sender<T>, map_fn: F) -> JoinHandle<()>
where
    R: Read + Send + 'static,
    T: Send + 'static,
    F: Fn(Result<String, std::io::Error>) -> Option<T> + Send + 'static,
{
    std::thread::spawn(move || {
        // Split on *either* `\n` or `\r`, not just `\n`. Tools that report
        // progress in place -- rsync's `--info=progress2`, for one -- redraw a
        // single status line by writing a carriage return instead of a newline.
        // A plain `.lines()` reader (which only breaks on `\n`) would buffer the
        // whole transfer into one line delivered at the very end, so the log
        // sits silent during the long copy. Breaking on `\r` too lets each
        // in-place refresh stream out as its own line.
        let mut buffered = BufReader::new(reader);
        let mut chunk: Vec<u8> = Vec::new();
        let mut byte = [0u8; 1];
        loop {
            match buffered.read(&mut byte) {
                Ok(0) => {
                    if !chunk.is_empty() {
                        let line = String::from_utf8_lossy(&chunk).into_owned();
                        if let Some(msg) = map_fn(Ok(line)) {
                            let _ = sender.send(msg);
                        }
                    }
                    return;
                }
                Ok(_) => {
                    if byte[0] == b'\n' || byte[0] == b'\r' {
                        // Skip empty segments (e.g. the `\n` half of a `\r\n`).
                        if chunk.is_empty() {
                            continue;
                        }
                        let line = String::from_utf8_lossy(&chunk).into_owned();
                        chunk.clear();
                        if let Some(msg) = map_fn(Ok(line))
                            && sender.send(msg).is_err()
                        {
                            return;
                        }
                    } else {
                        chunk.push(byte[0]);
                    }
                }
                Err(err) => {
                    if let Some(msg) = map_fn(Err(err)) {
                        let _ = sender.send(msg);
                    }
                    return;
                }
            }
        }
    })
}
