use chrono::{DateTime, Utc};

use super::{
    BlockerKind, ConditionalDep, PackageMetadata, RequiredUseExpr, SlotOperator, UseFlag,
    UseResolverError, package_from_md5_cache_path, util::strip_version_suffix,
};

/// Parses the subset of md5-cache metadata currently needed by the USE resolver.
pub fn parse_md5_cache_metadata(
    md5_cache_path: &std::path::Path,
    contents: &str,
    cached_at: DateTime<Utc>,
) -> Result<PackageMetadata, UseResolverError> {
    let (package, version) = package_from_md5_cache_path(md5_cache_path)?;
    let fields = parse_fields(contents);

    let iuse = fields
        .get("IUSE")
        .map(|value| parse_iuse(value))
        .transpose()?
        .unwrap_or_default();

    let depend = fields
        .get("DEPEND")
        .map(|value| parse_dependencies("DEPEND", value))
        .transpose()?
        .unwrap_or_default();

    let bdepend = fields
        .get("BDEPEND")
        .map(|value| parse_dependencies("BDEPEND", value))
        .transpose()?
        .unwrap_or_default();

    let rdepend = fields
        .get("RDEPEND")
        .map(|value| parse_dependencies("RDEPEND", value))
        .transpose()?
        .unwrap_or_default();

    let pdepend = fields
        .get("PDEPEND")
        .map(|value| parse_dependencies("PDEPEND", value))
        .transpose()?
        .unwrap_or_default();

    let required_use = fields
        .get("REQUIRED_USE")
        .map(|value| parse_required_use(value))
        .transpose()?
        .unwrap_or_default();

    let keywords = fields
        .get("KEYWORDS")
        .map(|value| parse_keywords(value))
        .unwrap_or_default();

    let licenses = fields
        .get("LICENSE")
        .map(|value| parse_tokens(value))
        .unwrap_or_default();

    let properties = fields
        .get("PROPERTIES")
        .map(|value| parse_tokens(value))
        .unwrap_or_default();

    let restrict = fields
        .get("RESTRICT")
        .map(|value| parse_tokens(value))
        .unwrap_or_default();

    let provides = fields
        .get("PROVIDE")
        .map(|value| parse_tokens(value))
        .unwrap_or_default();

    let (slot, subslot) = fields
        .get("SLOT")
        .map(|value| parse_slot(value))
        .transpose()?
        .unwrap_or((None, None));

    Ok(PackageMetadata {
        package,
        version,
        iuse,
        depend,
        bdepend,
        rdepend,
        pdepend,
        required_use,
        keywords,
        licenses,
        properties,
        restrict,
        provides,
        slot,
        subslot,
        cached_at,
    })
}

fn parse_fields(contents: &str) -> std::collections::HashMap<String, String> {
    contents
        .lines()
        .filter_map(|line| line.split_once('='))
        .map(|(key, value)| (key.trim().to_owned(), value.trim().to_owned()))
        .collect()
}

fn parse_iuse(value: &str) -> Result<Vec<UseFlag>, UseResolverError> {
    value.split_whitespace().map(parse_iuse_token).collect()
}

fn parse_iuse_token(token: &str) -> Result<UseFlag, UseResolverError> {
    let trimmed = token.trim();
    if trimmed.is_empty() {
        return Err(UseResolverError::InvalidField {
            field: "IUSE",
            message: "empty USE flag token".to_owned(),
        });
    }

    let default_enabled = trimmed.starts_with('+');
    let flag_name = trimmed
        .trim_start_matches('+')
        .trim_start_matches('-')
        .trim_start_matches('@')
        .trim_end_matches('?');

    if flag_name.is_empty() {
        return Err(UseResolverError::InvalidField {
            field: "IUSE",
            message: format!("invalid USE flag token: {trimmed}"),
        });
    }

    Ok(UseFlag {
        name: flag_name.to_owned(),
        default_enabled,
    })
}

fn parse_keywords(value: &str) -> Vec<String> {
    value.split_whitespace().map(ToOwned::to_owned).collect()
}

fn parse_tokens(value: &str) -> Vec<String> {
    value
        .split_whitespace()
        .filter(|token| !token.is_empty())
        .map(ToOwned::to_owned)
        .collect()
}

fn parse_slot(value: &str) -> Result<(Option<String>, Option<String>), UseResolverError> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Ok((None, None));
    }

    let mut parts = trimmed.split('/');
    let slot = parts.next().map(str::trim).filter(|slot| !slot.is_empty());
    let subslot = parts
        .next()
        .map(str::trim)
        .filter(|subslot| !subslot.is_empty());

    if parts.next().is_some() {
        return Err(UseResolverError::InvalidField {
            field: "SLOT",
            message: format!("invalid SLOT value `{trimmed}`"),
        });
    }

    Ok((slot.map(ToOwned::to_owned), subslot.map(ToOwned::to_owned)))
}

