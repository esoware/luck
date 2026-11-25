use luck_token::{Associativity, BinOp, CompoundOp, TokenKind, UnOp};

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
        (TokenKind::DotDot, BinOp::Concat),
        (TokenKind::Plus, BinOp::Add),
        (TokenKind::Minus, BinOp::Sub),
        (TokenKind::Star, BinOp::Mul),
        (TokenKind::Slash, BinOp::Div),
        (TokenKind::Percent, BinOp::Mod),
        (TokenKind::FloorDiv, BinOp::FloorDiv),
        (TokenKind::Caret, BinOp::Pow),
    ] {
        assert_eq!(BinOp::from_token_kind(kind), Some(op), "{kind:?}");
        assert_eq!(Some(op.static_text()), kind.static_text(), "{kind:?}");
    }
    for kind in [TokenKind::Not, TokenKind::Equal, TokenKind::Ampersand, TokenKind::Pipe] {
        assert_eq!(BinOp::from_token_kind(kind), None, "{kind:?}");
    }
}

#[test]
fn bin_op_and_un_op_precedence_follow_luau() {
    use Associativity::{Left, Right};
    for (ops, expected) in [
        (&[BinOp::Or][..], (1, Left)),
        (&[BinOp::And], (2, Left)),
        (&[BinOp::Lt, BinOp::Gt, BinOp::Le, BinOp::Ge, BinOp::Ne, BinOp::Eq], (3, Left)),
        (&[BinOp::Concat], (4, Right)),
        (&[BinOp::Add, BinOp::Sub], (5, Left)),
        (&[BinOp::Mul, BinOp::Div, BinOp::Mod, BinOp::FloorDiv], (6, Left)),
        (&[BinOp::Pow], (8, Right)),
    ] {
        for op in ops {
            assert_eq!(op.precedence(), expected, "{op:?}");
        }
    }
    assert_eq!(UnOp::PRECEDENCE, 7);
}

#[test]
fn un_op_and_compound_op_spell_their_tokens() {
    for (kind, expected) in
        [(TokenKind::Not, "not"), (TokenKind::Minus, "-"), (TokenKind::Hash, "#")]
    {
        let op = UnOp::from_token_kind(kind).expect("unary operator kind");
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
        let op = CompoundOp::from_token_kind(kind).expect("compound operator kind");
        assert_eq!(op.static_text(), expected);
        assert_eq!(kind.static_text(), Some(expected));
    }
}

#[test]
fn compound_op_applies_the_binary_operator_it_spells() {
    for op in [
        CompoundOp::Add,
        CompoundOp::Sub,
        CompoundOp::Mul,
        CompoundOp::Div,
        CompoundOp::FloorDiv,
        CompoundOp::Mod,
        CompoundOp::Pow,
        CompoundOp::Concat,
    ] {
        let spelled = op.static_text().strip_suffix('=').expect("a compound operator ends in `=`");
        assert_eq!(op.binary_op().static_text(), spelled, "{op:?}");
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
