//! Name helpers shared by rules, mainly for resolving a `Name(.Name)*` access
//! against the stdlib or the Roblox `Enum` catalog.

use luck_ast::{Expression, Var};
use luck_token::Span;

/// A `Name(.Name)*` chain's names, root first, and where its first and last
/// names sit.
pub(crate) struct DottedPath<'ast> {
    pub segments: Vec<&'ast str>,
    pub root_span: Span,
    pub leaf_span: Span,
}

/// Splits a `Name(.Name)*` chain into its names. Any other link, such as
/// indexing or a call, yields `None`.
pub(crate) fn dotted_path(var: &Var) -> Option<DottedPath<'_>> {
    let leaf_span = match var {
        Var::Name(token) => token.span,
        Var::FieldAccess(field_access) => field_access.name.span,
        Var::Index(_) => return None,
    };
    let mut segments: Vec<&str> = Vec::new();
    let mut cursor = var;
    let root_span = loop {
        match cursor {
            Var::Name(token) => {
                segments.push(token.identifier()?);
                break token.span;
            }
            Var::FieldAccess(field_access) => {
                segments.push(field_access.name.identifier()?);
                let Expression::Var(prefix) = &field_access.prefix else {
                    return None;
                };
                cursor = prefix;
            }
            Var::Index(_) => return None,
        }
    };
    segments.reverse();
    Some(DottedPath { segments, root_span, leaf_span })
}
