//! The printer recurses once per level, so a tree at the cap, or whatever a
//! parse past it or of broken input leaves, has to print on
//! `luck_ast::MIN_STACK_BYTES`.

use luck_allocator::Allocator;
use luck_ast::{Chunk, Expression, MAX_NESTING_DEPTH, SpanCounter, Synth, block_nesting_depth};
use luck_codegen::LiteralSpelling;
use luck_testgen::{
    broken_sources, deep_and_long_sources, on_the_smallest_stack, sources_past_the_nesting_cap,
    with_the_smallest_stack,
};
use luck_token::{BinOp, UnOp};

#[test]
fn compacting_fits_the_smallest_stack() {
    on_the_smallest_stack(deep_and_long_sources(), |source| {
        let allocator = Allocator::new();
        let parsed = luck_parser::parse(&allocator, source.as_bytes());
        assert!(parsed.errors.is_empty(), "{:?}", parsed.errors.first());
        for spelling in [LiteralSpelling::AsWritten, LiteralSpelling::Shortest] {
            let output = luck_codegen::compact(&parsed.chunk, spelling);
            let reparsed = luck_parser::parse(&allocator, &output);
            assert!(reparsed.errors.is_empty(), "the output reparses");
        }
    });
}

#[test]
fn compacting_what_a_failed_parse_leaves_fits_the_smallest_stack() {
    let sources = sources_past_the_nesting_cap().into_iter().chain(broken_sources());
    on_the_smallest_stack(sources, |source| {
        let allocator = Allocator::new();
        let parsed = luck_parser::parse(&allocator, source.as_bytes());
        assert!(!parsed.errors.is_empty(), "{:?}", source.get(..20));
        for spelling in [LiteralSpelling::AsWritten, LiteralSpelling::Shortest] {
            luck_codegen::compact(&parsed.chunk, spelling);
        }
    });
}

/// Nests its operand exactly one level deeper.
type Wrap = for<'ast> fn(Synth<'ast>, Expression<'ast>) -> Expression<'ast>;

/// `return x`, with `wrap` applied to `x` until the block nests `depth` deep.
fn nested_return(synth: Synth<'_>, depth: usize, wrap: Wrap) -> Chunk<'_> {
    let mut expr = synth.name_expr("x");
    for _ in 1..depth {
        expr = wrap(synth, expr);
    }
    let block = synth.block([synth.return_([expr])]);
    assert_eq!(block_nesting_depth(&block), depth, "each wrap adds one level");
    Chunk::new(block)
}

#[test]
fn compacting_a_built_tree_at_the_nesting_cap_fits_the_smallest_stack() {
    with_the_smallest_stack(|| {
        let wraps: [(&str, Wrap); 6] = [
            ("grouped operand", |synth, expr| {
                synth.binary_op(synth.number_i64(1), BinOp::Sub, expr)
            }),
            ("unary operator", |synth, expr| synth.unary_op(UnOp::Not, expr)),
            ("call argument", |synth, expr| synth.call(synth.name_expr("f"), [expr])),
            ("method argument", |synth, expr| synth.method_call(synth.name_expr("o"), "m", [expr])),
            ("index", |synth, expr| synth.index(synth.name_expr("t"), expr)),
            ("table", |synth, expr| synth.array([expr])),
        ];
        for (label, wrap) in wraps {
            let allocator = Allocator::new();
            let spans = SpanCounter::new();
            let chunk = nested_return(Synth::new(&allocator, &spans), MAX_NESTING_DEPTH, wrap);
            let output = luck_codegen::compact(&chunk, LiteralSpelling::AsWritten);
            let reparsed = luck_parser::parse(&allocator, &output);
            assert!(reparsed.errors.is_empty(), "{label}: {:?}", reparsed.errors.first());
            assert_eq!(block_nesting_depth(&reparsed.chunk.block), MAX_NESTING_DEPTH, "{label}");
        }
    });
}
