//! Token -> IR emission.
//!
//! Leaf text never comes from source. Fixed-spelling kinds map to static
//! strings, and literal kinds (identifiers, numbers, strings, interpolation
//! parts) carry their text in the `TokenKind`, so synthetic ASTs stay
//! formattable.

use luck_token::{Token, TokenKind};

use crate::ir::{Format, FormatElement, Formatter};

/// The IR element for a token kind: the text it carries, or its fixed
/// spelling.
#[expect(
    clippy::wildcard_enum_match_arm,
    reason = "every other kind has a fixed spelling, which static_text matches exhaustively"
)]
fn token_element(kind: &TokenKind) -> FormatElement {
    match kind {
        // Interpolated-string parts don't reach this in practice.
        // `InterpolatedString::fmt` writes them itself, since their braces
        // depend on which segments hold an expression.
        TokenKind::Identifier(text)
        | TokenKind::Number(text)
        | TokenKind::StringLiteral(text)
        | TokenKind::InterpolatedStringBegin(text)
        | TokenKind::InterpolatedStringMiddle(text)
        | TokenKind::InterpolatedStringEnd(text) => FormatElement::Text(text.clone()),
        // `Eof` has no spelling, so it prints nothing.
        fixed => FormatElement::Token(fixed.static_text().unwrap_or_default()),
    }
}

impl Format for Token {
    fn fmt(&self, formatter: &mut Formatter) {
        formatter.push(token_element(&self.kind));
    }
}
