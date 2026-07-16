//! Edit a `.fe2o3` config *source* (Rust) in place.
//!
//! `.fe2o3` is the declarative source of truth; `manifest.toml` is compiled from
//! it. `oxys install <pkg>` must therefore add packages to the source, not to the
//! compiled manifest. The source is hand-authored Rust with comments and builder
//! chains, so we do a targeted textual insertion into the existing
//! `packages: vec![ … ]` block rather than an AST round-trip (which would reformat
//! the file and strip comments).

use thiserror::Error;

#[derive(Debug, Error)]
pub enum ConfigSourceError {
    #[error("invalid package atom `{0}`")]
    InvalidAtom(String),
    #[error(
        "could not find a `packages: vec![ … ]` block in the config source; \
         add one before installing packages"
    )]
    NoPackagesVec,
    #[error("unterminated `packages: vec![` block in the config source")]
    UnterminatedVec,
}

/// Insert `Package::new("<atom>")` into the first `packages: vec![ … ]` block of a
/// `.fe2o3` source.
///
/// Returns `Ok(Some(new_source))` when the atom was inserted, `Ok(None)` when the
/// atom is already present (no-op), or an error for an invalid atom / a source with
/// no packages vec.
pub fn add_package_to_source(source: &str, atom: &str) -> Result<Option<String>, ConfigSourceError> {
    let atom = atom.trim();
    validate_atom(atom)?;

    // Dedupe: `Package::new("<atom>")` matches whether or not the entry carries a
    // trailing `.use_flags(...)`/`.keywords(...)` chain.
    let needle = format!("Package::new(\"{atom}\")");
    if source.contains(&needle) {
        return Ok(None);
    }

    let (open_idx, close_idx) = find_packages_vec(source)?;
    let base_indent = line_indent(source, packages_keyword_idx(source).unwrap_or(open_idx));
    let entry_indent = format!("{base_indent}    ");
    let new_entry = format!("{entry_indent}Package::new(\"{atom}\"),");

    // Is the closing `]` alone on its own line (the common multi-line vec)? If so,
    // splice the new entry as a preceding line so the closing bracket stays put.
    let line_start = source[..close_idx].rfind('\n').map(|nl| nl + 1).unwrap_or(0);
    let closing_lead = &source[line_start..close_idx];
    let mut out = String::with_capacity(source.len() + new_entry.len() + 2);
    if closing_lead.trim().is_empty() {
        out.push_str(&source[..line_start]);
        out.push_str(&new_entry);
        out.push('\n');
        out.push_str(&source[line_start..]);
    } else {
        // Inline vec (e.g. `vec![]` or `vec![ Package::new("x") ]`): expand it.
        out.push_str(&source[..close_idx]);
        out.push('\n');
        out.push_str(&new_entry);
        out.push('\n');
        out.push_str(&base_indent);
        out.push_str(&source[close_idx..]);
    }
    Ok(Some(out))
}

/// Gentoo atoms are `category/name` plus optional version/slot/use decorations. We
/// don't fully validate Portage syntax here -- we only reject values that are empty,
/// a de-selection (`-pkg`), or that would break out of the `"…"` string literal we
/// emit into the source.
fn validate_atom(atom: &str) -> Result<(), ConfigSourceError> {
    let invalid = atom.is_empty()
        || atom.starts_with('-')
        || atom
            .chars()
            .any(|c| c.is_whitespace() || c == '"' || c == '\\' || c.is_control());
    if invalid {
        return Err(ConfigSourceError::InvalidAtom(atom.to_owned()));
    }
    Ok(())
}

/// Byte index of the `packages` keyword of the `packages: vec![` declaration.
fn packages_keyword_idx(source: &str) -> Option<usize> {
    // Match `packages` followed (allowing whitespace) by `:` then `vec!`.
    let mut search_from = 0;
    while let Some(rel) = source[search_from..].find("packages") {
        let idx = search_from + rel;
        let rest = source[idx + "packages".len()..].trim_start();
        if let Some(after_colon) = rest.strip_prefix(':')
            && after_colon.trim_start().starts_with("vec!")
        {
            return Some(idx);
        }
        search_from = idx + "packages".len();
    }
    None
}

/// Locate the `[`/`]` byte indices that delimit the `packages: vec![ … ]` list,
/// counting nested brackets while skipping string literals and comments.
fn find_packages_vec(source: &str) -> Result<(usize, usize), ConfigSourceError> {
    let kw = packages_keyword_idx(source).ok_or(ConfigSourceError::NoPackagesVec)?;
    let open_idx = source[kw..]
        .find('[')
        .map(|rel| kw + rel)
        .ok_or(ConfigSourceError::NoPackagesVec)?;
    let close_idx = matching_bracket(source, open_idx)?;
    Ok((open_idx, close_idx))
}

