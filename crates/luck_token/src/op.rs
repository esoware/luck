use crate::TokenKind;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Associativity {
    Left,
    Right,
}

/// The spelling is fixed per variant, so a node holds this one-byte enum instead of a 12-byte
/// `Token`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BinOp {
    Add,
    Sub,
    Mul,
    Div,
    FloorDiv,
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
}

impl BinOp {
    #[expect(
        clippy::wildcard_enum_match_arm,
        reason = "None is the right answer for every non-operator token, present or future"
    )]
    pub const fn from_token_kind(kind: TokenKind) -> Option<Self> {
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
            _ => return None,
        })
    }

    pub const fn static_text(self) -> &'static str {
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
        }
    }

    /// `(precedence, associativity)`, from the precedence table that the
    /// parser and the printers share, with [`UnOp::PRECEDENCE`] as its unary
    /// level.
    pub const fn precedence(self) -> (u8, Associativity) {
        match self {
            Self::Or => (1, Associativity::Left),
            Self::And => (2, Associativity::Left),
            Self::Lt | Self::Gt | Self::Le | Self::Ge | Self::Ne | Self::Eq => {
                (3, Associativity::Left)
            }
            Self::Concat => (4, Associativity::Right),
            Self::Add | Self::Sub => (5, Associativity::Left),
            Self::Mul | Self::Div | Self::Mod | Self::FloorDiv => (6, Associativity::Left),
            Self::Pow => (8, Associativity::Right),
        }
    }

    pub const fn is_comparison(self) -> bool {
        match self {
            Self::Eq | Self::Ne | Self::Lt | Self::Le | Self::Gt | Self::Ge => true,
            Self::Add
            | Self::Sub
            | Self::Mul
            | Self::Div
            | Self::FloorDiv
            | Self::Mod
            | Self::Pow
            | Self::Concat
            | Self::And
            | Self::Or => false,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum UnOp {
    Not,
    Neg,
    Len,
}

impl UnOp {
    /// Sits between multiplicative (6) and `^` (8), as Luau's `unaryPriority`
    /// does.
    pub const PRECEDENCE: u8 = 7;

    #[expect(
        clippy::wildcard_enum_match_arm,
        reason = "None is the right answer for every non-operator token, present or future"
    )]
    pub const fn from_token_kind(kind: TokenKind) -> Option<Self> {
        Some(match kind {
            TokenKind::Not => Self::Not,
            TokenKind::Minus => Self::Neg,
            TokenKind::Hash => Self::Len,
            _ => return None,
        })
    }

    pub const fn static_text(self) -> &'static str {
        match self {
            Self::Not => "not",
            Self::Neg => "-",
            Self::Len => "#",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CompoundOp {
    Add,
    Sub,
    Mul,
    Div,
    FloorDiv,
    Mod,
    Pow,
    Concat,
}

impl CompoundOp {
    #[expect(
        clippy::wildcard_enum_match_arm,
        reason = "None is the right answer for every non-operator token, present or future"
    )]
    pub const fn from_token_kind(kind: TokenKind) -> Option<Self> {
        Some(match kind {
            TokenKind::PlusEqual => Self::Add,
            TokenKind::MinusEqual => Self::Sub,
            TokenKind::StarEqual => Self::Mul,
            TokenKind::SlashEqual => Self::Div,
            TokenKind::FloorDivEqual => Self::FloorDiv,
            TokenKind::PercentEqual => Self::Mod,
            TokenKind::CaretEqual => Self::Pow,
            TokenKind::DotDotEqual => Self::Concat,
            _ => return None,
        })
    }

    pub const fn static_text(self) -> &'static str {
        match self {
            Self::Add => "+=",
            Self::Sub => "-=",
            Self::Mul => "*=",
            Self::Div => "/=",
            Self::FloorDiv => "//=",
            Self::Mod => "%=",
            Self::Pow => "^=",
            Self::Concat => "..=",
        }
    }

    pub const fn binary_op(self) -> BinOp {
        match self {
            Self::Add => BinOp::Add,
            Self::Sub => BinOp::Sub,
            Self::Mul => BinOp::Mul,
            Self::Div => BinOp::Div,
            Self::FloorDiv => BinOp::FloorDiv,
            Self::Mod => BinOp::Mod,
            Self::Pow => BinOp::Pow,
            Self::Concat => BinOp::Concat,
        }
    }
}
