use core::mem;

use luck_token::Span;

use crate::expr::Expression;
use crate::generated::Visit;
use crate::span::GetSpan as _;
use crate::stmt::{Block, Statement};
use crate::types::Type;

/// The deepest tree `luck_parser` builds, in every dialect, in the levels
/// [`Visit::descend`] counts.
///
/// Luau's parser counts a level for each statement in a block and each
/// expression, and stops past 1,000, and Lua's stops at 200. Nesting a
/// construct on itself takes at most two levels here for each of Luau's, and
/// the deepest, 1,000 nested function declarations, takes 2,001: the chunk's
/// block, then a body and a block for each. So luck reads any construct
/// nested on itself as deep as either does.
///
/// Every traversal recurses once per level, so this is what bounds their
/// stack, to [`MIN_STACK_BYTES`].
pub const MAX_NESTING_DEPTH: usize = 2001;

/// The stack a thread needs to run any luck stage on any input.
///
/// That's about six times the most any stage was measured to take on a tree
/// [`MAX_NESTING_DEPTH`] deep in an optimized build, on Linux and Windows,
/// and two and a half times in an unoptimized one. A main thread starts with
/// the platform's stack, 1 MiB on Windows, and a spawned Rust thread with
/// 2 MiB, so give this to every thread that parses, prints, formats,
/// minifies, analyzes, lints, or bundles, with
/// `std::thread::Builder::stack_size`. The linter runs a large file's rules
/// on rayon's pool, and `luck_bundler` reads modules there, so build that
/// with `ThreadPoolBuilder::stack_size` too. `luck_lsp` starts its own
/// workers with it.
///
/// A stack is reserved address space, and a thread takes memory only for the
/// pages it touches.
pub const MIN_STACK_BYTES: usize = 16 * 1024 * 1024;

/// How many levels deep `block` nests, itself included, or
/// `MAX_NESTING_DEPTH + 1` for anything deeper.
///
/// The walk stops there, so it is safe on a tree of any depth.
#[must_use]
pub fn block_nesting_depth(block: &Block<'_>) -> usize {
    let mut meter = DepthMeter { depth: 0, deepest: 0 };
    meter.visit_block(block);
    meter.deepest
}

/// How many levels deep `expr` nests, zero for a name or a literal, or
/// `MAX_NESTING_DEPTH + 1` for anything deeper.
///
/// The walk stops there, so it is safe on a tree of any depth.
#[must_use]
pub fn expr_nesting_depth(expr: &Expression<'_>) -> usize {
    let mut meter = DepthMeter { depth: 0, deepest: 0 };
    meter.visit_expr(expr);
    meter.deepest
}

/// How many levels deep `ty` nests, zero for a name, or
/// `MAX_NESTING_DEPTH + 1` for anything deeper.
///
/// The walk stops there, so it is safe on a type of any depth.
#[must_use]
pub fn type_nesting_depth(ty: &Type<'_>) -> usize {
    let mut meter = DepthMeter { depth: 0, deepest: 0 };
    meter.visit_type(ty);
    meter.deepest
}

/// The span of the innermost statement, expression, or type at each place
/// where `block`, sitting `depth` levels down, nests past
/// [`MAX_NESTING_DEPTH`]: one for each construct the cap refuses.
///
/// The walk stops at the cap, so it is safe on a tree of any depth.
#[must_use]
pub fn spans_past_nesting_cap(block: &Block<'_>, depth: usize) -> Vec<Span> {
    let mut finder = OverflowFinder { depth, node: block.span, spans: Vec::new() };
    finder.visit_block(block);
    finder.spans
}

struct DepthMeter {
    depth: usize,
    deepest: usize,
}

impl Visit<'_> for DepthMeter {
    fn descend(&mut self) -> bool {
        self.depth = self.depth.saturating_add(1);
        self.deepest = self.deepest.max(self.depth);
        self.depth <= MAX_NESTING_DEPTH
    }

    fn ascend(&mut self) {
        self.depth = self.depth.saturating_sub(1);
    }
}

struct OverflowFinder {
    depth: usize,
    /// The innermost statement, expression, or type the walk is in.
    node: Span,
    spans: Vec<Span>,
}

impl<'ast> Visit<'ast> for OverflowFinder {
    fn descend(&mut self) -> bool {
        self.depth = self.depth.saturating_add(1);
        let fits = self.depth <= MAX_NESTING_DEPTH;
        if !fits {
            self.spans.push(self.node);
        }
        fits
    }

