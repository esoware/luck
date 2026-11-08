//! Name helpers shared by rules, mainly for resolving a `Name(.Name)*` access
//! against the stdlib or the Roblox `Enum` catalog.

use luck_ast::{Expression, Var};
use luck_token::Span;

/// Splits a `Name(.Name)*` chain into its names and their spans, root first.
/// Any other link, such as indexing or a call, yields `None`.
pub(crate) fn dotted_path(var: &Var) -> Option<(Vec<&str>, Vec<Span>)> {
    let mut segments: Vec<&str> = Vec::new();
    let mut spans: Vec<Span> = Vec::new();
    let mut cursor = var;
    loop {
        match cursor {
            Var::Name(token) => {
                segments.push(token.identifier()?);
                spans.push(token.span);
                break;
            }
            Var::FieldAccess(field_access) => {
                segments.push(field_access.name.identifier()?);
                spans.push(field_access.name.span);
                let Expression::Var(prefix) = &field_access.prefix else {
                    return None;
                };
                cursor = prefix;
            }
            Var::Index(_) => return None,
        }
    }
    segments.reverse();
    spans.reverse();
    Some((segments, spans))
}
