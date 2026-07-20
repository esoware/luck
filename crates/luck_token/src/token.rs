use std::fmt;

use compact_str::CompactString;

use crate::Span;

/// Every distinct token the lexer can produce.
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
    InterpBegin(CompactString), // `text{
    InterpMid(CompactString),   // }text{
    InterpEnd(CompactString),   // }text`

    // Luau
    At,       // @
    Arrow,    // ->
    Question, // ?

    // Special
    Eof,
}

impl fmt::Display for TokenKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Identifier(name) => write!(f, "identifier '{name}'"),
            Self::Number(n) => write!(f, "number '{n}'"),
            Self::StringLiteral(_) => write!(f, "string"),
            Self::And => write!(f, "'and'"),
            Self::Break => write!(f, "'break'"),
            Self::Do => write!(f, "'do'"),
            Self::Else => write!(f, "'else'"),
            Self::ElseIf => write!(f, "'elseif'"),
            Self::End => write!(f, "'end'"),
            Self::False => write!(f, "'false'"),
            Self::For => write!(f, "'for'"),
            Self::Function => write!(f, "'function'"),
            Self::If => write!(f, "'if'"),
            Self::In => write!(f, "'in'"),
            Self::Local => write!(f, "'local'"),
            Self::Nil => write!(f, "'nil'"),
            Self::Not => write!(f, "'not'"),
            Self::Or => write!(f, "'or'"),
            Self::Repeat => write!(f, "'repeat'"),
            Self::Return => write!(f, "'return'"),
            Self::Then => write!(f, "'then'"),
            Self::True => write!(f, "'true'"),
            Self::Until => write!(f, "'until'"),
            Self::While => write!(f, "'while'"),
            Self::Goto => write!(f, "'goto'"),
            Self::Global => write!(f, "'global'"),
            Self::Plus => write!(f, "'+'"),
            Self::Minus => write!(f, "'-'"),
            Self::Star => write!(f, "'*'"),
            Self::Slash => write!(f, "'/'"),
            Self::FloorDiv => write!(f, "'//'"),
            Self::Percent => write!(f, "'%'"),
            Self::Caret => write!(f, "'^'"),
            Self::Hash => write!(f, "'#'"),
            Self::Ampersand => write!(f, "'&'"),
            Self::Tilde => write!(f, "'~'"),
            Self::Pipe => write!(f, "'|'"),
            Self::ShiftLeft => write!(f, "'<<'"),
            Self::ShiftRight => write!(f, "'>>'"),
            Self::Dot => write!(f, "'.'"),
            Self::DotDot => write!(f, "'..'"),
            Self::DotDotDot => write!(f, "'...'"),
            Self::Semicolon => write!(f, "';'"),
            Self::Colon => write!(f, "':'"),
            Self::DoubleColon => write!(f, "'::'"),
            Self::Comma => write!(f, "','"),
            Self::Equal => write!(f, "'='"),
            Self::EqualEqual => write!(f, "'=='"),
            Self::TildeEqual => write!(f, "'~='"),
            Self::Less => write!(f, "'<'"),
            Self::LessEqual => write!(f, "'<='"),
            Self::Greater => write!(f, "'>'"),
            Self::GreaterEqual => write!(f, "'>='"),
            Self::LeftParen => write!(f, "'('"),
            Self::RightParen => write!(f, "')'"),
            Self::LeftBrace => write!(f, "'{{'"),
            Self::RightBrace => write!(f, "'}}'"),
            Self::LeftBracket => write!(f, "'['"),
            Self::RightBracket => write!(f, "']'"),
            Self::PlusEqual => write!(f, "'+='"),
            Self::MinusEqual => write!(f, "'-='"),
            Self::StarEqual => write!(f, "'*='"),
            Self::SlashEqual => write!(f, "'/='"),
            Self::FloorDivEqual => write!(f, "'//='"),
            Self::PercentEqual => write!(f, "'%='"),
            Self::CaretEqual => write!(f, "'^='"),
            Self::DotDotEqual => write!(f, "'..='"),
            Self::InterpBegin(_) => write!(f, "interpolated string start"),
            Self::InterpMid(_) => write!(f, "interpolated string middle"),
            Self::InterpEnd(_) => write!(f, "interpolated string end"),
            Self::At => write!(f, "'@'"),
            Self::Arrow => write!(f, "'->'"),
            Self::Question => write!(f, "'?'"),
            Self::Eof => write!(f, "end of file"),
        }
    }
}

