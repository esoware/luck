use luck_token::Associativity;

use crate::expr::{BinaryOp, Expression};
use crate::function::FunctionAttribute;
use crate::stmt::{Block, Statement};
use crate::visitor::Visitor;

/// The deepest tree `luck_parser` builds, in the levels [`Visitor::descend`]
/// counts.
///
/// Every traversal recurses once per level, drop and clone included, so this
/// is what bounds their stack.
pub const MAX_NESTING_DEPTH: usize = 256;

/// How many levels deep `block` nests, itself included, or
/// `MAX_NESTING_DEPTH + 1` for anything deeper.
///
/// The walk stops there, so it is safe on a tree of any depth.
#[must_use]
pub fn block_nesting_depth(block: &Block) -> usize {
    let mut meter = DepthMeter { depth: 0, deepest: 0 };
    meter.visit_block(block);
    meter.deepest
}

/// How many levels deep `expr` nests, zero for a name or a literal, or
/// `MAX_NESTING_DEPTH + 1` for anything deeper.
///
/// The walk stops there, so it is safe on a tree of any depth.
#[must_use]
pub fn expr_nesting_depth(expr: &Expression) -> usize {
    let mut meter = DepthMeter { depth: 0, deepest: 0 };
    meter.visit_expr(expr);
    meter.deepest
}

/// How many levels deeper each operand of `binary_op` sits once the tree is
/// printed and read back, left operand first.
///
/// Printing drops the grouping of an operand that is a bare operator of its
/// parent's precedence on the side the parent doesn't associate to. `a or (b
/// or c)` built with no `Parenthesized` node reads back as `(a or b) or c`,
/// with `a` one level deeper. The walkers count these levels, so a tree's
/// depth is its depth as printed. The parser builds no such tree, and for
/// one of its own both counts are zero.
#[must_use]
pub fn regrouped_levels(binary_op: &BinaryOp) -> (usize, usize) {
    let (precedence, associativity) = binary_op.op.precedence();
    match associativity {
        Associativity::Left => (bare_links(&binary_op.rhs, precedence), 0),
        Associativity::Right => (0, bare_links(&binary_op.lhs, precedence)),
    }
}

/// Operators of `precedence` that `expr` joins with no parentheses between
/// them, counted to one past the cap.
fn bare_links(expr: &Expression, precedence: u8) -> usize {
    let is_link = |candidate: &Expression| matches!(candidate, Expression::BinaryOp(inner) if inner.op.precedence().0 == precedence);
    if !is_link(expr) {
        return 0;
    }
    let mut links = 0usize;
    let mut pending = vec![expr];
    while let Some(next) = pending.pop() {
        if let Expression::BinaryOp(inner) = next
            && is_link(next)
        {
            links = links.saturating_add(1);
            if links > MAX_NESTING_DEPTH {
                break;
            }
            pending.push(&inner.lhs);
            pending.push(&inner.rhs);
        }
    }
    links
}

struct DepthMeter {
    depth: usize,
    deepest: usize,
}

impl DepthMeter {
    /// The walkers skip attribute arguments, which the parser nests one level
    /// under the function they sit on.
    fn visit_attributes(&mut self, attributes: &[FunctionAttribute]) {
        for args in attributes.iter().filter_map(|attribute| attribute.args.as_ref()) {
            if self.descend() {
                for arg in args.iter() {
                    self.visit_expr(arg);
                }
            }
            self.ascend();
        }
    }
}

impl<'ast> Visitor<'ast> for DepthMeter {
    fn descend(&mut self) -> bool {
        self.depth = self.depth.saturating_add(1);
        self.deepest = self.deepest.max(self.depth);
        self.depth <= MAX_NESTING_DEPTH
    }

    fn ascend(&mut self) {
        self.depth = self.depth.saturating_sub(1);
    }

    fn visit_stmt(&mut self, stmt: &'ast Statement) {
        if let Statement::FunctionDecl(function_decl) = stmt {
            self.visit_attributes(&function_decl.attributes);
        }
        if let Statement::LocalFunction(local_function) = stmt {
            self.visit_attributes(&local_function.attributes);
        }
        self.walk_stmt(stmt);
    }

    fn visit_expr(&mut self, expr: &'ast Expression) {
        if let Expression::FunctionDef(function_def) = expr {
            self.visit_attributes(&function_def.attributes);
        }
        self.walk_expr(expr);
    }
}

#[cfg(test)]
mod tests {
    use luck_token::{BinOp, Span, UnOp};

    use super::*;
    use crate::expr::{UnaryOp, Var};
    use crate::synth::{FunctionSignature, Synth};

    fn return_depth(expr: Expression) -> usize {
        let synth = Synth::new();
        block_nesting_depth(&synth.block(Vec::new(), Some(synth.return_(vec![expr]))))
    }

    #[test]
    fn a_leaf_adds_no_level_to_its_block() {
        let synth = Synth::new();
        assert_eq!(expr_nesting_depth(&synth.name_expr("x")), 0);
        assert_eq!(expr_nesting_depth(&synth.number("1")), 0);
        assert_eq!(return_depth(synth.name_expr("x")), 1);
    }

    #[test]
    fn each_wrapping_node_is_one_level() {
        let synth = Synth::new();
        let leaf = || synth.name_expr("x");
        for (label, expr) in [
            ("parentheses", synth.parenthesized(leaf())),
            ("unary operator", synth.unary_op(UnOp::Not, leaf())),
            ("binary operator", synth.binary_op(leaf(), BinOp::Add, leaf())),
            ("call", synth.call(leaf(), vec![leaf()])),
            ("string call", synth.call_string(leaf(), "s")),
            ("index", synth.index(leaf(), leaf())),
            ("field", synth.field(leaf(), "y")),
            ("table", synth.array(vec![leaf()])),
            ("empty table", synth.table(Vec::new())),
        ] {
            assert_eq!(expr_nesting_depth(&expr), 1, "{label}");
        }
    }

