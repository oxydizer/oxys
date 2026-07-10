use chrono::{DateTime, Utc};

use super::{
    package_from_md5_cache_path, util::strip_version_suffix, BlockerKind, ConditionalDep,
    PackageMetadata, RequiredUseExpr, SlotOperator, UseFlag, UseResolverError,
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
pub fn parse_required_use(value: &str) -> Result<Vec<RequiredUseExpr>, UseResolverError> {
    let tokens = tokenize(value);
    let mut cursor = TokenCursor::new(&tokens, "REQUIRED_USE");
    let exprs = parse_required_use_sequence(&mut cursor, false)?;

    if !cursor.is_exhausted() {
        return Err(cursor.invalid_field(format!(
            "unexpected token `{}`",
            cursor.peek().unwrap_or_default()
        )));
    }

    Ok(exprs)
}

/// Parses a sequence of REQUIRED_USE expressions until end-of-input or `)`.
fn parse_required_use_sequence(
    cursor: &mut TokenCursor<'_>,
    stop_on_rparen: bool,
) -> Result<Vec<RequiredUseExpr>, UseResolverError> {
    let mut exprs = Vec::new();

    while let Some(token) = cursor.peek() {
        if token == ")" {
            if stop_on_rparen {
                cursor.next();
                return Ok(exprs);
            }
            return Err(cursor.invalid_field("unexpected `)`".to_owned()));
        }

        exprs.push(parse_required_use_expr(cursor)?);
    }

    if stop_on_rparen {
        return Err(cursor.invalid_field("unterminated `(`".to_owned()));
    }

    Ok(exprs)
}

/// Parses a single REQUIRED_USE expression from the token cursor.
fn parse_required_use_expr(
    cursor: &mut TokenCursor<'_>,
) -> Result<RequiredUseExpr, UseResolverError> {
    let field = cursor.field;
    let token = cursor
        .next()
        .map(str::to_owned)
        .ok_or_else(|| UseResolverError::InvalidField {
            field,
            message: "unexpected end of input".to_owned(),
        })?;

    match token.as_str() {
        "||" => Ok(RequiredUseExpr::AnyOf(parse_required_use_group(cursor)?)),
        "^^" => Ok(RequiredUseExpr::ExactlyOne(parse_required_use_group(
            cursor,
        )?)),
        "??" => Ok(RequiredUseExpr::AtMostOne(parse_required_use_group(
            cursor,
        )?)),
        "(" => Ok(RequiredUseExpr::AllOf(parse_required_use_sequence(
            cursor, true,
        )?)),
        ")" => Err(cursor.invalid_field("unexpected `)`".to_owned())),
        _ if token.ends_with('?') => {
            let condition = token.trim_end_matches('?').trim();
            if condition.is_empty() {
                return Err(cursor.invalid_field(format!("invalid conditional token `{token}`")));
            }
            Ok(RequiredUseExpr::IfThen(
                condition.to_owned(),
                parse_required_use_group(cursor)?,
            ))
        }
        _ => parse_required_use_flag(&token, field),
    }
}

/// Parses a parenthesized REQUIRED_USE group.
fn parse_required_use_group(
    cursor: &mut TokenCursor<'_>,
) -> Result<Vec<RequiredUseExpr>, UseResolverError> {
    cursor.expect("(")?;
    let exprs = parse_required_use_sequence(cursor, true)?;
    if exprs.is_empty() {
        return Err(cursor.invalid_field("empty group is not allowed".to_owned()));
    }
    Ok(exprs)
}

/// Parses a REQUIRED_USE flag leaf expression.
fn parse_required_use_flag(
    token: &str,
    field: &'static str,
) -> Result<RequiredUseExpr, UseResolverError> {
    if token.is_empty() {
        return Err(UseResolverError::InvalidField {
            field,
            message: "empty flag token".to_owned(),
        });
    }

    if let Some(flag) = token.strip_prefix('!') {
        if flag.is_empty() {
            return Err(UseResolverError::InvalidField {
                field,
                message: format!("invalid flag token `{token}`"),
            });
        }
        return Ok(RequiredUseExpr::Not(flag.to_owned()));
    }

    Ok(RequiredUseExpr::Flag(token.to_owned()))
}

/// Splits md5-cache expression text into whitespace and parenthesis-delimited tokens.
fn tokenize(value: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut current = String::new();

    for ch in value.chars() {
        match ch {
            '(' | ')' => {
                if !current.is_empty() {
                    tokens.push(std::mem::take(&mut current));
                }
                tokens.push(ch.to_string());
            }
            ch if ch.is_whitespace() => {
                if !current.is_empty() {
                    tokens.push(std::mem::take(&mut current));
                }
            }
            _ => current.push(ch),
        }
    }

    if !current.is_empty() {
        tokens.push(current);
    }

    tokens
}

struct TokenCursor<'a> {
    tokens: &'a [String],
    index: usize,
    field: &'static str,
}

