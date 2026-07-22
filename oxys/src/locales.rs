//! Validate locales against the list shipped by glibc.

use std::path::Path;

/// The locale catalogue installed by sys-libs/glibc.
pub const SUPPORTED_LOCALES_PATH: &str = "/usr/share/i18n/SUPPORTED";

/// Return the canonical `locale.gen` line for `locale`, when it is listed in
/// glibc's SUPPORTED catalogue. The locale name is deliberately restricted to
/// the characters used by glibc locale identifiers before it is compared.
pub fn supported_locale_line(supported: &Path, locale: &str) -> Option<String> {
    let locale = locale.trim();
    if locale.is_empty()
        || locale.len() > 64
        || !locale
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.' | b'@'))
    {
        return None;
    }

    let catalogue = std::fs::read_to_string(supported).ok()?;
    catalogue.lines().find_map(|line| {
        let line = line.trim();
        if line.starts_with('#') || line.split_whitespace().next() != Some(locale) {
            None
        } else {
            Some(line.to_owned())
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_exact_supported_locale_and_rejects_unsafe_values() {
        let tmp = tempfile::tempdir().unwrap();
        let supported = tmp.path().join("SUPPORTED");
        std::fs::write(
            &supported,
            "# generated catalogue\nen_GB.UTF-8 UTF-8\nen_US.UTF-8 UTF-8\n",
        )
        .unwrap();

        assert_eq!(
            supported_locale_line(&supported, "en_US.UTF-8"),
            Some("en_US.UTF-8 UTF-8".to_owned())
        );
        assert_eq!(supported_locale_line(&supported, "en_US"), None);
        assert_eq!(supported_locale_line(&supported, "../../etc/passwd"), None);
    }
}