    #[test]
    fn a_table_argument_nests_like_a_parenthesized_one() {
        let synth = Synth::new();
        let bare = synth.call_table(synth.name_expr("f"), Vec::new());
        let parenthesized = synth.call(synth.name_expr("f"), vec![synth.table(Vec::new())]);
        assert_eq!(expr_nesting_depth(&bare), 2);
        assert_eq!(expr_nesting_depth(&parenthesized), 2);
    }

    #[test]
    fn a_function_adds_its_body_and_its_block() {
        let synth = Synth::new();
        let function = synth.function_def(&[], synth.block(Vec::new(), None));
        assert_eq!(expr_nesting_depth(&function), 2);
    }

    #[test]
    fn siblings_count_the_deepest_one_only() {
        let synth = Synth::new();
        let deep = synth.parenthesized(synth.parenthesized(synth.name_expr("x")));
        let shallow = synth.parenthesized(synth.name_expr("y"));
        assert_eq!(expr_nesting_depth(&synth.binary_op(shallow, BinOp::Add, deep)), 3);
    }

    #[test]
    fn a_left_chain_nests_one_level_per_link() {
        let synth = Synth::new();
        let mut chain = synth.name_expr("x");
        for _ in 0..40u32 {
            chain = synth.binary_op(chain, BinOp::Add, synth.number("1"));
        }
        assert_eq!(expr_nesting_depth(&chain), 40);
        assert_eq!(return_depth(chain), 41);
    }

    #[test]
    fn a_bare_chain_on_the_side_its_operator_does_not_group_to_counts_as_it_prints() {
        let leaf = |name: &str| Expression::Var(Var::Name(Synth::new().identifier(name)));
        let bare = |lhs: Expression, op: BinOp, rhs: Expression| {
            Expression::BinaryOp(Box::new(BinaryOp { span: Span::default(), lhs, op, rhs }))
        };
        // `a or (b or (c or d))` with no parentheses prints `a or b or c or d`,
        // which nests `a` under three operators.
        let right_nested = bare(
            leaf("a"),
            BinOp::Or,
            bare(leaf("b"), BinOp::Or, bare(leaf("c"), BinOp::Or, leaf("d"))),
        );
        assert_eq!(expr_nesting_depth(&right_nested), 3);
        let Expression::BinaryOp(root) = &right_nested else {
            panic!("the chain is a binary operator");
        };
        assert_eq!(regrouped_levels(root), (2, 0));

        // `(a .. b) .. c` with no parentheses prints `a .. b .. c`, which
        // nests `c` under both operators.
        let left_nested = bare(bare(leaf("a"), BinOp::Concat, leaf("b")), BinOp::Concat, leaf("c"));
        assert_eq!(expr_nesting_depth(&left_nested), 2);

        // A chain grouped the way its operator reads sinks nothing.
        let parsed_shape = bare(bare(leaf("a"), BinOp::Or, leaf("b")), BinOp::Or, leaf("c"));
        let Expression::BinaryOp(parsed_root) = &parsed_shape else {
            panic!("the chain is a binary operator");
        };
        assert_eq!(regrouped_levels(parsed_root), (0, 0));
        assert_eq!(expr_nesting_depth(&parsed_shape), 2);

        // Two bare chains under one operator read back as one long chain.
        let pair = |first: &str, second: &str| bare(leaf(first), BinOp::Or, leaf(second));
        let balanced = bare(
            bare(pair("a", "b"), BinOp::Or, pair("c", "d")),
            BinOp::Or,
            bare(pair("e", "f"), BinOp::Or, pair("g", "h")),
        );
        assert_eq!(expr_nesting_depth(&balanced), 7);
    }

    #[test]
    fn nested_blocks_count_one_level_each() {
        let synth = Synth::new();
        let mut block = synth.block(Vec::new(), None);
        for _ in 0..9u32 {
            block = synth.block(vec![synth.do_block(block)], None);
        }
        assert_eq!(block_nesting_depth(&block), 10);
    }

    #[test]
    fn measuring_stops_one_level_past_the_cap() {
        let synth = Synth::new();
        let mut chain = synth.name_expr("x");
        for _ in 0..MAX_NESTING_DEPTH {
            chain = synth.unary_op(UnOp::Not, chain);
        }
        assert_eq!(expr_nesting_depth(&chain), MAX_NESTING_DEPTH);
        for _ in 0..50_000u32 {
            chain = Expression::UnaryOp(Box::new(UnaryOp {
                span: Span::default(),
                op: UnOp::Not,
                operand: chain,
            }));
        }
        assert_eq!(expr_nesting_depth(&chain), MAX_NESTING_DEPTH + 1);
        // Dropped whole, a tree this deep recurses once per level.
        while let Expression::UnaryOp(mut unary_op) = chain {
            chain = unary_op.operand.take();
        }
    }

    #[test]
    fn attribute_arguments_nest_under_their_function() {
        let synth = Synth::new();
        let nested = synth.array(vec![synth.array(vec![synth.array(Vec::new())])]);
        let attribute = synth.function_attribute("deprecated", Some(vec![nested]));
        let function = synth.function_def_full(
            vec![attribute],
            FunctionSignature::default(),
            synth.block(Vec::new(), None),
        );
        assert_eq!(expr_nesting_depth(&function), 4);
    }
}
