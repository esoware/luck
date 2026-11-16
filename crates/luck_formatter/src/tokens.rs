//! Leaf text never comes from source.
//!
//! Fixed-spelling kinds map to static strings, and literal kinds (identifiers, numbers, strings,
//! interpolation parts) carry their text in the `TokenKind`, so synthetic ASTs stay formattable.

use luck_token::{Token, TokenKind};

use crate::ir::{Format, Formatter, token};

impl Format for Token {
    #[expect(
        clippy::wildcard_enum_match_arm,
        reason = "every other kind has a fixed spelling, which static_text matches exhaustively"
    )]
    fn fmt(&self, f: &mut Formatter) {
        match &self.kind {
            // Interpolated-string parts don't reach this in practice.
            // `InterpolatedString::fmt` writes them itself, since their braces
            // depend on which segments hold an expression.
            TokenKind::Identifier(text)
            | TokenKind::Number(text)
            | TokenKind::Integer(text)
            | TokenKind::StringLiteral(text)
            | TokenKind::InterpolatedStringBegin(text)
            | TokenKind::InterpolatedStringMiddle(text)
            | TokenKind::InterpolatedStringEnd(text) => f.write_text(text.clone()),
            // `Eof` has no spelling, so it prints nothing.
            fixed => token(fixed.static_text().unwrap_or_default()).fmt(f),
        }
    }
}