impl<'a> TokenCursor<'a> {
    fn new(tokens: &'a [String], field: &'static str) -> Self {
        Self {
            tokens,
            index: 0,
            field,
        }
    }

    fn peek(&self) -> Option<&str> {
        self.tokens.get(self.index).map(String::as_str)
    }

    fn next(&mut self) -> Option<&str> {
        let token = self.tokens.get(self.index).map(String::as_str);
        if token.is_some() {
            self.index += 1;
        }
        token
    }

    fn expect(&mut self, expected: &str) -> Result<(), UseResolverError> {
        let field = self.field;
        match self.next().map(str::to_owned) {
            Some(token) if token == expected => Ok(()),
            Some(token) => Err(UseResolverError::InvalidField {
                field,
                message: format!("expected `{expected}`, found `{token}`"),
            }),
            None => Err(UseResolverError::InvalidField {
                field,
                message: format!("expected `{expected}`, found end of input"),
            }),
        }
    }

    fn is_exhausted(&self) -> bool {
        self.index >= self.tokens.len()
    }

    fn invalid_field(&self, message: String) -> UseResolverError {
        UseResolverError::InvalidField {
            field: self.field,
            message,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::parse_required_use;
    use crate::use_resolver::RequiredUseExpr;

    #[test]
    fn parses_simple_flag_required_use() -> Result<(), Box<dyn std::error::Error>> {
        assert_eq!(
            parse_required_use("flag")?,
            vec![RequiredUseExpr::Flag("flag".to_owned())]
        );
        Ok(())
    }

    #[test]
    fn parses_negated_flag_required_use() -> Result<(), Box<dyn std::error::Error>> {
        assert_eq!(
            parse_required_use("!flag")?,
            vec![RequiredUseExpr::Not("flag".to_owned())]
        );
        Ok(())
    }

    #[test]
    fn parses_any_of_required_use() -> Result<(), Box<dyn std::error::Error>> {
        assert_eq!(
            parse_required_use("|| ( a b )")?,
            vec![RequiredUseExpr::AnyOf(vec![
                RequiredUseExpr::Flag("a".to_owned()),
                RequiredUseExpr::Flag("b".to_owned()),
            ])]
        );
        Ok(())
    }

    #[test]
    fn parses_exactly_one_required_use() -> Result<(), Box<dyn std::error::Error>> {
        assert_eq!(
            parse_required_use("^^ ( a b c )")?,
            vec![RequiredUseExpr::ExactlyOne(vec![
                RequiredUseExpr::Flag("a".to_owned()),
                RequiredUseExpr::Flag("b".to_owned()),
                RequiredUseExpr::Flag("c".to_owned()),
            ])]
        );
        Ok(())
    }

    #[test]
    fn parses_at_most_one_required_use() -> Result<(), Box<dyn std::error::Error>> {
        assert_eq!(
            parse_required_use("?? ( a b )")?,
            vec![RequiredUseExpr::AtMostOne(vec![
                RequiredUseExpr::Flag("a".to_owned()),
                RequiredUseExpr::Flag("b".to_owned()),
            ])]
        );
        Ok(())
    }

    #[test]
    fn parses_conditional_required_use() -> Result<(), Box<dyn std::error::Error>> {
        assert_eq!(
            parse_required_use("foo? ( bar )")?,
            vec![RequiredUseExpr::IfThen(
                "foo".to_owned(),
                vec![RequiredUseExpr::Flag("bar".to_owned())],
            )]
        );
        Ok(())
    }

    #[test]
    fn parses_nested_required_use() -> Result<(), Box<dyn std::error::Error>> {
        assert_eq!(
            parse_required_use("foo? ( || ( a b ) )")?,
            vec![RequiredUseExpr::IfThen(
                "foo".to_owned(),
                vec![RequiredUseExpr::AnyOf(vec![
                    RequiredUseExpr::Flag("a".to_owned()),
                    RequiredUseExpr::Flag("b".to_owned()),
                ])],
            )]
        );
        Ok(())
    }

    #[test]
    fn rejects_invalid_required_use() {
        assert!(parse_required_use("|| ( a b ").is_err());
    }
}