/// Binary operator associativity.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Assoc {
    Left,
    Right,
}

/// Precedence level of the unary operators (`not`, `#`, `-`, `~`). Sits
/// between multiplicative (10) and `^` (12), per the Lua reference manual.
pub const UNARY_PRECEDENCE: u8 = 11;

impl TokenKind {
    /// `(precedence, associativity)` for a binary operator token. The single
    /// precedence table shared by the parser, the minifier's paren
    /// simplifier, and the AST synthesizer; returns `None` for any token
    /// that is not a binary operator. Version-gated operators (bitwise,
    /// floor div) are included unconditionally; the lexer ensures they only
    /// appear in the token stream for supporting versions.
    #[must_use]
    pub fn binary_precedence(&self) -> Option<(u8, Assoc)> {
        match self {
            Self::Or => Some((1, Assoc::Left)),
            Self::And => Some((2, Assoc::Left)),
            Self::Less
            | Self::Greater
            | Self::LessEqual
            | Self::GreaterEqual
            | Self::TildeEqual
            | Self::EqualEqual => Some((3, Assoc::Left)),
            // Lua 5.3+ bitwise operators
            Self::Pipe => Some((4, Assoc::Left)),
            Self::Tilde => Some((5, Assoc::Left)),
            Self::Ampersand => Some((6, Assoc::Left)),
            Self::ShiftLeft | Self::ShiftRight => Some((7, Assoc::Left)),
            Self::DotDot => Some((8, Assoc::Right)),
            Self::Plus | Self::Minus => Some((9, Assoc::Left)),
            Self::Star | Self::Slash | Self::Percent | Self::FloorDiv => Some((10, Assoc::Left)),
            Self::Caret => Some((12, Assoc::Right)),
            _ => None,
        }
    }

    #[must_use]
    pub fn is_stat_start(&self) -> bool {
        matches!(
            self,
            Self::If
                | Self::While
                | Self::Do
                | Self::For
                | Self::Repeat
                | Self::Function
                | Self::Local
                | Self::Global
                | Self::Goto
                | Self::DoubleColon
                | Self::Semicolon
                | Self::Return
                | Self::Break
                | Self::Identifier(_)
                | Self::LeftParen
                | Self::At
        )
    }

    #[must_use]
    pub fn is_unary_op(&self) -> bool {
        matches!(self, Self::Minus | Self::Not | Self::Hash | Self::Tilde)
    }
}

/// Binary operator, stored in the AST instead of a full `Token`: the
/// spelling is fixed per variant, so nodes carry `(BinOp, Span)` in 8+8
/// bytes where a `Token` would cost 40.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BinOp {
    Add,
    Sub,
    Mul,
    Div,
    FloorDiv, // Lua 5.3+
    Mod,
    Pow,
    Concat,
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
    And,
    Or,
    BitAnd, // Lua 5.3+
    BitOr,  // Lua 5.3+
    BitXor, // Lua 5.3+
    Shl,    // Lua 5.3+
    Shr,    // Lua 5.3+
}

impl BinOp {
    /// Bitwise operators exist in 5.3+ only. The lexer already rejects
    /// them for 5.1/5.2, but Luau emits `&`/`|` tokens for its type
    /// grammar, so the parser must gate on this too.
    #[must_use]
    pub fn is_bitwise(self) -> bool {
        matches!(
            self,
            Self::BitAnd | Self::BitOr | Self::BitXor | Self::Shl | Self::Shr
        )
    }

