use core::fmt;

use compact_str::CompactString;

use crate::{LuaVersion, Span};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TokenKind {
    // Literals
    Identifier(CompactString),
    Number(CompactString),
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

    // Lua 5.5
    Global,

    // Symbols
    Plus,         // +
    Minus,        // -
    Star,         // *
    Slash,        // /
    FloorDiv,     // //
    Percent,      // %
    Caret,        // ^
    Hash,         // #
    Ampersand,    // &
    Tilde,        // ~
    Pipe,         // |
    ShiftLeft,    // <<
    ShiftRight,   // >>
    Dot,          // .
    DotDot,       // ..
    DotDotDot,    // ...
    Semicolon,    // ;
    Colon,        // :
    DoubleColon,  // ::
    Comma,        // ,
    Equal,        // =
    EqualEqual,   // ==
    TildeEqual,   // ~=
    Less,         // <
    LessEqual,    // <=
    Greater,      // >
    GreaterEqual, // >=
    LeftParen,    // (
    RightParen,   // )
    LeftBrace,    // {
    RightBrace,   // }
    LeftBracket,  // [
    RightBracket, // ]

    // Luau compound assignment operators
    PlusEqual,     // +=
    MinusEqual,    // -=
    StarEqual,     // *=
    SlashEqual,    // /=
    FloorDivEqual, // //=
    PercentEqual,  // %=
    CaretEqual,    // ^=
    DotDotEqual,   // ..=

    // Luau interpolated string tokens
    InterpolatedStringBegin(CompactString),  // `text{
    InterpolatedStringMiddle(CompactString), // }text{
    InterpolatedStringEnd(CompactString),    // }text`

    // Luau
    At,       // @
    Arrow,    // ->
    Question, // ?

    // Special
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
            Self::Goto => "goto",     // Lua 5.2+
            Self::Global => "global", // Lua 5.5
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
            // Lua 5.5
            "global" if version.has_global() => Self::Global,
            _ => return None,
        };
        Some(kind)
    }

    /// Whether `text` is a keyword under `version`, counting Luau's
    /// contextual `continue` as one.
    #[must_use]
    pub fn is_reserved(text: &str, version: LuaVersion) -> bool {
        Self::keyword(text, version).is_some() || (version.has_continue() && text == "continue")
    }

    /// Whether `text` is spelled like an identifier: an ASCII letter or `_`,
    /// then ASCII letters, digits, and `_`. Keywords are spelled that way too,
    /// so a name check pairs this with a keyword check such as
    /// [`Self::keyword`] or [`Self::is_reserved`].
    #[must_use]
    pub fn is_identifier_shaped(text: &str) -> bool {
        let mut bytes = text.bytes();
        bytes.next().is_some_and(|first| first.is_ascii_alphabetic() || first == b'_')
            && bytes.all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
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

    #[inline]
    #[must_use]
    pub fn identifier(&self) -> Option<&str> {
        if let TokenKind::Identifier(name) = &self.kind { Some(name.as_str()) } else { None }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // `lex` returns every token in one buffer, so a size regression costs
    // memory on every file. `CompactString` (24) plus tag padding puts
    // `TokenKind` at 32, and `Span` (8) brings `Token` to 40.
    #[test]
    #[cfg(target_pointer_width = "64")]
    fn token_size_is_pinned() {
        assert_eq!(size_of::<TokenKind>(), 32);
        assert_eq!(size_of::<Token>(), 40);
        assert_eq!(size_of::<Option<Token>>(), 40);
    }

    #[test]
    fn compact_string_keeps_short_identifiers_inline() {
        // The README promises that short strings skip the heap.
        assert!(!CompactString::from("short_ident").is_heap_allocated());
        assert!(CompactString::from("x".repeat(64)).is_heap_allocated());
    }

    #[test]
    fn display_shows_identifier_and_number_text_but_not_string_text() {
        assert_eq!(
            TokenKind::Identifier(CompactString::from("foo")).to_string(),
            "identifier `foo`"
        );
        assert_eq!(TokenKind::Number(CompactString::from("42")).to_string(), "number `42`");
        // The string payload is deliberately not echoed.
        assert_eq!(TokenKind::StringLiteral(CompactString::from("secret")).to_string(), "string");
        assert_eq!(
            TokenKind::InterpolatedStringMiddle(CompactString::from("secret")).to_string(),
            "interpolated string middle"
        );
    }

    #[test]
    fn display_quotes_symbols_and_keywords() {
        assert_eq!(TokenKind::And.to_string(), "`and`");
        assert_eq!(TokenKind::DotDotDot.to_string(), "`...`");
        assert_eq!(TokenKind::LeftBrace.to_string(), "`{`");
        assert_eq!(TokenKind::RightBrace.to_string(), "`}`");
        assert_eq!(TokenKind::Arrow.to_string(), "`->`");
        assert_eq!(TokenKind::Eof.to_string(), "end of file");
    }

    #[test]
    fn static_text_is_none_only_for_text_carrying_kinds_and_eof() {
        assert_eq!(TokenKind::Global.static_text(), Some("global"));
        assert_eq!(TokenKind::DoubleColon.static_text(), Some("::"));
        assert_eq!(TokenKind::Eof.static_text(), None);
        assert_eq!(TokenKind::Identifier(CompactString::from("end")).static_text(), None);
        assert_eq!(
            TokenKind::InterpolatedStringBegin(CompactString::from("x")).static_text(),
            None
        );
    }

    #[test]
    fn keyword_reads_back_each_keyword_spelling_under_its_versions() {
        let base_keywords = [
            TokenKind::And,
            TokenKind::Break,
            TokenKind::Do,
            TokenKind::Else,
            TokenKind::ElseIf,
            TokenKind::End,
            TokenKind::False,
            TokenKind::For,
            TokenKind::Function,
            TokenKind::If,
            TokenKind::In,
            TokenKind::Local,
            TokenKind::Nil,
            TokenKind::Not,
            TokenKind::Or,
            TokenKind::Repeat,
            TokenKind::Return,
            TokenKind::Then,
            TokenKind::True,
            TokenKind::Until,
            TokenKind::While,
        ];
        for version in LuaVersion::ALL {
            for kind in &base_keywords {
                let text = kind.static_text().unwrap_or_default();
                assert_eq!(TokenKind::keyword(text, version).as_ref(), Some(kind));
            }
            let goto = version.has_goto().then_some(TokenKind::Goto);
            assert_eq!(TokenKind::keyword("goto", version), goto, "{version:?}");
            let global = version.has_global().then_some(TokenKind::Global);
            assert_eq!(TokenKind::keyword("global", version), global, "{version:?}");
            for name in ["", "x", "End", "functions", "continue", "type"] {
                assert_eq!(TokenKind::keyword(name, version), None, "{name:?}");
            }
        }
    }

    #[test]
    fn is_reserved_adds_luau_continue_to_each_version_keywords() {
        for version in LuaVersion::ALL {
            assert!(TokenKind::is_reserved("while", version), "{version:?}");
            assert_eq!(TokenKind::is_reserved("goto", version), version.has_goto(), "{version:?}");
            assert_eq!(
                TokenKind::is_reserved("global", version),
                version.has_global(),
                "{version:?}"
            );
            assert_eq!(
                TokenKind::is_reserved("continue", version),
                version.has_continue(),
                "{version:?}"
            );
            for name in ["", "x", "type", "export", "While"] {
                assert!(!TokenKind::is_reserved(name, version), "{name:?} {version:?}");
            }
        }
    }

    #[test]
    fn is_identifier_shaped_holds_for_ascii_names_keywords_included() {
        for text in ["x", "_", "_private", "snake_case2", "camelCase", "end", "goto"] {
            assert!(TokenKind::is_identifier_shaped(text), "{text:?}");
        }
        for text in ["", "1st", "a-b", "has space", "a.b", "a\\nb", "\u{dc}nicode", "x\u{e9}"] {
            assert!(!TokenKind::is_identifier_shaped(text), "{text:?}");
        }
    }

    #[test]
    fn identifier_reads_only_identifier_names() {
        let span = Span::new(0, 3);
        let name = Token::new(TokenKind::Identifier(CompactString::from("foo")), span);
        assert_eq!(name.identifier(), Some("foo"));
        let string = Token::new(TokenKind::StringLiteral(CompactString::from("\"foo\"")), span);
        assert_eq!(string.identifier(), None);
        assert_eq!(Token::new(TokenKind::Nil, span).identifier(), None);
    }
}
