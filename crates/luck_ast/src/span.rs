use luck_token::Span;

/// Where a node is in its source, or, for a node built in code, the span it
/// was built with.
///
/// A node with a `span` field returns it, and one without spans from its
/// first field to its last: a named table field from its name to its value.
/// `luck_ast_tools` generates it for every node. Ported from `oxc_span`'s
/// `GetSpan`.
pub trait GetSpan {
    fn span(&self) -> Span;
}