fn parse_dependencies(
    field: &'static str,
    value: &str,
) -> Result<Vec<ConditionalDep>, UseResolverError> {
    let tokens = tokenize(value);
    let mut cursor = TokenCursor::new(&tokens, field);
    let mut deps = Vec::new();
    let mut conditions = Vec::new();
    parse_dependency_group(&mut cursor, &mut conditions, &mut deps, false)?;

    if !cursor.is_exhausted() {
        return Err(cursor.invalid_field(format!(
            "unexpected token `{}`",
            cursor.peek().unwrap_or_default()
        )));
    }

    Ok(deps)
}

/// Recursively parses dependency groups while preserving nested USE conditions.
fn parse_dependency_group(
    cursor: &mut TokenCursor<'_>,
    conditions: &mut Vec<String>,
    deps: &mut Vec<ConditionalDep>,
    stop_on_rparen: bool,
) -> Result<(), UseResolverError> {
    while let Some(token) = cursor.peek() {
        match token {
            ")" => {
                if stop_on_rparen {
                    cursor.next();
                    return Ok(());
                }
                return Err(cursor.invalid_field("unexpected `)`".to_owned()));
            }
            "(" => {
                cursor.next();
                parse_dependency_group(cursor, conditions, deps, true)?;
            }
            "||" | "&&" | "^^" | "??" => {
                cursor.next();
            }
            _ if token.ends_with('?') => {
                let condition = parse_dependency_condition(token);
                cursor.next();
                cursor.expect("(")?;
                conditions.push(condition);
                parse_dependency_group(cursor, conditions, deps, true)?;
                conditions.pop();
            }
            _ => {
                let token = cursor.next().unwrap_or_default();
                if let Some((package, blocker, slot, subslot, slot_operator)) =
                    extract_package_atom(token)
                {
                    deps.push(ConditionalDep {
                        condition: flatten_conditions(conditions),
                        package,
                        blocker,
                        slot,
                        subslot,
                        slot_operator,
                    });
                }
            }
        }
    }

    if stop_on_rparen {
        return Err(cursor.invalid_field("unterminated `(`".to_owned()));
    }

    Ok(())
}

/// Normalizes a dependency conditional token like `foo?` into `foo`.
fn parse_dependency_condition(token: &str) -> String {
    token.trim_end_matches('?').trim().to_owned()
}

/// Flattens nested dependency conditions into a conjunction string.
fn flatten_conditions(conditions: &[String]) -> Option<String> {
    if conditions.is_empty() {
        None
    } else {
        Some(conditions.join(" && "))
    }
}

/// Extracts a normalized dependency atom and optional blocker marker from a token.
fn extract_package_atom(
    token: &str,
) -> Option<(
    String,
    Option<BlockerKind>,
    Option<String>,
    Option<String>,
    Option<SlotOperator>,
)> {
    let trimmed = token
        .trim()
        .trim_matches('(')
        .trim_matches(')')
        .trim_matches('[')
        .trim_matches(']')
        .trim();

    if trimmed.is_empty() || trimmed == "||" || trimmed == "&&" || !trimmed.contains('/') {
        return None;
    }

    let (blocker, atom) = if let Some(atom) = trimmed.strip_prefix("!!") {
        (Some(BlockerKind::Hard), atom)
    } else if let Some(atom) = trimmed.strip_prefix('!') {
        (Some(BlockerKind::Soft), atom)
    } else {
        (None, trimmed)
    };

    let no_use_deps = atom.split('[').next()?;
    let (no_slot, slot, subslot, slot_operator) = parse_dependency_slot(no_use_deps);

    let no_operator = no_slot.trim_start_matches(|ch: char| matches!(ch, '<' | '>' | '=' | '~'));

    let (category, remainder) = no_operator.split_once('/')?;
    if category.is_empty() || remainder.is_empty() {
        return None;
    }

    let package = strip_version_suffix(remainder);
    if package.is_empty() {
        return None;
    }

    Some((
        format!("{category}/{package}"),
        blocker,
        slot,
        subslot,
        slot_operator,
    ))
}

fn parse_dependency_slot(
    atom: &str,
) -> (&str, Option<String>, Option<String>, Option<SlotOperator>) {
    let Some((package, slot_part)) = atom.split_once(':') else {
        return (atom, None, None, None);
    };

    let trimmed = slot_part.trim();
    if trimmed.is_empty() {
        return (package, None, None, None);
    }

    if trimmed == "*" {
        return (package, None, None, Some(SlotOperator::Any));
    }

    let (body, slot_operator) = if let Some(body) = trimmed.strip_suffix('=') {
        (body, Some(SlotOperator::Equal))
    } else {
        (trimmed, None)
    };

    let mut parts = body.split('/');
    let slot = parts
        .next()
        .map(|value| value.trim())
        .filter(|value| !value.is_empty());
    let subslot = parts
        .next()
        .map(|value| value.trim())
        .filter(|value| !value.is_empty());

    (
        package,
        slot.map(ToOwned::to_owned),
        subslot.map(ToOwned::to_owned),
        slot_operator,
    )
}

/// Parses `REQUIRED_USE` expressions from md5-cache metadata.
mod required_use;

pub use required_use::parse_required_use;
use required_use::{TokenCursor, tokenize};

#[cfg(test)]
mod tests;
