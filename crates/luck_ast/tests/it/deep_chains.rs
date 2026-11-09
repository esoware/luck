use core::mem::ManuallyDrop;

use luck_ast::{Expression, Statement, Synth, Transform, Visitor, stmt_starts_with_paren};
use luck_token::BinOp;

use crate::common::assert_runs_without_stack_overflow;

/// `x = 1+1+...` of this length loads fine on stock Lua 5.1 and 5.4, because
/// the left spine is built in a loop.
const TERMS: usize = 200_000;

/// `a + 1 + 1 + ...`, which `Synth` builds iteratively as a left-deep tree,
/// the same shape the parser builds for a long left-associative chain.
fn long_left_associative_sum(synth: &Synth) -> Expression {
    let mut chain = synth.name_expr("a");
    for _ in 0..TERMS {
        chain = synth.binary_op(chain, BinOp::Add, synth.number("1"));
    }
    chain
}

/// `f()()()...`, a long left-deep chain of call suffixes.
fn long_call_chain_statement(synth: &Synth) -> Statement {
    let mut chain = synth.call(synth.name_expr("f"), vec![]);
    for _ in 0..TERMS {
        chain = synth.call(chain, vec![]);
    }
    synth.call_stmt(chain)
}

#[test]
fn dropping_a_long_left_associative_chain_does_not_overflow() {
    assert_runs_without_stack_overflow(
        "deep_chains::dropping_a_long_left_associative_chain_does_not_overflow",
        || drop(long_left_associative_sum(&Synth::new())),
    );
}

#[test]
fn visitor_walks_a_long_left_associative_chain_without_overflow() {
    struct DefaultWalk;
    impl Visitor<'_> for DefaultWalk {}

    assert_runs_without_stack_overflow(
        "deep_chains::visitor_walks_a_long_left_associative_chain_without_overflow",
        || {
            // ManuallyDrop keeps a Drop overflow from masking the result.
            let chain = ManuallyDrop::new(long_left_associative_sum(&Synth::new()));
            DefaultWalk.visit_expr(&chain);
        },
    );
}

#[test]
fn transform_walks_a_long_left_associative_chain_without_overflow() {
    struct Identity;
    impl Transform for Identity {}

    assert_runs_without_stack_overflow(
        "deep_chains::transform_walks_a_long_left_associative_chain_without_overflow",
        || {
            let chain = long_left_associative_sum(&Synth::new());
            let rewritten = ManuallyDrop::new(Identity.transform_expr(chain));
            assert!(matches!(*rewritten, Expression::BinaryOp(_)));
        },
    );
}

#[test]
fn stmt_starts_with_paren_handles_a_long_call_chain_without_overflow() {
    assert_runs_without_stack_overflow(
        "deep_chains::stmt_starts_with_paren_handles_a_long_call_chain_without_overflow",
        || {
            let stmt = ManuallyDrop::new(long_call_chain_statement(&Synth::new()));
            assert!(!stmt_starts_with_paren(&stmt), "`f()()...` starts with an identifier");
        },
    );
}
