use core::fmt;

use compact_str::CompactString;

use crate::{LuaVersion, Span};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TokenKind {
    Identifier(CompactString),
    Number(CompactString),
    /// Luau: a numeral with the `i` suffix, which reads as an exact 64-bit
    /// integer.
    Integer(CompactString),
    StringLiteral(CompactString),

    // Keywords (Lua 5.1 base)
    And,
    Break,
    Do,
    Else,
    ElseIf,
    End,
    False,
    For,
    Function,
    If,
    In,
    Local,
    Nil,
    Not,
    Or,
    Repeat,
    Return,
    Then,
    True,
    Until,
    While,

    // Lua 5.2+
    Goto,

    Plus,
    Minus,
    Star,
    Slash,
    FloorDiv,
    Percent,
    Caret,
    Hash,
    Ampersand,
    Tilde,
    Pipe,
    ShiftLeft,
    ShiftRight,
    Dot,
    DotDot,
    DotDotDot,
    Semicolon,
    Colon,
    DoubleColon,
    Comma,
    Equal,
    EqualEqual,
    TildeEqual,
    Less,
    LessEqual,
    Greater,
    GreaterEqual,
    LeftParen,
    RightParen,
    LeftBrace,
    RightBrace,
    LeftBracket,
    RightBracket,

    // Luau compound assignment operators
    PlusEqual,
    MinusEqual,
    StarEqual,
    SlashEqual,
    FloorDivEqual,
    PercentEqual,
    CaretEqual,
    DotDotEqual,

    // Luau interpolated string tokens
    InterpolatedStringBegin(CompactString),  // `text{
    InterpolatedStringMiddle(CompactString), // }text{
    InterpolatedStringEnd(CompactString),    // }text`

    // Luau
    At,
    Arrow,
    Question,

    Eof,
}

impl fmt::Display for TokenKind {
    #[expect(
        clippy::wildcard_enum_match_arm,
        reason = "every other kind has a fixed spelling, which static_text matches exhaustively"
    )]
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Identifier(name) => write!(f, "identifier `{name}`"),
            Self::Number(text) => write!(f, "number `{text}`"),
            Self::Integer(text) => write!(f, "integer `{text}`"),
            Self::StringLiteral(_) => f.write_str("string"),
            Self::InterpolatedStringBegin(_) => f.write_str("interpolated string start"),
            Self::InterpolatedStringMiddle(_) => f.write_str("interpolated string middle"),
            Self::InterpolatedStringEnd(_) => f.write_str("interpolated string end"),
            Self::Eof => f.write_str("end of file"),
            fixed => write!(f, "`{}`", fixed.static_text().unwrap_or_default()),
        }
    }
}

