use crate::TokenKind;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Associativity {
    Left,
    Right,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OperandSide {
    Left,
    Right,
}

/// The spelling is fixed per variant, so a node holds this one-byte enum instead of a 40-byte
/// `Token`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BinOp {
    Add,
    Sub,
    Mul,
    Div,
    FloorDiv, // Lua 5.3+, Luau
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
    /// Bitwise operators exist only in Lua 5.3+. Luau still lexes `&` and `|`
    /// for its type grammar, so the parser checks this as well.
    #[must_use]
    pub const fn is_bitwise(self) -> bool {
        match self {
            Self::BitAnd | Self::BitOr | Self::BitXor | Self::Shl | Self::Shr => true,
            Self::Add
            | Self::Sub
            | Self::Mul
            | Self::Div
            | Self::FloorDiv
            | Self::Mod
            | Self::Pow
            | Self::Concat
            | Self::Eq
            | Self::Ne
            | Self::Lt
            | Self::Le
            | Self::Gt
            | Self::Ge
            | Self::And
            | Self::Or => false,
        }
    }

    /// Version-gated operators (bitwise, floor div) map unconditionally. The
    /// lexer only produces them for versions that have them, except Luau's
    /// `&` and `|`, which belong to its type syntax (see [`Self::is_bitwise`]).
    #[must_use]
    #[expect(
        clippy::wildcard_enum_match_arm,
        reason = "None is the right answer for every non-operator token, present or future"
    )]
    pub const fn from_token_kind(kind: &TokenKind) -> Option<Self> {
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

    #[must_use]
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
            Self::BitAnd => "&",
            Self::BitOr => "|",
            Self::BitXor => "~",
            Self::Shl => "<<",
            Self::Shr => ">>",
        }
    }

    /// `(precedence, associativity)`, from the precedence table that the
    /// parser, the minifier, and the AST synthesizer all share, with
    /// [`UnOp::PRECEDENCE`] as its unary level.
    #[must_use]
    pub const fn precedence(self) -> (u8, Associativity) {
        match self {
            Self::Or => (1, Associativity::Left),
            Self::And => (2, Associativity::Left),
            Self::Lt | Self::Gt | Self::Le | Self::Ge | Self::Ne | Self::Eq => {
                (3, Associativity::Left)
            }
            Self::BitOr => (4, Associativity::Left),
            Self::BitXor => (5, Associativity::Left),
            Self::BitAnd => (6, Associativity::Left),
            Self::Shl | Self::Shr => (7, Associativity::Left),
            Self::Concat => (8, Associativity::Right),
            Self::Add | Self::Sub => (9, Associativity::Left),
            Self::Mul | Self::Div | Self::Mod | Self::FloorDiv => (10, Associativity::Left),
            Self::Pow => (12, Associativity::Right),
        }
    }

    /// Whether a binary expression on this operator, printed without
    /// parentheses as `outer`'s `side` operand, parses back as that operand:
    /// it binds tighter than `outer`, or as tight with `outer` associating
    /// toward `side`.
    #[must_use]
    pub const fn can_nest_bare(self, outer: Self, side: OperandSide) -> bool {
        let (precedence, _) = self.precedence();
        let (outer_precedence, outer_associativity) = outer.precedence();
        precedence > outer_precedence
            || (precedence == outer_precedence
                && matches!(
                    (side, outer_associativity),
                    (OperandSide::Left, Associativity::Left)
                        | (OperandSide::Right, Associativity::Right)
                ))
    }

    #[must_use]
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
            | Self::Or
            | Self::BitAnd
            | Self::BitOr
            | Self::BitXor
            | Self::Shl
            | Self::Shr => false,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum UnOp {
    Not,
    Neg,
    Len,
    BitNot, // Lua 5.3+
}

impl UnOp {
    /// Sits between multiplicative (10) and `^` (12), per the Lua reference manual.
    pub const PRECEDENCE: u8 = 11;

    #[must_use]
    #[expect(
        clippy::wildcard_enum_match_arm,
        reason = "None is the right answer for every non-operator token, present or future"
    )]
    pub const fn from_token_kind(kind: &TokenKind) -> Option<Self> {
        Some(match kind {
            TokenKind::Not => Self::Not,
            TokenKind::Minus => Self::Neg,
            TokenKind::Hash => Self::Len,
            TokenKind::Tilde => Self::BitNot,
            _ => return None,
        })
    }

    #[must_use]
    pub const fn static_text(self) -> &'static str {
        match self {
            Self::Not => "not",
            Self::Neg => "-",
            Self::Len => "#",
            Self::BitNot => "~",
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
    #[must_use]
    #[expect(
        clippy::wildcard_enum_match_arm,
        reason = "None is the right answer for every non-operator token, present or future"
    )]
    pub const fn from_token_kind(kind: &TokenKind) -> Option<Self> {
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

    #[must_use]
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
}
