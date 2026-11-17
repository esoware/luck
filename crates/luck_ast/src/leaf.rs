use luck_token::Span;

/// An identifier: a variable, a field, a parameter, a label, a type, or an
/// attribute. A name the parser found missing has empty text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Name<'ast> {
    pub text: &'ast str,
    pub span: Span,
}

/// A number, an integer, a string, or a piece of an interpolated string, as
/// written.
///
/// A string keeps its quotes or long brackets and its escapes, and an
/// interpolated piece is what lies between its delimiters. The node holding it
/// says which it is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Literal<'ast> {
    pub text: &'ast str,
    pub span: Span,
}