    fn ascend(&mut self) {
        self.depth = self.depth.saturating_sub(1);
    }

    fn visit_stmt(&mut self, stmt: &'ast Statement<'ast>) {
        let outer = mem::replace(&mut self.node, stmt.span());
        self.walk_stmt(stmt);
        self.node = outer;
    }

    fn visit_expr(&mut self, expr: &'ast Expression<'ast>) {
        let outer = mem::replace(&mut self.node, expr.span());
        self.walk_expr(expr);
        self.node = outer;
    }

    fn visit_type(&mut self, ty: &'ast Type<'ast>) {
        let outer = mem::replace(&mut self.node, ty.span());
        self.walk_type(ty);
        self.node = outer;
    }
}

#[cfg(test)]
mod tests {
    use luck_token::{BinOp, UnOp};

    use super::*;
    use crate::expr::BinaryLink;
    use crate::fixtures::Fixture;
    use crate::one_or_more::OneOrMore;
    use crate::synth::{FunctionSignature, Synth};

    fn return_depth<'ast>(synth: Synth<'ast>, expr: Expression<'ast>) -> usize {
        block_nesting_depth(&synth.block([synth.return_([expr])]))
    }

    #[test]
    fn a_leaf_adds_no_level_to_its_block() {
        let fixture = Fixture::default();
        let synth = fixture.synth();
        assert_eq!(expr_nesting_depth(&synth.name_expr("x")), 0);
        assert_eq!(expr_nesting_depth(&synth.number_i64(1)), 0);
        assert_eq!(return_depth(synth, synth.name_expr("x")), 1);
    }

    #[test]
    fn each_wrapping_node_is_one_level() {
        let fixture = Fixture::default();
        let synth = fixture.synth();
        let leaf = || synth.name_expr("x");
        for (label, expr) in [
            ("parentheses", synth.single_value(synth.vararg())),
            ("unary operator", synth.unary_op(UnOp::Not, leaf())),
            ("binary operator", synth.binary_op(leaf(), BinOp::Add, leaf())),
            ("call", synth.call(leaf(), [leaf()])),
            ("string call", synth.call_string(leaf(), "s")),
            ("index", synth.index(leaf(), leaf())),
            ("field", synth.field(leaf(), "y")),
            ("instantiation", synth.type_instantiation(leaf(), [synth.ty_named("T")])),
            ("table", synth.array([leaf()])),
            ("empty table", synth.table([])),
        ] {
            assert_eq!(expr_nesting_depth(&expr), 1, "{label}");
        }
    }

    #[test]
    fn a_table_argument_nests_like_a_parenthesized_one() {
        let fixture = Fixture::default();
        let synth = fixture.synth();
        let bare = synth.call_table(synth.name_expr("f"), []);
        let parenthesized = synth.call(synth.name_expr("f"), [synth.table([])]);
        assert_eq!(expr_nesting_depth(&bare), 2);
        assert_eq!(expr_nesting_depth(&parenthesized), 2);
    }

    #[test]
    fn a_function_adds_its_body_and_its_block() {
        let fixture = Fixture::default();
        let synth = fixture.synth();
        let function = synth.function_def(&[], synth.block([]));
        assert_eq!(expr_nesting_depth(&function), 2);
    }

    #[test]
    fn siblings_count_the_deepest_one_only() {
        let fixture = Fixture::default();
        let synth = fixture.synth();
        let deep = synth.unary_op(UnOp::Not, synth.unary_op(UnOp::Not, synth.name_expr("x")));
        let shallow = synth.unary_op(UnOp::Not, synth.name_expr("y"));
        assert_eq!(expr_nesting_depth(&synth.binary_op(shallow, BinOp::Add, deep)), 3);
    }

    #[test]
    fn a_run_is_one_level_however_long() {
        let fixture = Fixture::default();
        let synth = fixture.synth();
        let mut sum = synth.name_expr("x");
        let mut path = synth.name_expr("t");
        let mut concat = synth.name_expr("s");
        for _ in 0..300u32 {
            sum = synth.binary_op(sum, BinOp::Sub, synth.number_i64(1));
            path = synth.method_call(synth.field(path, "a"), "m", []);
            concat = synth.binary_op(synth.name_expr("s"), BinOp::Concat, concat);
        }
        for run in [sum, path, concat] {
            assert_eq!(expr_nesting_depth(&run), 1);
            assert_eq!(return_depth(synth, run), 2);
        }
    }

    #[test]
    fn a_run_of_question_marks_is_one_level() {
        let fixture = Fixture::default();
        let synth = fixture.synth();
        let twice = synth.ty_optional(synth.ty_optional(synth.ty_named("T")));
        assert_eq!(type_nesting_depth(&twice), 1);
    }

    #[test]
    fn a_run_in_a_run_at_its_level_counts_its_own_level() {
        // `(a .. b) .. c`, which prints with its parentheses.
        let fixture = Fixture::default();
        let synth = fixture.synth();
        let allocator = fixture.allocator();
        let run = |first, operand| {
            let links = OneOrMore::new(BinaryLink::new(BinOp::Concat, operand), allocator);
            Expression::new_binary_op(Span::default(), first, links, allocator)
        };
        let nested = run(run(synth.name_expr("a"), synth.name_expr("b")), synth.name_expr("c"));
        assert_eq!(expr_nesting_depth(&nested), 2);
    }

    #[test]
    fn a_type_counts_the_levels_it_nests() {
        let fixture = Fixture::default();
        let synth = fixture.synth();
        assert_eq!(type_nesting_depth(&synth.ty_named("T")), 0);
        let nested = || synth.ty_optional(synth.ty_table_array(synth.ty_named("T")));
        assert_eq!(type_nesting_depth(&nested()), 2);
        let cast = synth.type_cast(synth.name_expr("x"), nested());
        assert_eq!(expr_nesting_depth(&cast), 1 + type_nesting_depth(&nested()));
    }

    #[test]
    fn spans_past_the_cap_name_each_construct_it_refuses() {
        let fixture = Fixture::default();
        let synth = fixture.synth();
        let negations = |count: usize| {
            let mut expr = synth.name_expr("x");
            for _ in 0..count {
                expr = synth.unary_op(UnOp::Not, expr);
            }
            expr
        };
        // The `level`th negation from the outside.
        let negation_span = |expr: &Expression<'_>, level: usize| {
            let mut current = expr;
            for _ in 1..level {
                if let Expression::UnaryOp(negation) = current {
                    current = &negation.operand;
                }
            }
            current.span()
        };
        let (deep, deeper) = (negations(10), negations(12));
        let expected = [negation_span(&deep, 8), negation_span(&deeper, 8)];
        let pair = synth.call(synth.name_expr("f"), [deep, deeper]);
        let block = synth.block([synth.call_stmt(pair)]);
        // The block and the call take two levels, so seven negations of each
        // argument fit under the cap from nine levels below it.
        assert_eq!(spans_past_nesting_cap(&block, MAX_NESTING_DEPTH - 9), expected);
        assert_eq!(spans_past_nesting_cap(&block, MAX_NESTING_DEPTH - 14), []);
    }

    #[test]
    fn nested_blocks_count_one_level_each() {
        let fixture = Fixture::default();
        let synth = fixture.synth();
        let mut block = synth.block([]);
        for _ in 0..9u32 {
            block = synth.block([synth.do_block(block)]);
        }
        assert_eq!(block_nesting_depth(&block), 10);
    }

    #[test]
    fn measuring_stops_one_level_past_the_cap() {
        let fixture = Fixture::default();
        let synth = fixture.synth();
        let allocator = fixture.allocator();
        let mut chain = synth.name_expr("x");
        for _ in 0..MAX_NESTING_DEPTH {
            chain = synth.unary_op(UnOp::Not, chain);
        }
        assert_eq!(expr_nesting_depth(&chain), MAX_NESTING_DEPTH);
        for _ in 0..50_000u32 {
            chain = Expression::new_unary_op(Span::default(), UnOp::Not, chain, allocator);
        }
        assert_eq!(expr_nesting_depth(&chain), MAX_NESTING_DEPTH + 1);
    }

    #[test]
    fn attribute_arguments_nest_under_their_function() {
        let fixture = Fixture::default();
        let synth = fixture.synth();
        let nested = synth.array([synth.array([synth.array([])])]);
        let attribute = synth.function_attribute_with_args("deprecated", [nested]);
        let function =
            synth.function_def_full([attribute], FunctionSignature::default(), synth.block([]));
        assert_eq!(expr_nesting_depth(&function), 4);
    }
}
