use std::fmt;

use crate::{LuaVersion, Span};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TokenKind {
    Identifier,

    // Contextual keywords, each a name wherever its keyword reading doesn't
    // apply, and a name in every version that lacks it. They follow
    // `Identifier`, so `is_name` is one comparison.
    Continue, // Luau
    Export,   // Luau
    Const,    // Luau
    Type,     // Luau
    Typeof,   // Luau
    Read,     // Luau
    Write,    // Luau
    Global,   // Lua 5.5

    Number,
    /// Luau: a numeral with the `i` suffix, which reads as an exact 64-bit
    /// integer.
    Integer,
    StringLiteral,

    // Luau interpolated string pieces, each spanning its delimiters
    InterpolatedStringBegin,  // `text{
    InterpolatedStringMiddle, // }text{
    InterpolatedStringEnd,    // }text`
    InterpolatedStringSimple, // `text`, which holds no expression

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

    // Luau
    At,
    Arrow,
    Question,

    Eof,
}

impl fmt::Display for TokenKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.spelling() {
            Spelling::Fixed(text) => write!(f, "`{text}`"),
            Spelling::Described(description) => f.write_str(description),
        }
    }
}

/// A kind's exact source spelling, or for a kind whose text varies and for
/// `Eof`, which has none, what an error message calls it.
enum Spelling {
    Fixed(&'static str),
    Described(&'static str),
}

impl TokenKind {
    /// The exact source spelling, or `None` for a kind whose text varies and
    /// for `Eof`, which has none.
    #[inline]
    pub const fn static_text(self) -> Option<&'static str> {
        match self.spelling() {
            Spelling::Fixed(text) => Some(text),
            Spelling::Described(_) => None,
        }
    }

    const fn spelling(self) -> Spelling {
        Spelling::Fixed(match self {
            Self::Identifier => return Spelling::Described("identifier"),
            Self::Number => return Spelling::Described("number"),
            Self::Integer => return Spelling::Described("integer"),
            Self::StringLiteral => return Spelling::Described("string"),
            Self::InterpolatedStringBegin => {
                return Spelling::Described("interpolated string start");
            }
            Self::InterpolatedStringMiddle => {
                return Spelling::Described("interpolated string middle");
            }
            Self::InterpolatedStringEnd => return Spelling::Described("interpolated string end"),
            Self::InterpolatedStringSimple => return Spelling::Described("interpolated string"),
            Self::Eof => return Spelling::Described("end of file"),
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
            Self::Goto => "goto",         // Lua 5.2+
            Self::Continue => "continue", // Luau
            Self::Export => "export",     // Luau
            Self::Const => "const",       // Luau
            Self::Type => "type",         // Luau
            Self::Typeof => "typeof",     // Luau
            Self::Read => "read",         // Luau
            Self::Write => "write",       // Luau
            Self::Global => "global",     // Lua 5.5
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

    /// The keyword or contextual keyword `text` spells under `version`, as the
    /// lexer reads it, or `None` when it's an identifier there, as `goto` is
    /// on 5.1 and Luau and `type` is outside Luau.
    #[inline]
    pub fn keyword(text: &str, version: LuaVersion) -> Option<Self> {
        let packed = pack_keyword_candidate(text.as_bytes())?;
        let (keyword, kind) = KEYWORDS[keyword_slot(packed)];
        (keyword == packed && kind.is_keyword_in(version)).then_some(kind)
    }

    /// Whether code luck writes can't name a variable `text` under
    /// `version`: a keyword, or Lua 5.5's `global`, which builds without
    /// `LUA_COMPAT_GLOBAL` reserve. Every other contextual keyword is a name
    /// wherever a name goes.
    pub fn is_reserved(text: &str, version: LuaVersion) -> bool {
        Self::keyword(text, version).is_some_and(|kind| !kind.is_name() || kind == Self::Global)
    }

    /// Whether the parser reads this kind as a name where one goes: an
    /// identifier, or a contextual keyword.
    #[inline]
    pub const fn is_name(self) -> bool {
        match self {
            Self::Identifier
            | Self::Continue
            | Self::Export
            | Self::Const
            | Self::Type
            | Self::Typeof
            | Self::Read
            | Self::Write
            | Self::Global => true,
            Self::Number
            | Self::Integer
            | Self::StringLiteral
            | Self::InterpolatedStringBegin
            | Self::InterpolatedStringMiddle
            | Self::InterpolatedStringEnd
            | Self::InterpolatedStringSimple
            | Self::And
            | Self::Break
            | Self::Do
            | Self::Else
            | Self::ElseIf
            | Self::End
            | Self::False
            | Self::For
            | Self::Function
            | Self::If
            | Self::In
            | Self::Local
            | Self::Nil
            | Self::Not
            | Self::Or
            | Self::Repeat
            | Self::Return
            | Self::Then
            | Self::True
            | Self::Until
            | Self::While
            | Self::Goto
            | Self::Plus
            | Self::Minus
            | Self::Star
            | Self::Slash
            | Self::FloorDiv
            | Self::Percent
            | Self::Caret
            | Self::Hash
            | Self::Ampersand
            | Self::Tilde
            | Self::Pipe
            | Self::ShiftLeft
            | Self::ShiftRight
            | Self::Dot
            | Self::DotDot
            | Self::DotDotDot
            | Self::Semicolon
            | Self::Colon
            | Self::DoubleColon
            | Self::Comma
            | Self::Equal
            | Self::EqualEqual
            | Self::TildeEqual
            | Self::Less
            | Self::LessEqual
            | Self::Greater
            | Self::GreaterEqual
            | Self::LeftParen
            | Self::RightParen
            | Self::LeftBrace
            | Self::RightBrace
            | Self::LeftBracket
            | Self::RightBracket
            | Self::PlusEqual
            | Self::MinusEqual
            | Self::StarEqual
            | Self::SlashEqual
            | Self::FloorDivEqual
            | Self::PercentEqual
            | Self::CaretEqual
            | Self::DotDotEqual
            | Self::At
            | Self::Arrow
            | Self::Question
            | Self::Eof => false,
        }
    }

    /// Whether `version` reads this keyword's spelling as the keyword, which
    /// holds for every keyword but those only some versions have.
    const fn is_keyword_in(self, version: LuaVersion) -> bool {
        match self {
            // Lua 5.2+
            Self::Goto => version.has_goto(),
            // Luau
            Self::Continue => version.has_continue(),
            // Luau
            Self::Const => version.has_const_stmt(),
            // Luau
            Self::Export | Self::Type | Self::Typeof | Self::Read | Self::Write => {
                version.has_type_syntax()
            }
            // Lua 5.5
            Self::Global => version.has_global(),
            Self::And
            | Self::Break
            | Self::Do
            | Self::Else
            | Self::ElseIf
            | Self::End
            | Self::False
            | Self::For
            | Self::Function
            | Self::If
            | Self::In
            | Self::Local
            | Self::Nil
            | Self::Not
            | Self::Or
            | Self::Repeat
            | Self::Return
            | Self::Then
            | Self::True
            | Self::Until
            | Self::While => true,
            Self::Identifier
            | Self::Number
            | Self::Integer
            | Self::StringLiteral
            | Self::InterpolatedStringBegin
            | Self::InterpolatedStringMiddle
            | Self::InterpolatedStringEnd
            | Self::InterpolatedStringSimple
            | Self::Plus
            | Self::Minus
            | Self::Star
            | Self::Slash
            | Self::FloorDiv
            | Self::Percent
            | Self::Caret
            | Self::Hash
            | Self::Ampersand
            | Self::Tilde
            | Self::Pipe
            | Self::ShiftLeft
            | Self::ShiftRight
            | Self::Dot
            | Self::DotDot
            | Self::DotDotDot
            | Self::Semicolon
            | Self::Colon
            | Self::DoubleColon
            | Self::Comma
            | Self::Equal
            | Self::EqualEqual
            | Self::TildeEqual
            | Self::Less
            | Self::LessEqual
            | Self::Greater
            | Self::GreaterEqual
            | Self::LeftParen
            | Self::RightParen
            | Self::LeftBrace
            | Self::RightBrace
            | Self::LeftBracket
            | Self::RightBracket
            | Self::PlusEqual
            | Self::MinusEqual
            | Self::StarEqual
            | Self::SlashEqual
            | Self::FloorDivEqual
            | Self::PercentEqual
            | Self::CaretEqual
            | Self::DotDotEqual
            | Self::At
            | Self::Arrow
            | Self::Question
            | Self::Eof => false,
        }
    }

    /// Whether `text` is spelled like an identifier: an ASCII letter or `_`,
    /// then ASCII letters, digits, and `_`. Keywords are spelled that way too,
    /// so a name check pairs this with a keyword check such as
    /// [`Self::is_reserved`].
    pub fn is_identifier_shaped(text: &str) -> bool {
        let mut bytes = text.bytes();
        bytes.next().is_some_and(|b| b.is_ascii_alphabetic() || b == b'_')
            && bytes.all(|b| b.is_ascii_alphanumeric() || b == b'_')
    }
}

/// Every keyword is 2 to 8 bytes, so its spelling packs into a `u64`.
const KEYWORD_KINDS: [TokenKind; 30] = [
    TokenKind::And,
    TokenKind::Break,
    TokenKind::Do,
    TokenKind::Else,
    TokenKind::ElseIf,
    TokenKind::End,
    TokenKind::False,
    TokenKind::For,
    TokenKind::Function,
    TokenKind::Goto,
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
    TokenKind::Continue,
    TokenKind::Export,
    TokenKind::Const,
    TokenKind::Type,
    TokenKind::Typeof,
    TokenKind::Read,
    TokenKind::Write,
    TokenKind::Global,
];

/// Gives each keyword its own slot. Building [`KEYWORDS`] fails to compile on
/// a collision, so a new keyword may need a new multiplier.
const KEYWORD_HASH_MULTIPLIER: u64 = 0x4FA6_9611_45F2_1E95;

/// A perfect hash table of the keywords, each beside its little-endian
/// spelling, zero-padded. A free slot holds 0, which no name packs to.
static KEYWORDS: [(u64, TokenKind); 64] = {
    let mut table = [(0, TokenKind::Identifier); 64];
    let mut i = 0;
    while i < KEYWORD_KINDS.len() {
        let kind = KEYWORD_KINDS[i];
        let Some(spelling) = kind.static_text() else {
            panic!("a keyword has a fixed spelling");
        };
        let mut padded = [0; 8];
        let (head, _) = padded.split_at_mut(spelling.len());
        head.copy_from_slice(spelling.as_bytes());
        let packed = u64::from_le_bytes(padded);
        let slot = keyword_slot(packed);
        let entry = &mut table[slot];
        assert!(entry.0 == 0, "two keywords hash to one slot");
        *entry = (packed, kind);
        i += 1;
    }
    table
};

#[inline]
const fn keyword_slot(packed: u64) -> usize {
    (packed.wrapping_mul(KEYWORD_HASH_MULTIPLIER) >> 58) as usize
}

/// `bytes` packed as [`KEYWORDS`] packs a keyword, or `None` for a length no
/// keyword has.
///
/// Two overlapping loads cover any length from 2 to 8, and the bytes they
/// share are the same, so OR-ing them rebuilds the spelling.
#[inline]
fn pack_keyword_candidate(bytes: &[u8]) -> Option<u64> {
    let length = bytes.len();
    if length < 4 {
        let first = u64::from(u16::from_le_bytes(*bytes.first_chunk()?));
        let last = u64::from(u16::from_le_bytes(*bytes.last_chunk()?));
        Some(first | (last << ((length - 2) * 8)))
    } else if length <= 8 {
        let first = u64::from(u32::from_le_bytes(*bytes.first_chunk()?));
        let last = u64::from(u32::from_le_bytes(*bytes.last_chunk()?));
        Some(first | (last << ((length - 4) * 8)))
    } else {
        None
    }
}

/// 12 bytes and `Copy`. A token holds no text: [`Token::text`] slices it from
/// the source it was lexed from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Token {
    pub span: Span,
    pub kind: TokenKind,
    /// Whether a line break separates this token from the previous one, or
    /// from the start of the source, comments included.
    pub has_line_break_before: bool,
    /// Whether this string or interpolated string piece runs to the end of its
    /// line or of the source without its closing delimiter, or for `Eof`,
    /// whether a block comment ran to it without its closer. Only the last
    /// comment can.
    pub is_unterminated: bool,
    /// Whether this quoted string or interpolated string piece holds an
    /// escape sequence, or this long-bracket string a CR, which its value
    /// rewrites. Without one, its value is its text as written.
    pub has_escape: bool,
}

impl Token {
    /// The text this token carries in `source`, the source it was lexed from:
    /// for an interpolated string piece, what lies between its delimiters, and
    /// for any other token its whole spelling, a string's quotes or long
    /// brackets included. Escapes stay as written.
    #[inline]
    pub fn text(self, source: &[u8]) -> &[u8] {
        self.text_span().text(source)
    }

    /// Where [`Self::text`] lies: the whole token, or for an interpolated
    /// string piece, what lies between its delimiters.
    #[inline]
    pub const fn text_span(self) -> Span {
        if matches!(
            self.kind,
            TokenKind::InterpolatedStringBegin
                | TokenKind::InterpolatedStringMiddle
                | TokenKind::InterpolatedStringEnd
                | TokenKind::InterpolatedStringSimple
        ) {
            // Every delimiter is one byte, and only a closing one can be missing.
            let start = self.span.start + 1;
            let end = if self.is_unterminated { self.span.end } else { self.span.end - 1 };
            Span::new(start, if end > start { end } else { start })
        } else {
            self.span
        }
    }

    /// How an error message names this token: its kind, followed by its text
    /// for a name, a contextual keyword included, or a numeral.
    pub fn display(self, source: &[u8]) -> impl fmt::Display {
        TokenDisplay { kind: self.kind, text: self.text(source) }
    }
}

struct TokenDisplay<'source> {
    kind: TokenKind,
    text: &'source [u8],
}

impl fmt::Display for TokenDisplay<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // A name or a numeral is ASCII, which this writes as it is.
        if self.kind.is_name() {
            write!(f, "{} `{}`", TokenKind::Identifier, self.text.escape_ascii())
        } else if matches!(self.kind, TokenKind::Number | TokenKind::Integer) {
            write!(f, "{} `{}`", self.kind, self.text.escape_ascii())
        } else {
            self.kind.fmt(f)
        }
    }
}
