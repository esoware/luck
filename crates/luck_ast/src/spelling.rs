//! The text each leaf prints as, or the error a printer returns for a leaf
//! with none that reads back as itself where it sits.
//!
//! `luck_codegen` and `luck_formatter` take every leaf from here, so they
//! print and refuse the same trees. Neither knows the target dialect, so a
//! leaf passes when it reads back in some dialect, as the name `goto` does.

use luck_token::{
    SourceError, Span, Token, TokenKind, is_interpolated_text, is_numeral, is_string_literal,
};

use crate::expr::{InterpolatedString, Literal};
use crate::synth::is_valid_identifier;
use crate::types::{Type, TypeField};

/// # Errors
///
/// When `token` isn't an identifier, or its text isn't a name in any dialect,
/// as the empty placeholder parse recovery leaves for a missing name isn't.
pub fn name_spelling(token: &Token) -> Result<&str, SourceError> {
    if let TokenKind::Identifier(name) = &token.kind
        && is_valid_identifier(name)
    {
        return Ok(name);
    }
    Err(SourceError {
        span: token.span,
        message: format!("expected identifier, found {}", described(&token.kind)),
    })
}

/// For a name that starts a Luau type, where `typeof` without a module before
/// it opens a `typeof(...)` type instead.
///
/// # Errors
///
/// When `name` isn't a name in any dialect, or is `typeof`.
pub fn type_name_spelling(name: &str, span: Span) -> Result<&str, SourceError> {
    if is_valid_identifier(name) && name != "typeof" {
        return Ok(name);
    }
    Err(SourceError { span, message: format!("expected type name, found {}", quoted(name)) })
}

/// # Errors
///
/// When the text isn't one numeral in any dialect, or ends in a Luau `i`,
/// which makes it an [`Expression::Integer`](crate::Expression::Integer).
pub fn number_spelling(literal: &Literal) -> Result<&str, SourceError> {
    let text = literal.text.as_str();
    if is_numeral(text) && !text.ends_with('i') {
        return Ok(text);
    }
    Err(SourceError {
        span: literal.span,
        message: format!("expected number literal, found {}", quoted(text)),
    })
}

/// # Errors
///
/// When the text isn't one numeral that ends in a Luau `i`.
pub fn integer_spelling(literal: &Literal) -> Result<&str, SourceError> {
    let text = literal.text.as_str();
    if is_numeral(text) && text.ends_with('i') {
        return Ok(text);
    }
    Err(SourceError {
        span: literal.span,
        message: format!("expected integer literal, found {}", quoted(text)),
    })
}

/// # Errors
///
/// When the text isn't one quoted or long-bracket string in any dialect.
pub fn string_spelling(literal: &Literal) -> Result<&str, SourceError> {
    string_text_spelling(&literal.text, literal.span)
}

/// # Errors
///
/// As [`string_spelling`] does.
fn string_text_spelling(text: &str, span: Span) -> Result<&str, SourceError> {
    if is_string_literal(text) {
        return Ok(text);
    }
    Err(SourceError { span, message: format!("expected string literal, found {}", quoted(text)) })
}

/// A singleton type is a string, `true`, `false`, or `nil`, or a number,
/// which the parser reads there too so sources carrying one round-trip.
///
/// # Errors
///
/// When `token` is any other kind, or its text isn't one literal of its kind
/// in any dialect.
pub fn singleton_spelling(token: &Token) -> Result<&str, SourceError> {
    if let TokenKind::StringLiteral(text) = &token.kind {
        return string_text_spelling(text, token.span);
    }
    if let TokenKind::Number(text) = &token.kind {
        if is_numeral(text) {
            return Ok(text);
        }
        return Err(SourceError {
            span: token.span,
            message: format!("expected number literal, found {}", quoted(text)),
        });
    }
    if let Some(keyword @ ("true" | "false" | "nil")) = token.kind.static_text() {
        return Ok(keyword);
    }
    Err(SourceError {
        span: token.span,
        message: format!("expected singleton type, found {}", described(&token.kind)),
    })
}

