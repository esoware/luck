use crate::TokenKind;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Associativity {
    Left,
    Right,
}

/// Which operand of a binary operator an expression fills.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OperandSide {
    Left,
    Right,
}

/// Binary operator as stored in the AST. The spelling is fixed per variant, so
/// a node holds this one-byte enum instead of a 40-byte `Token`.
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

    /// The operator a binary operator token denotes, or `None` for any other
    /// token.
    ///
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

    /// The exact source spelling.
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

/// Unary operator (`not`, `-`, `#`, `~`) as stored in the AST.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum UnOp {
    Not,
    Neg,
    Len,
    BitNot, // Lua 5.3+
}

impl UnOp {
    /// The level every unary operator binds at. Sits between multiplicative
    /// (10) and `^` (12), per the Lua reference manual.
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

    /// The exact source spelling.
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

/// Luau compound-assignment operator as stored in the AST.
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

    /// The exact source spelling.
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bin_op_from_token_kind_maps_every_binary_operator_token() {
        for (kind, op) in [
            (TokenKind::Or, BinOp::Or),
            (TokenKind::And, BinOp::And),
            (TokenKind::Less, BinOp::Lt),
            (TokenKind::Greater, BinOp::Gt),
            (TokenKind::LessEqual, BinOp::Le),
            (TokenKind::GreaterEqual, BinOp::Ge),
            (TokenKind::TildeEqual, BinOp::Ne),
            (TokenKind::EqualEqual, BinOp::Eq),
            (TokenKind::Pipe, BinOp::BitOr),
            (TokenKind::Tilde, BinOp::BitXor),
            (TokenKind::Ampersand, BinOp::BitAnd),
            (TokenKind::ShiftLeft, BinOp::Shl),
            (TokenKind::ShiftRight, BinOp::Shr),
            (TokenKind::DotDot, BinOp::Concat),
            (TokenKind::Plus, BinOp::Add),
            (TokenKind::Minus, BinOp::Sub),
            (TokenKind::Star, BinOp::Mul),
            (TokenKind::Slash, BinOp::Div),
            (TokenKind::Percent, BinOp::Mod),
            (TokenKind::FloorDiv, BinOp::FloorDiv),
            (TokenKind::Caret, BinOp::Pow),
        ] {
            assert_eq!(BinOp::from_token_kind(&kind), Some(op), "{kind:?}");
            assert_eq!(Some(op.static_text()), kind.static_text(), "{kind:?}");
        }
        assert_eq!(BinOp::from_token_kind(&TokenKind::Not), None);
        assert_eq!(BinOp::from_token_kind(&TokenKind::Equal), None);
    }

    #[test]
    fn bin_op_and_un_op_precedence_follow_the_lua_manual() {
        use Associativity::{Left, Right};
        for (ops, expected) in [
            (&[BinOp::Or][..], (1, Left)),
            (&[BinOp::And], (2, Left)),
            (&[BinOp::Lt, BinOp::Gt, BinOp::Le, BinOp::Ge, BinOp::Ne, BinOp::Eq], (3, Left)),
            (&[BinOp::BitOr], (4, Left)),
            (&[BinOp::BitXor], (5, Left)),
            (&[BinOp::BitAnd], (6, Left)),
            (&[BinOp::Shl, BinOp::Shr], (7, Left)),
            (&[BinOp::Concat], (8, Right)),
            (&[BinOp::Add, BinOp::Sub], (9, Left)),
            (&[BinOp::Mul, BinOp::Div, BinOp::Mod, BinOp::FloorDiv], (10, Left)),
            (&[BinOp::Pow], (12, Right)),
        ] {
            for op in ops {
                assert_eq!(op.precedence(), expected, "{op:?}");
            }
        }
        assert_eq!(UnOp::PRECEDENCE, 11);
    }

    #[test]
    fn bin_op_can_nest_bare_when_tighter_or_on_the_associative_side() {
        for side in [OperandSide::Left, OperandSide::Right] {
            assert!(BinOp::Mul.can_nest_bare(BinOp::Add, side), "{side:?}");
            assert!(!BinOp::Add.can_nest_bare(BinOp::Mul, side), "{side:?}");
        }
        // `a - b + c` is `(a - b) + c`, so only the left side nests bare.
        assert!(BinOp::Sub.can_nest_bare(BinOp::Add, OperandSide::Left));
        assert!(!BinOp::Sub.can_nest_bare(BinOp::Add, OperandSide::Right));
        // `..` and `^` group right.
        for op in [BinOp::Concat, BinOp::Pow] {
            assert!(op.can_nest_bare(op, OperandSide::Right), "{op:?}");
            assert!(!op.can_nest_bare(op, OperandSide::Left), "{op:?}");
        }
    }

    #[test]
    fn un_op_and_compound_op_spell_their_tokens() {
        for (kind, expected) in [
            (TokenKind::Not, "not"),
            (TokenKind::Minus, "-"),
            (TokenKind::Hash, "#"),
            (TokenKind::Tilde, "~"),
        ] {
            let op = UnOp::from_token_kind(&kind).expect("unary operator kind");
            assert_eq!(op.static_text(), expected);
            assert_eq!(kind.static_text(), Some(expected));
        }
        for (kind, expected) in [
            (TokenKind::PlusEqual, "+="),
            (TokenKind::MinusEqual, "-="),
            (TokenKind::StarEqual, "*="),
            (TokenKind::SlashEqual, "/="),
            (TokenKind::FloorDivEqual, "//="),
            (TokenKind::PercentEqual, "%="),
            (TokenKind::CaretEqual, "^="),
            (TokenKind::DotDotEqual, "..="),
        ] {
            let op = CompoundOp::from_token_kind(&kind).expect("compound operator kind");
            assert_eq!(op.static_text(), expected);
            assert_eq!(kind.static_text(), Some(expected));
        }
    }

    #[test]
    fn bin_op_is_comparison_holds_only_for_comparisons() {
        assert!(BinOp::Eq.is_comparison());
        assert!(BinOp::Lt.is_comparison());
        assert!(BinOp::Ge.is_comparison());
        assert!(!BinOp::Add.is_comparison());
        assert!(!BinOp::And.is_comparison());
        assert!(!BinOp::Concat.is_comparison());
    }
}
