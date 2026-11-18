use luck_ast::{ContentEq as _, Expression, NameIndex, Statement, Synth};
use luck_token::BinOp;

use crate::common::Fixture;

/// `local x = a + 1`.
fn local_sum(synth: Synth<'_>) -> Statement<'_> {
    let sum = synth.binary_op(synth.name_expr("a"), BinOp::Add, synth.number("1"));
    synth.local(&["x"], [sum])
}

#[test]
fn trees_that_differ_only_in_spans_hold_the_same_content() {
    let (near, far) = (Fixture::default(), Fixture::starting_at(1000));
    let (near_stmt, far_stmt) = (local_sum(near.synth()), local_sum(far.synth()));
    assert_ne!(near_stmt, far_stmt);
    assert!(near_stmt.content_eq(&far_stmt));
}

#[test]
fn a_name_s_index_and_a_trailing_separator_do_not_count() {
    let fixture = Fixture::default();
    let synth = fixture.synth();
    let mut indexed = synth.name("x");
    indexed.index = Some(NameIndex::new(3));
    assert!(indexed.content_eq(&synth.name("x")));

    let Expression::TableConstructor(mut with_separator) = synth.array([synth.nil()]) else {
        panic!("expected a table");
    };
    with_separator.has_trailing_separator = true;
    let Expression::TableConstructor(without_separator) = synth.array([synth.nil()]) else {
        panic!("expected a table");
    };
    assert!(with_separator.content_eq(&without_separator));
}

#[test]
fn trees_that_differ_anywhere_else_differ() {
    let fixture = Fixture::default();
    let synth = fixture.synth();
    let pairs = [
        (
            synth.binary_op(synth.name_expr("a"), BinOp::Add, synth.number("1")),
            synth.binary_op(synth.name_expr("a"), BinOp::Sub, synth.number("1")),
        ),
        (synth.name_expr("a"), synth.name_expr("b")),
        (synth.nil(), synth.boolean(false)),
        (synth.number("1"), synth.number("1.0")),
        (
            synth.call(synth.name_expr("f"), []),
            synth.parenthesized(synth.call(synth.name_expr("f"), [])),
        ),
    ];
    for (lhs, rhs) in &pairs {
        assert!(!lhs.content_eq(rhs), "{lhs:?} and {rhs:?}");
    }
}