impl TokenKind {
    /// The exact source spelling, or `None` for a kind that carries its text
    /// and for `Eof`, which has none.
    #[inline]
    #[must_use]
    pub const fn static_text(&self) -> Option<&'static str> {
        Some(match self {
            Self::Identifier(_)
            | Self::Number(_)
            | Self::Integer(_)
            | Self::StringLiteral(_)
            | Self::InterpolatedStringBegin(_)
            | Self::InterpolatedStringMiddle(_)
            | Self::InterpolatedStringEnd(_)
            | Self::Eof => return None,
            Self::And => "and",
            Self::Break => "break",
            Self::Do => "do",
            Self::Else => "else",
            Self::ElseIf => "elseif",
            Self::End => "end",
            Self::False => "false",
            Self::For => "for",
            Self::Function => "function",
            Self::If => "if",
            Self::In => "in",
            Self::Local => "local",
            Self::Nil => "nil",
            Self::Not => "not",
            Self::Or => "or",
            Self::Repeat => "repeat",
            Self::Return => "return",
            Self::Then => "then",
            Self::True => "true",
            Self::Until => "until",
            Self::While => "while",
            Self::Goto => "goto", // Lua 5.2+
            Self::Plus => "+",
            Self::Minus => "-",
            Self::Star => "*",
            Self::Slash => "/",
            Self::FloorDiv => "//", // Lua 5.3+, Luau
            Self::Percent => "%",
            Self::Caret => "^",
            Self::Hash => "#",
            Self::Ampersand => "&",   // Lua 5.3+
            Self::Tilde => "~",       // Lua 5.3+
            Self::Pipe => "|",        // Lua 5.3+
            Self::ShiftLeft => "<<",  // Lua 5.3+
            Self::ShiftRight => ">>", // Lua 5.3+
            Self::Dot => ".",
            Self::DotDot => "..",
            Self::DotDotDot => "...",
            Self::Semicolon => ";",
            Self::Colon => ":",
            Self::DoubleColon => "::",
            Self::Comma => ",",
            Self::Equal => "=",
            Self::EqualEqual => "==",
            Self::TildeEqual => "~=",
            Self::Less => "<",
            Self::LessEqual => "<=",
            Self::Greater => ">",
            Self::GreaterEqual => ">=",
            Self::LeftParen => "(",
            Self::RightParen => ")",
            Self::LeftBrace => "{",
            Self::RightBrace => "}",
            Self::LeftBracket => "[",
            Self::RightBracket => "]",
            Self::PlusEqual => "+=",      // Luau
            Self::MinusEqual => "-=",     // Luau
            Self::StarEqual => "*=",      // Luau
            Self::SlashEqual => "/=",     // Luau
            Self::FloorDivEqual => "//=", // Luau
            Self::PercentEqual => "%=",   // Luau
            Self::CaretEqual => "^=",     // Luau
            Self::DotDotEqual => "..=",   // Luau
            Self::At => "@",              // Luau
            Self::Arrow => "->",          // Luau
            Self::Question => "?",        // Luau
        })
    }

    /// The keyword `text` spells under `version`, or `None` when it's a name
    /// there, as `goto` is on 5.1 and Luau.
    ///
    /// Every Lua keyword is 2-8 bytes of lowercase ASCII, so a length or
    /// first-byte check rejects many identifiers before the match, which LLVM
    /// compiles into a length-first switch.
    #[inline]
    #[must_use]
    pub fn keyword(text: &str, version: LuaVersion) -> Option<Self> {
        if text.len() < 2
            || text.len() > 8
            || !text.as_bytes().first().is_some_and(u8::is_ascii_lowercase)
        {
            return None;
        }
        let kind = match text {
            "and" => Self::And,
            "break" => Self::Break,
            "do" => Self::Do,
            "else" => Self::Else,
            "elseif" => Self::ElseIf,
            "end" => Self::End,
            "false" => Self::False,
            "for" => Self::For,
            "function" => Self::Function,
            "if" => Self::If,
            "in" => Self::In,
            "local" => Self::Local,
            "nil" => Self::Nil,
            "not" => Self::Not,
            "or" => Self::Or,
            "repeat" => Self::Repeat,
            "return" => Self::Return,
            "then" => Self::Then,
            "true" => Self::True,
            "until" => Self::Until,
            "while" => Self::While,
            // Lua 5.2+
            "goto" if version.has_goto() => Self::Goto,
            _ => return None,
        };
        Some(kind)
    }

    /// Whether `text` is a keyword under `version`, counting Luau's
    /// contextual `continue` and Lua 5.5's contextual `global` as ones.
    #[must_use]
    pub fn is_reserved(text: &str, version: LuaVersion) -> bool {
        Self::keyword(text, version).is_some()
            || (version.has_continue() && text == "continue")
            || (version.has_global() && text == "global")
    }

    /// Whether `text` is spelled like an identifier: an ASCII letter or `_`,
    /// then ASCII letters, digits, and `_`. Keywords are spelled that way too,
    /// so a name check pairs this with a keyword check such as
    /// [`Self::keyword`] or [`Self::is_reserved`].
    #[must_use]
    pub fn is_identifier_shaped(text: &str) -> bool {
        let mut bytes = text.bytes();
        bytes.next().is_some_and(|b| b.is_ascii_alphabetic() || b == b'_')
            && bytes.all(|b| b.is_ascii_alphanumeric() || b == b'_')
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Token {
    pub kind: TokenKind,
    pub span: Span,
}

impl Token {
    #[must_use]
    pub const fn new(kind: TokenKind, span: Span) -> Self {
        Self { kind, span }
    }

    /// The name this token spells, or `None` for any other token and for the
    /// empty placeholder parse recovery leaves where a name was missing.
    #[inline]
    #[must_use]
    pub fn identifier(&self) -> Option<&str> {
        if let TokenKind::Identifier(name) = &self.kind
            && !name.is_empty()
        {
            Some(name.as_str())
        } else {
            None
        }
    }
}
