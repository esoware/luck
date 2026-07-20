//! Token -> IR emission from carried token values.
//!
//! The formatter never slices source for leaf text: fixed-spelling kinds
//! map to static strings, and literal kinds (identifiers, numbers, strings,
//! interpolation parts) carry their text in the `TokenKind`. This is what
//! makes synthetic ASTs formattable.

use luck_token::{Token, TokenKind};

use crate::ir::{FormatElement, Formatter};

/// The fixed spelling of a token kind, or `None` for kinds that carry
/// their own text.
pub(crate) fn static_text(kind: &TokenKind) -> Option<&'static str> {
    match kind {
        TokenKind::Identifier(_)
        | TokenKind::Number(_)
        | TokenKind::StringLiteral(_)
        | TokenKind::InterpBegin(_)
        | TokenKind::InterpMid(_)
        | TokenKind::InterpEnd(_) => None,
        TokenKind::And => Some("and"),
        TokenKind::Break => Some("break"),
        TokenKind::Do => Some("do"),
        TokenKind::Else => Some("else"),
        TokenKind::ElseIf => Some("elseif"),
        TokenKind::End => Some("end"),
        TokenKind::False => Some("false"),
        TokenKind::For => Some("for"),
        TokenKind::Function => Some("function"),
        TokenKind::If => Some("if"),
        TokenKind::In => Some("in"),
        TokenKind::Local => Some("local"),
        TokenKind::Nil => Some("nil"),
        TokenKind::Not => Some("not"),
        TokenKind::Or => Some("or"),
        TokenKind::Repeat => Some("repeat"),
        TokenKind::Return => Some("return"),
        TokenKind::Then => Some("then"),
        TokenKind::True => Some("true"),
        TokenKind::Until => Some("until"),
        TokenKind::While => Some("while"),
        TokenKind::Goto => Some("goto"),     // Lua 5.2+
        TokenKind::Global => Some("global"), // Lua 5.5
        TokenKind::Plus => Some("+"),
        TokenKind::Minus => Some("-"),
        TokenKind::Star => Some("*"),
        TokenKind::Slash => Some("/"),
        TokenKind::FloorDiv => Some("//"), // Lua 5.3+
        TokenKind::Percent => Some("%"),
        TokenKind::Caret => Some("^"),
        TokenKind::Hash => Some("#"),
        TokenKind::Ampersand => Some("&"),   // Lua 5.3+
        TokenKind::Tilde => Some("~"),       // Lua 5.3+
        TokenKind::Pipe => Some("|"),        // Lua 5.3+
        TokenKind::ShiftLeft => Some("<<"),  // Lua 5.3+
        TokenKind::ShiftRight => Some(">>"), // Lua 5.3+
        TokenKind::Dot => Some("."),
        TokenKind::DotDot => Some(".."),
        TokenKind::DotDotDot => Some("..."),
        TokenKind::Semicolon => Some(";"),
        TokenKind::Colon => Some(":"),
        TokenKind::DoubleColon => Some("::"),
        TokenKind::Comma => Some(","),
        TokenKind::Equal => Some("="),
        TokenKind::EqualEqual => Some("=="),
        TokenKind::TildeEqual => Some("~="),
        TokenKind::Less => Some("<"),
        TokenKind::LessEqual => Some("<="),
        TokenKind::Greater => Some(">"),
        TokenKind::GreaterEqual => Some(">="),
        TokenKind::LeftParen => Some("("),
        TokenKind::RightParen => Some(")"),
        TokenKind::LeftBrace => Some("{"),
        TokenKind::RightBrace => Some("}"),
        TokenKind::LeftBracket => Some("["),
        TokenKind::RightBracket => Some("]"),
        TokenKind::PlusEqual => Some("+="),      // Luau
        TokenKind::MinusEqual => Some("-="),     // Luau
        TokenKind::StarEqual => Some("*="),      // Luau
        TokenKind::SlashEqual => Some("/="),     // Luau
        TokenKind::FloorDivEqual => Some("//="), // Luau
        TokenKind::PercentEqual => Some("%="),   // Luau
        TokenKind::CaretEqual => Some("^="),     // Luau
        TokenKind::DotDotEqual => Some("..="),   // Luau
        TokenKind::At => Some("@"),              // Luau
        TokenKind::Arrow => Some("->"),          // Luau
        TokenKind::Question => Some("?"),        // Luau
        TokenKind::Eof => Some(""),
    }
}

/// Push a token's text as the appropriate IR element.
pub(crate) fn write_token(f: &mut Formatter, token: &Token) {
    match &token.kind {
        TokenKind::Identifier(name) => f.push(FormatElement::Text(name.clone())),
        TokenKind::Number(number) => f.push(FormatElement::Text(number.clone())),
        TokenKind::StringLiteral(literal) => f.push(FormatElement::Text(literal.clone())),
        TokenKind::InterpBegin(_) | TokenKind::InterpMid(_) | TokenKind::InterpEnd(_) => {
            unreachable!(
                "interpolated-string parts are emitted by InterpolatedString::fmt, \
                 which derives their punctuation from expression presence"
            )
        }
        kind => {
            let content = static_text(kind).expect("carried-text kinds are matched above");
            f.push(FormatElement::Token(content));
        }
    }
}

/// A token as a `Format` value, for use inside `write!` sequences.
pub(crate) struct FormatToken<'a>(pub &'a Token);

impl crate::ir::Format for FormatToken<'_> {
    fn fmt(&self, f: &mut Formatter) {
        write_token(f, self.0);
    }
}