    #[must_use]
    pub fn from_token_kind(kind: &TokenKind) -> Option<Self> {
        Some(match kind {
            TokenKind::Plus => Self::Add,
            TokenKind::Minus => Self::Sub,
            TokenKind::Star => Self::Mul,
            TokenKind::Slash => Self::Div,
            TokenKind::FloorDiv => Self::FloorDiv,
            TokenKind::Percent => Self::Mod,
            TokenKind::Caret => Self::Pow,
            TokenKind::DotDot => Self::Concat,
            TokenKind::EqualEqual => Self::Eq,
            TokenKind::TildeEqual => Self::Ne,
            TokenKind::Less => Self::Lt,
            TokenKind::LessEqual => Self::Le,
            TokenKind::Greater => Self::Gt,
            TokenKind::GreaterEqual => Self::Ge,
            TokenKind::And => Self::And,
            TokenKind::Or => Self::Or,
            TokenKind::Ampersand => Self::BitAnd,
            TokenKind::Pipe => Self::BitOr,
            TokenKind::Tilde => Self::BitXor,
            TokenKind::ShiftLeft => Self::Shl,
            TokenKind::ShiftRight => Self::Shr,
            _ => return None,
        })
    }

    /// The exact source spelling.
    #[must_use]
    pub fn static_text(self) -> &'static str {
        match self {
            Self::Add => "+",
            Self::Sub => "-",
            Self::Mul => "*",
            Self::Div => "/",
            Self::FloorDiv => "//",
            Self::Mod => "%",
            Self::Pow => "^",
            Self::Concat => "..",
            Self::Eq => "==",
            Self::Ne => "~=",
            Self::Lt => "<",
            Self::Le => "<=",
            Self::Gt => ">",
            Self::Ge => ">=",
            Self::And => "and",
            Self::Or => "or",
            Self::BitAnd => "&",
            Self::BitOr => "|",
            Self::BitXor => "~",
            Self::Shl => "<<",
            Self::Shr => ">>",
        }
    }

    /// `(precedence, associativity)`, same table as
    /// [`TokenKind::binary_precedence`].
    #[must_use]
    pub fn precedence(self) -> (u8, Assoc) {
        match self {
            Self::Or => (1, Assoc::Left),
            Self::And => (2, Assoc::Left),
            Self::Lt | Self::Gt | Self::Le | Self::Ge | Self::Ne | Self::Eq => (3, Assoc::Left),
            Self::BitOr => (4, Assoc::Left),
            Self::BitXor => (5, Assoc::Left),
            Self::BitAnd => (6, Assoc::Left),
            Self::Shl | Self::Shr => (7, Assoc::Left),
            Self::Concat => (8, Assoc::Right),
            Self::Add | Self::Sub => (9, Assoc::Left),
            Self::Mul | Self::Div | Self::Mod | Self::FloorDiv => (10, Assoc::Left),
            Self::Pow => (12, Assoc::Right),
        }
    }

    #[must_use]
    pub fn is_comparison(self) -> bool {
        matches!(
            self,
            Self::Eq | Self::Ne | Self::Lt | Self::Le | Self::Gt | Self::Ge
        )
    }
}

/// Unary operator (`not`, `-`, `#`, `~`), stored as `(UnOp, Span)` in the AST.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum UnOp {
    Not,
    Neg,
    Len,
    BitNot, // Lua 5.3+
}

impl UnOp {
    #[must_use]
    pub fn from_token_kind(kind: &TokenKind) -> Option<Self> {
        Some(match kind {
            TokenKind::Not => Self::Not,
            TokenKind::Minus => Self::Neg,
            TokenKind::Hash => Self::Len,
            TokenKind::Tilde => Self::BitNot,
            _ => return None,
        })
    }

    /// The exact source spelling.
    #[must_use]
    pub fn static_text(self) -> &'static str {
        match self {
            Self::Not => "not",
            Self::Neg => "-",
            Self::Len => "#",
            Self::BitNot => "~",
        }
    }
}

