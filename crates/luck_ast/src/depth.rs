use crate::expr::Expression;
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
    use crate::expr::{BinaryLink, BinaryOp, UnaryOp};
    use crate::one_or_more::OneOrMore;
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
            ("instantiation", synth.type_instantiation(leaf(), vec![synth.ty_named("T")])),
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
    fn a_run_is_one_level_however_long() {
        let synth = Synth::new();
        let mut sum = synth.name_expr("x");
        let mut path = synth.name_expr("t");
        let mut concat = synth.name_expr("s");
        for _ in 0..300u32 {
            sum = synth.binary_op(sum, BinOp::Sub, synth.number("1"));
            path = synth.method_call(synth.field(path, "a"), "m", Vec::new());
            concat = synth.binary_op(synth.name_expr("s"), BinOp::Concat, concat);
        }
        for run in [sum, path, concat] {
            assert_eq!(expr_nesting_depth(&run), 1);
            assert_eq!(return_depth(run), 2);
        }
    }

    #[test]
    fn a_bare_run_nested_in_a_run_counts_its_own_level() {
        // `(a .. b) .. c` built with no parentheses prints `a .. b .. c`,
        // which reads back as one run, so the tree is never shallower than
        // what it prints.
        let leaf = |name: &str| Synth::new().name_expr(name);
        let run = |first: Expression, op: BinOp, operand: Expression| {
            Expression::BinaryOp(Box::new(BinaryOp {
                span: Span::default(),
                first,
                links: OneOrMore::new(BinaryLink { op, operand }),
            }))
        };
        let nested = run(run(leaf("a"), BinOp::Concat, leaf("b")), BinOp::Concat, leaf("c"));
        assert_eq!(expr_nesting_depth(&nested), 2);
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
