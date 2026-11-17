use core::fmt;

use crate::{LuaVersion, Span};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TokenKind {
    Identifier,
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
    #[expect(
        clippy::wildcard_enum_match_arm,
        reason = "every other kind has a fixed spelling, which static_text matches exhaustively"
    )]
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Identifier => "identifier",
            Self::Number => "number",
            Self::Integer => "integer",
            Self::StringLiteral => "string",
            Self::InterpolatedStringBegin => "interpolated string start",
            Self::InterpolatedStringMiddle => "interpolated string middle",
            Self::InterpolatedStringEnd => "interpolated string end",
            Self::InterpolatedStringSimple => "interpolated string",
            Self::Eof => "end of file",
            fixed => return write!(f, "`{}`", fixed.static_text().unwrap_or_default()),
        })
    }
}

impl TokenKind {
    /// The exact source spelling, or `None` for a kind whose text varies and
    /// for `Eof`, which has none.
    #[inline]
    #[must_use]
    pub const fn static_text(self) -> Option<&'static str> {
        Some(match self {
            Self::Identifier
            | Self::Number
            | Self::Integer
            | Self::StringLiteral
            | Self::InterpolatedStringBegin
            | Self::InterpolatedStringMiddle
            | Self::InterpolatedStringEnd
            | Self::InterpolatedStringSimple
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
    #[inline]
    #[must_use]
    pub fn keyword(text: &str, version: LuaVersion) -> Option<Self> {
        let packed = pack_keyword_candidate(text.as_bytes())?;
        let &(keyword, kind) = KEYWORDS.get(keyword_slot(packed))?;
        (keyword == packed && (kind != Self::Goto || version.has_goto())).then_some(kind)
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

/// Every keyword is 2 to 8 bytes, so its spelling packs into a `u64`.
const KEYWORD_KINDS: [TokenKind; 22] = [
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
];

/// Gives each keyword its own slot. Building [`KEYWORDS`] fails to compile on
/// a collision, so a new keyword may need a new multiplier.
const KEYWORD_HASH_MULTIPLIER: u64 = 0x2A3D_9F0D_57A9_2BDF;

/// A perfect hash table of the keywords, each beside its little-endian
/// spelling, zero-padded. A free slot holds 0, which no name packs to.
static KEYWORDS: [(u64, TokenKind); 32] = {
    let mut table = [(0, TokenKind::Identifier); 32];
    let mut i = 0;
    while i < KEYWORD_KINDS.len() {
        #[expect(clippy::indexing_slicing, reason = "const evaluation checks every index")]
        let kind = KEYWORD_KINDS[i];
        let Some(spelling) = kind.static_text() else {
            panic!("a keyword has a fixed spelling");
        };
        let mut padded = [0; 8];
        let (head, _) = padded.split_at_mut(spelling.len());
        head.copy_from_slice(spelling.as_bytes());
        #[expect(clippy::little_endian_bytes, reason = "the packing is little-endian by design")]
        let packed = u64::from_le_bytes(padded);
        let slot = keyword_slot(packed);
        #[expect(clippy::indexing_slicing, reason = "const evaluation checks every index")]
        let entry = &mut table[slot];
        assert!(entry.0 == 0, "two keywords hash to one slot");
        *entry = (packed, kind);
        i += 1;
    }
    table
};

#[inline]
#[expect(clippy::as_conversions, reason = "the shift leaves 5 bits, which fit any usize")]
const fn keyword_slot(packed: u64) -> usize {
    (packed.wrapping_mul(KEYWORD_HASH_MULTIPLIER) >> 59) as usize
}

/// `bytes` packed as [`KEYWORDS`] packs a keyword, or `None` for a length no
/// keyword has.
///
/// Two overlapping loads cover any length from 2 to 8, and the bytes they
/// share are the same, so OR-ing them rebuilds the spelling.
#[inline]
#[expect(clippy::little_endian_bytes, reason = "the packing is little-endian by design")]
fn pack_keyword_candidate(bytes: &[u8]) -> Option<u64> {
    let overlap_shift = |length: usize, load_width: usize| {
        u32::try_from(length.wrapping_sub(load_width).wrapping_mul(8)).unwrap_or_default()
    };
    let length = bytes.len();
    if length < 4 {
        let first = u64::from(u16::from_le_bytes(*bytes.first_chunk()?));
        let last = u64::from(u16::from_le_bytes(*bytes.last_chunk()?));
        Some(first | last.wrapping_shl(overlap_shift(length, 2)))
    } else if length <= 8 {
        let first = u64::from(u32::from_le_bytes(*bytes.first_chunk()?));
        let last = u64::from(u32::from_le_bytes(*bytes.last_chunk()?));
        Some(first | last.wrapping_shl(overlap_shift(length, 4)))
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
    /// line or of the source without its closing delimiter.
    pub is_unterminated: bool,
}

impl Token {
    /// The text this token carries in `source`, the source it was lexed from:
    /// for an interpolated string piece, what lies between its delimiters, and
    /// for any other token its whole spelling, a string's quotes or long
    /// brackets included. Escapes stay as written.
    #[inline]
    #[must_use]
    pub fn text(self, source: &str) -> &str {
        let span = if matches!(
            self.kind,
            TokenKind::InterpolatedStringBegin
                | TokenKind::InterpolatedStringMiddle
                | TokenKind::InterpolatedStringEnd
                | TokenKind::InterpolatedStringSimple
        ) {
            // Every delimiter is one byte, and only a closing one can be missing.
            let start = self.span.start.saturating_add(1);
            let end =
                if self.is_unterminated { self.span.end } else { self.span.end.saturating_sub(1) };
            Span::new(start, end.max(start))
        } else {
            self.span
        };
        span.text(source)
    }

    /// How an error message names this token: its kind, followed by its text
    /// for a name or a numeral.
    #[must_use]
    pub fn display(self, source: &str) -> impl fmt::Display {
        TokenDisplay { kind: self.kind, text: self.text(source) }
    }
}

struct TokenDisplay<'source> {
    kind: TokenKind,
    text: &'source str,
}

impl fmt::Display for TokenDisplay<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if matches!(self.kind, TokenKind::Identifier | TokenKind::Number | TokenKind::Integer) {
            write!(f, "{} `{}`", self.kind, self.text)
        } else {
            self.kind.fmt(f)
        }
    }
}