/// Luau compound-assignment operator, stored as `(CompoundOp, Span)`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CompoundOp {
    AddAssign,
    SubAssign,
    MulAssign,
    DivAssign,
    FloorDivAssign,
    ModAssign,
    PowAssign,
    ConcatAssign,
}

impl CompoundOp {
    #[must_use]
    pub fn from_token_kind(kind: &TokenKind) -> Option<Self> {
        Some(match kind {
            TokenKind::PlusEqual => Self::AddAssign,
            TokenKind::MinusEqual => Self::SubAssign,
            TokenKind::StarEqual => Self::MulAssign,
            TokenKind::SlashEqual => Self::DivAssign,
            TokenKind::FloorDivEqual => Self::FloorDivAssign,
            TokenKind::PercentEqual => Self::ModAssign,
            TokenKind::CaretEqual => Self::PowAssign,
            TokenKind::DotDotEqual => Self::ConcatAssign,
            _ => return None,
        })
    }

    /// The exact source spelling.
    #[must_use]
    pub fn static_text(self) -> &'static str {
        match self {
            Self::AddAssign => "+=",
            Self::SubAssign => "-=",
            Self::MulAssign => "*=",
            Self::DivAssign => "/=",
            Self::FloorDivAssign => "//=",
            Self::ModAssign => "%=",
            Self::PowAssign => "^=",
            Self::ConcatAssign => "..=",
        }
    }

    /// The underlying binary operation (`x += e` computes `x + e`).
    #[must_use]
    pub fn binop(self) -> BinOp {
        match self {
            Self::AddAssign => BinOp::Add,
            Self::SubAssign => BinOp::Sub,
            Self::MulAssign => BinOp::Mul,
            Self::DivAssign => BinOp::Div,
            Self::FloorDivAssign => BinOp::FloorDiv,
            Self::ModAssign => BinOp::Mod,
            Self::PowAssign => BinOp::Pow,
            Self::ConcatAssign => BinOp::Concat,
        }
    }
}

/// A single token with its position in source.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Token {
    pub kind: TokenKind,
    pub span: Span,
}

