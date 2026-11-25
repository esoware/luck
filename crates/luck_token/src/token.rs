use std::fmt;

use crate::Span;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TokenKind {
    Identifier,

    // Contextual keywords, each a name wherever its keyword reading doesn't
    // apply. They follow `Identifier`, so `is_name` is one comparison.
    Continue,
    Export,
    Const,
    Type,
    Typeof,
    Read,
    Write,

    Number,
    /// A numeral with the `i` suffix, which reads as an exact 64-bit integer.
    Integer,
    StringLiteral,

    // Interpolated string pieces, each spanning its delimiters
    InterpolatedStringBegin,  // `text{
    InterpolatedStringMiddle, // }text{
    InterpolatedStringEnd,    // }text`
    InterpolatedStringSimple, // `text`, which holds no expression

    // Keywords
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

    Plus,
    Minus,
    Star,
    Slash,
    FloorDiv,
    Percent,
    Caret,
    Hash,
    Ampersand,
    Pipe,
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

    // Compound assignment operators
    PlusEqual,
    MinusEqual,
    StarEqual,
    SlashEqual,
    FloorDivEqual,
    PercentEqual,
    CaretEqual,
    DotDotEqual,

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
            Self::Continue => "continue",
            Self::Export => "export",
            Self::Const => "const",
            Self::Type => "type",
            Self::Typeof => "typeof",
            Self::Read => "read",
            Self::Write => "write",
            Self::Plus => "+",
            Self::Minus => "-",
            Self::Star => "*",
            Self::Slash => "/",
            Self::FloorDiv => "//",
            Self::Percent => "%",
            Self::Caret => "^",
            Self::Hash => "#",
            Self::Ampersand => "&",
            Self::Pipe => "|",
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
            Self::PlusEqual => "+=",
            Self::MinusEqual => "-=",
            Self::StarEqual => "*=",
            Self::SlashEqual => "/=",
            Self::FloorDivEqual => "//=",
            Self::PercentEqual => "%=",
            Self::CaretEqual => "^=",
            Self::DotDotEqual => "..=",
            Self::At => "@",
            Self::Arrow => "->",
            Self::Question => "?",
        })
    }

    /// The keyword or contextual keyword `text` spells, as the lexer reads
    /// it, or `None` for any other name.
    #[inline]
    pub fn keyword(text: impl AsRef<[u8]>) -> Option<Self> {
        let packed = pack_keyword_candidate(text.as_ref())?;
        let (keyword, kind) = KEYWORDS[keyword_slot(packed)];
        (keyword == packed).then_some(kind)
    }

    /// The kind of the name of `length` bytes, at least one, that opens
    /// `first_eight`, eight bytes in little-endian order that run past a
    /// shorter name: as [`Self::keyword`] reads it, or [`Self::Identifier`].
    ///
    /// It takes no branch on the length, for a lexer that reads many names
    /// at once.
    #[inline]
    pub fn of_name(first_eight: u64, length: usize) -> Self {
        let spelled = u64::MAX >> (64 - 8 * length.clamp(1, 8));
        let packed = first_eight & spelled;
        let (keyword, kind) = KEYWORDS[keyword_slot(packed)];
        if keyword == packed && length <= 8 { kind } else { Self::Identifier }
    }

    /// Whether code luck writes can't name a variable `text`: a keyword, but
    /// no contextual keyword, which is a name wherever a name goes.
    pub fn is_reserved(text: &str) -> bool {
        Self::keyword(text).is_some_and(|kind| !kind.is_name())
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
            | Self::Write => true,
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
            | Self::Plus
            | Self::Minus
            | Self::Star
            | Self::Slash
            | Self::FloorDiv
            | Self::Percent
            | Self::Caret
            | Self::Hash
            | Self::Ampersand
            | Self::Pipe
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
const KEYWORD_KINDS: [TokenKind; 28] = [
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
    TokenKind::Continue,
    TokenKind::Export,
    TokenKind::Const,
    TokenKind::Type,
    TokenKind::Typeof,
    TokenKind::Read,
    TokenKind::Write,
];

/// Gives each keyword its own slot. Building [`KEYWORDS`] fails to compile on
/// a collision, so a new keyword may need a new multiplier.
const KEYWORD_HASH_MULTIPLIER: u64 = 0xE043_5658_D461_2E0B;

/// A perfect hash table of the keywords, each beside its little-endian
/// spelling, zero-padded. A free slot holds 0, which no name packs to.
static KEYWORDS: [(u64, TokenKind); 32] = {
    let mut table = [(0, TokenKind::Identifier); 32];
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
    (packed.wrapping_mul(KEYWORD_HASH_MULTIPLIER) >> 59) as usize
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
