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

impl TokenKind {
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

    pub fn is_unary_op(&self) -> bool {
        matches!(self, Self::Minus | Self::Not | Self::Hash | Self::Tilde)
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
    fn is_unary_op_classifies() {
        assert!(TokenKind::Minus.is_unary_op());
        assert!(TokenKind::Not.is_unary_op());
        assert!(TokenKind::Hash.is_unary_op());
        assert!(TokenKind::Tilde.is_unary_op());
        assert!(!TokenKind::Plus.is_unary_op());
        assert!(!TokenKind::Star.is_unary_op());
    }
}