impl Token {
    pub fn new(kind: TokenKind, span: Span) -> Self {
        Self { kind, span }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Tokens sit in a large flat buffer; a size regression here multiplies
    // across every file. CompactString payload (24) + tag padding fixes
    // TokenKind at 32, + Span (8) = 40.
    #[test]
    #[cfg(target_pointer_width = "64")]
    fn token_size_is_pinned() {
        assert_eq!(std::mem::size_of::<TokenKind>(), 32);
        assert_eq!(std::mem::size_of::<Token>(), 40);
        assert_eq!(std::mem::size_of::<Option<Token>>(), 40);
    }

    #[test]
    fn token_new_stores_kind_and_span() {
        let span = Span::new(3, 7);
        let token = Token::new(TokenKind::Nil, span);
        assert_eq!(token.kind, TokenKind::Nil);
        assert_eq!(token.span, span);
    }

    #[test]
    fn compact_string_variants_store_text() {
        let ident = TokenKind::Identifier(CompactString::from("foo"));
        assert_eq!(ident, TokenKind::Identifier(CompactString::from("foo")));
        assert_ne!(ident, TokenKind::Identifier(CompactString::from("bar")));
        assert_eq!(
            TokenKind::Number(CompactString::from("1.5")),
            TokenKind::Number(CompactString::from("1.5"))
        );
    }

    #[test]
    fn display_carries_payload_for_named_kinds() {
        assert_eq!(
            TokenKind::Identifier(CompactString::from("foo")).to_string(),
            "identifier 'foo'"
        );
        assert_eq!(
            TokenKind::Number(CompactString::from("42")).to_string(),
            "number '42'"
        );
        // The string payload is deliberately not echoed.
        assert_eq!(
            TokenKind::StringLiteral(CompactString::from("secret")).to_string(),
            "string"
        );
    }

    #[test]
    fn display_quotes_symbols_and_keywords() {
        assert_eq!(TokenKind::And.to_string(), "'and'");
        assert_eq!(TokenKind::DotDotDot.to_string(), "'...'");
        assert_eq!(TokenKind::LeftBrace.to_string(), "'{'");
        assert_eq!(TokenKind::RightBrace.to_string(), "'}'");
        assert_eq!(TokenKind::Eof.to_string(), "end of file");
    }

    #[test]
    fn is_stat_start_classifies() {
        assert!(TokenKind::If.is_stat_start());
        assert!(TokenKind::Local.is_stat_start());
        assert!(TokenKind::Identifier(CompactString::from("x")).is_stat_start());
        assert!(TokenKind::LeftParen.is_stat_start());
        assert!(TokenKind::At.is_stat_start());
        assert!(!TokenKind::Plus.is_stat_start());
        assert!(!TokenKind::End.is_stat_start());
        assert!(!TokenKind::Eof.is_stat_start());
    }

    #[test]
    fn binop_matches_token_kind_precedence() {
        let kinds = [
            TokenKind::Or,
            TokenKind::And,
            TokenKind::Less,
            TokenKind::Greater,
            TokenKind::LessEqual,
            TokenKind::GreaterEqual,
            TokenKind::TildeEqual,
            TokenKind::EqualEqual,
            TokenKind::Pipe,
            TokenKind::Tilde,
            TokenKind::Ampersand,
            TokenKind::ShiftLeft,
            TokenKind::ShiftRight,
            TokenKind::DotDot,
            TokenKind::Plus,
            TokenKind::Minus,
            TokenKind::Star,
            TokenKind::Slash,
            TokenKind::Percent,
            TokenKind::FloorDiv,
            TokenKind::Caret,
        ];
        for kind in kinds {
            let op = BinOp::from_token_kind(&kind).expect("binary operator kind");
            assert_eq!(Some(op.precedence()), kind.binary_precedence(), "{kind:?}");
        }
        assert_eq!(BinOp::from_token_kind(&TokenKind::Not), None);
        assert_eq!(BinOp::from_token_kind(&TokenKind::Equal), None);
    }

    #[test]
    fn unop_and_compound_round_trip() {
        for (kind, expected) in [
            (TokenKind::Not, "not"),
            (TokenKind::Minus, "-"),
            (TokenKind::Hash, "#"),
            (TokenKind::Tilde, "~"),
        ] {
            let op = UnOp::from_token_kind(&kind).expect("unary operator kind");
            assert_eq!(op.static_text(), expected);
        }
        for (kind, expected, folded) in [
            (TokenKind::PlusEqual, "+=", BinOp::Add),
            (TokenKind::MinusEqual, "-=", BinOp::Sub),
            (TokenKind::StarEqual, "*=", BinOp::Mul),
            (TokenKind::SlashEqual, "/=", BinOp::Div),
            (TokenKind::FloorDivEqual, "//=", BinOp::FloorDiv),
            (TokenKind::PercentEqual, "%=", BinOp::Mod),
            (TokenKind::CaretEqual, "^=", BinOp::Pow),
            (TokenKind::DotDotEqual, "..=", BinOp::Concat),
        ] {
            let op = CompoundOp::from_token_kind(&kind).expect("compound operator kind");
            assert_eq!(op.static_text(), expected);
            assert_eq!(op.binop(), folded);
        }
    }

    #[test]
    fn binop_comparison_classification() {
        assert!(BinOp::Eq.is_comparison());
        assert!(BinOp::Lt.is_comparison());
        assert!(BinOp::Ge.is_comparison());
        assert!(!BinOp::Add.is_comparison());
        assert!(!BinOp::And.is_comparison());
        assert!(!BinOp::Concat.is_comparison());
    }

    #[test]
    fn is_unary_op_classifies() {
        assert!(TokenKind::Minus.is_unary_op());
        assert!(TokenKind::Not.is_unary_op());
        assert!(TokenKind::Hash.is_unary_op());
        assert!(TokenKind::Tilde.is_unary_op());
        assert!(!TokenKind::Plus.is_unary_op());
        assert!(!TokenKind::Star.is_unary_op());
    }
}
