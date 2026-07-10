use std::path::{Path, PathBuf};

pub(crate) fn version_split_index(value: &str) -> Option<usize> {
    value
        .char_indices()
        .rev()
        .find(|(idx, ch)| {
            if *ch != '-' {
                return false;
            }

            value
                .get(idx + 1..)
                .and_then(|suffix| suffix.chars().next())
                .is_some_and(|next| next.is_ascii_digit())
        })
        .map(|(idx, _)| idx)
}

pub(crate) fn strip_version_suffix(value: &str) -> &str {
    version_split_index(value)
        .map(|idx| &value[..idx])
        .unwrap_or(value)
}

pub(crate) fn sibling_temp_path(path: &Path, fallback_name: &str) -> PathBuf {
    let file_name = path
        .file_name()
        .and_then(|value| value.to_str())
        .map(|value| format!("{value}.tmp"))
        .unwrap_or_else(|| fallback_name.to_owned());

    path.with_file_name(file_name)
}