/// Given the index of an opening `[`, return the index of its matching `]`,
/// ignoring brackets inside string/char literals and `//` / `/* */` comments.
fn matching_bracket(source: &str, open_idx: usize) -> Result<usize, ConfigSourceError> {
    let bytes = source.as_bytes();
    let mut depth = 0i32;
    let mut i = open_idx;
    let mut state = Scan::Code;
    while i < bytes.len() {
        let b = bytes[i];
        match state {
            Scan::Code => match b {
                b'"' => state = Scan::Str,
                b'\'' => state = Scan::Char,
                b'/' if bytes.get(i + 1) == Some(&b'/') => state = Scan::LineComment,
                b'/' if bytes.get(i + 1) == Some(&b'*') => {
                    state = Scan::BlockComment;
                    i += 1;
                }
                b'[' => depth += 1,
                b']' => {
                    depth -= 1;
                    if depth == 0 {
                        return Ok(i);
                    }
                }
                _ => {}
            },
            Scan::Str => match b {
                b'\\' => i += 1, // skip escaped char
                b'"' => state = Scan::Code,
                _ => {}
            },
            Scan::Char => match b {
                b'\\' => i += 1,
                b'\'' => state = Scan::Code,
                _ => {}
            },
            Scan::LineComment => {
                if b == b'\n' {
                    state = Scan::Code;
                }
            }
            Scan::BlockComment => {
                if b == b'*' && bytes.get(i + 1) == Some(&b'/') {
                    state = Scan::Code;
                    i += 1;
                }
            }
        }
        i += 1;
    }
    Err(ConfigSourceError::UnterminatedVec)
}

enum Scan {
    Code,
    Str,
    Char,
    LineComment,
    BlockComment,
}

/// Leading whitespace of the line containing byte index `idx`.
fn line_indent(source: &str, idx: usize) -> String {
    let line_start = source[..idx].rfind('\n').map(|nl| nl + 1).unwrap_or(0);
    source[line_start..]
        .chars()
        .take_while(|c| *c == ' ' || *c == '\t')
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"use oxys::prelude::*;

pub fn config() -> Oxys {
    Oxys {
        packages: vec![
            // core tools kept in @world
            Package::new("net-misc/curl"),
            Package::new("dev-vcs/git").keywords(["**"]),
        ],
        ..Default::default()
    }
}
"#;

    #[test]
    fn inserts_before_closing_bracket_with_matching_indent() {
        let out = add_package_to_source(SAMPLE, "app-editors/neovim")
            .unwrap()
            .expect("should insert");
        assert!(out.contains("            Package::new(\"app-editors/neovim\"),\n        ],"));
        // Existing entries and comments are preserved.
        assert!(out.contains("// core tools kept in @world"));
        assert!(out.contains("Package::new(\"dev-vcs/git\").keywords([\"**\"]),"));
    }

    #[test]
    fn dedupes_existing_atom_even_with_builder_chain() {
        assert!(add_package_to_source(SAMPLE, "net-misc/curl").unwrap().is_none());
        assert!(add_package_to_source(SAMPLE, "dev-vcs/git").unwrap().is_none());
    }

    #[test]
    fn nested_brackets_do_not_confuse_the_matcher() {
        // The `.keywords(["**"])` on the last entry has nested brackets right
        // before the closing `]`; insertion must still land in the outer vec.
        let out = add_package_to_source(SAMPLE, "app-shells/fish").unwrap().unwrap();
        let neovim_at = out.find("app-shells/fish").unwrap();
        let close_at = out.find("],").unwrap();
        assert!(neovim_at < close_at);
    }

    #[test]
    fn multiple_sequential_inserts_accumulate() {
        let step1 = add_package_to_source(SAMPLE, "app-editors/neovim").unwrap().unwrap();
        let step2 = add_package_to_source(&step1, "app-shells/fish").unwrap().unwrap();
        assert!(step2.contains("Package::new(\"app-editors/neovim\"),"));
        assert!(step2.contains("Package::new(\"app-shells/fish\"),"));
    }

    #[test]
    fn expands_inline_empty_vec() {
        let src = "    Oxys { packages: vec![], ..Default::default() }";
        let out = add_package_to_source(src, "net-misc/curl").unwrap().unwrap();
        assert!(out.contains("Package::new(\"net-misc/curl\"),"));
    }

    #[test]
    fn rejects_invalid_atoms() {
        assert!(matches!(
            add_package_to_source(SAMPLE, "-net-misc/curl"),
            Err(ConfigSourceError::InvalidAtom(_))
        ));
        assert!(matches!(
            add_package_to_source(SAMPLE, ""),
            Err(ConfigSourceError::InvalidAtom(_))
        ));
        assert!(matches!(
            add_package_to_source(SAMPLE, "bad\"atom"),
            Err(ConfigSourceError::InvalidAtom(_))
        ));
    }

    #[test]
    fn errors_when_no_packages_vec() {
        let src = "pub fn config() -> Oxys { Oxys { ..Default::default() } }";
        assert!(matches!(
            add_package_to_source(src, "net-misc/curl"),
            Err(ConfigSourceError::NoPackagesVec)
        ));
    }
}
