//! Dotted-name path extraction shared by the rules that resolve a
//! `Name(.Name)*` access against the stdlib or Roblox `Enum` catalog
//! (`deprecated`, `roblox_unknown_enum_member`).

use luck_ast::Expression;
use luck_ast::expr::Var;
use luck_token::{Span, Token, TokenKind};

/// Unwind a `Name(.Name)*` chain into its segments and per-segment spans,
/// outermost last. Any non-name link (indexing, a call) aborts and yields
/// `None`.
pub(crate) fn dotted_path(expr: &Expression) -> Option<(Vec<&str>, Vec<Span>)> {
    let mut segments: Vec<&str> = Vec::new();
    let mut spans: Vec<Span> = Vec::new();
    let mut cursor = expr;
    loop {
        match cursor {
            Expression::Var(Var::Name(token)) => {
                segments.push(identifier(token)?);
                spans.push(token.span);
                break;
            }
            Expression::Var(Var::FieldAccess(field_access)) => {
                segments.push(identifier(&field_access.name)?);
                spans.push(field_access.name.span);
                cursor = &field_access.prefix;
            }
            _ => return None,
        }
    }
    segments.reverse();
    spans.reverse();
    Some((segments, spans))
}

fn identifier(token: &Token) -> Option<&str> {
    match &token.kind {
        TokenKind::Identifier(name) => Some(name.as_str()),
        _ => None,
    }
}
