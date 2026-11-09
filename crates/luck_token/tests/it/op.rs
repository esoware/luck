use luck_token::{Associativity, BinOp, CompoundOp, OperandSide, TokenKind, UnOp};

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
