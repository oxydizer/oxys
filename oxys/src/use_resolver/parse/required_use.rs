use super::*;

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
pub(super) fn tokenize(value: &str) -> Vec<String> {
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

pub(super) struct TokenCursor<'a> {
    tokens: &'a [String],
    index: usize,
    field: &'static str,
}

impl<'a> TokenCursor<'a> {
    pub(super) fn new(tokens: &'a [String], field: &'static str) -> Self {
        Self {
            tokens,
            index: 0,
            field,
        }
    }

    pub(super) fn peek(&self) -> Option<&str> {
        self.tokens.get(self.index).map(String::as_str)
    }

    pub(super) fn next(&mut self) -> Option<&str> {
        let token = self.tokens.get(self.index).map(String::as_str);
        if token.is_some() {
            self.index += 1;
        }
        token
    }

    pub(super) fn expect(&mut self, expected: &str) -> Result<(), UseResolverError> {
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

    pub(super) fn is_exhausted(&self) -> bool {
        self.index >= self.tokens.len()
    }

    pub(super) fn invalid_field(&self, message: String) -> UseResolverError {
        UseResolverError::InvalidField {
            field: self.field,
            message,
        }
    }
}