/// # Errors
///
/// When `token` isn't the identifier `read` or `write`.
pub fn access_spelling(token: &Token) -> Result<&str, SourceError> {
    if let TokenKind::Identifier(name) = &token.kind
        && matches!(name.as_str(), "read" | "write")
    {
        return Ok(name);
    }
    Err(SourceError {
        span: token.span,
        message: format!("expected `read` or `write`, found {}", described(&token.kind)),
    })
}

/// A bare `read` or `write` that opens a table type field reads back as its
/// access modifier.
///
/// # Errors
///
/// For an array field with no access modifier whose element type opens with
/// the name `read` or `write`.
pub fn check_type_field(field: &TypeField) -> Result<(), SourceError> {
    if let TypeField::Array { access: None, value, .. } = field
        && let Some((name, span)) = leading_access_word(value)
    {
        return Err(SourceError {
            span,
            message: format!(
                "`{name}` opening a table type field reads back as an access modifier"
            ),
        });
    }
    Ok(())
}

/// The name `ty` prints first, when that's `read` or `write`.
pub(crate) fn leading_access_word(ty: &Type) -> Option<(&str, Span)> {
    let mut current = ty;
    let (name, span) = loop {
        current = match current {
            Type::Name { name, span } => break (name.as_str(), *span),
            Type::Named(named) => {
                let first = named.prefix.as_ref().unwrap_or(&named.name);
                break (first.identifier()?, first.span);
            }
            Type::GenericPack(pack) => break (pack.name.identifier()?, pack.name.span),
            Type::Optional(optional) => &optional.type_value,
            Type::Union(union) if !union.has_leading_pipe => union.types.first()?,
            Type::Intersection(intersection) if !intersection.has_leading_ampersand => {
                intersection.types.first()?
            }
            Type::Union(_)
            | Type::Intersection(_)
            | Type::Typeof(_)
            | Type::Table(_)
            | Type::Function(_)
            | Type::Parenthesized(_)
            | Type::Pack(_)
            | Type::Singleton(_)
            | Type::Variadic(_)
            | Type::Error(_) => return None,
        };
    };
    matches!(name, "read" | "write").then_some((name, span))
}

/// Whether the segments are shaped as the lexer reads them.
///
/// It reads a string with no expressions as an empty `InterpolatedStringBegin`
/// and an `InterpolatedStringEnd` holding its text, and one with expressions
/// as a `Begin`, a `Middle` before each expression after the first, and an
/// `End`.
///
/// # Errors
///
/// For segments in any other shape, or text that isn't one segment's raw
/// text, escapes as written.
pub fn check_interpolated_string(string: &InterpolatedString) -> Result<(), SourceError> {
    let [first, .., _] = string.segments.as_slice() else {
        return Err(SourceError {
            span: string.span,
            message: "malformed interpolated string".to_owned(),
        });
    };
    let last_index = string.segments.len().saturating_sub(1);
    let has_holes = first.expr.is_some();
    for (i, segment) in string.segments.iter().enumerate() {
        let kind = &segment.literal.kind;
        let has_expr = segment.expr.is_some();
        let text = if let TokenKind::InterpolatedStringBegin(text) = kind
            && i == 0
            && (has_holes || (last_index == 1 && text.is_empty()))
        {
            text
        } else if let TokenKind::InterpolatedStringMiddle(text) = kind
            && i > 0
            && i < last_index
            && has_expr
        {
            text
        } else if let TokenKind::InterpolatedStringEnd(text) = kind
            && i == last_index
            && !has_expr
        {
            text
        } else {
            return Err(SourceError {
                span: segment.literal.span,
                message: "malformed interpolated string".to_owned(),
            });
        };
        if !is_interpolated_text(text) {
            return Err(SourceError {
                span: segment.literal.span,
                message: format!("expected interpolated string text, found {}", quoted(text)),
            });
        }
    }
    Ok(())
}

/// The error a printer returns for an error node, which stands where parse
/// recovery found no construct.
#[must_use]
pub fn error_node(span: Span) -> SourceError {
    SourceError { span, message: "an error node has no text".to_owned() }
}

fn described(kind: &TokenKind) -> String {
    if let TokenKind::Identifier(name) = kind { quoted(name) } else { kind.to_string() }
}

fn quoted(text: &str) -> String {
    if text.is_empty() { "nothing".to_owned() } else { format!("`{}`", text.escape_debug()) }
}
