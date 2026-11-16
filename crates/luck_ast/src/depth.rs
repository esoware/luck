use core::mem;

use luck_token::{Span, UnOp};

use crate::expr::{BinaryOp, Expression};
use crate::function::FunctionAttribute;
use crate::query::ends_in_if_expr;
use crate::stmt::{Block, Statement};
use crate::types::Type;
use crate::visitor::Visitor;

/// The deepest nesting any dialect parses, Luau's, in the levels
/// [`Visitor::descend`] counts. Each dialect's own cap is
/// [`LuaVersion::max_nesting_depth`](luck_token::LuaVersion::max_nesting_depth).
///
/// Every traversal recurses a bounded number of times per level, drop and
/// clone included, so this is what bounds their stack.
pub const MAX_NESTING_DEPTH: usize = 1000;

/// How many levels deep `block`'s statements nest, or
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

/// How many levels deep `ty` nests, zero for a name, or
/// `MAX_NESTING_DEPTH + 1` for anything deeper.
///
/// The walk stops there, so it is safe on a type of any depth.
#[must_use]
pub fn type_nesting_depth(ty: &Type) -> usize {
    let mut meter = DepthMeter { depth: 0, deepest: 0 };
    meter.visit_type(ty);
    meter.deepest
}

/// The span of the innermost statement, expression, or type at each place
/// where `block`, sitting `depth` levels down, nests past `limit`: one for
/// each construct the cap refuses.
///
/// The walk stops at the cap, so it is safe on a tree of any depth.
#[must_use]
pub fn spans_past_nesting_cap(block: &Block, depth: usize, limit: usize) -> Vec<Span> {
    let mut finder = OverflowFinder { depth, limit, node: block.span, spans: Vec::new() };
    finder.visit_block(block);
    finder.spans
}

// A child the parser reads as part of its parent opens no level of its own,
// but a tree built in code can hold one bare where the printer has to put
// parentheses, which the parser reads as a level. The walkers count that
// level too, so a tree is never shallower than what it prints.

/// What the rules for where an expression needs parentheses look at.
#[derive(Clone, Copy)]
enum Form {
    /// Anything Luau can cast without parentheses.
    Simple,
    /// A run of binary operators at this precedence.
    Run(u8),
    Unary,
    IfExpr,
    Cast,
}

fn form(expr: &Expression) -> Form {
    match expr {
        Expression::BinaryOp(run) => Form::Run(run.precedence().0),
        Expression::UnaryOp(_) => Form::Unary,
        // Luau
        Expression::IfExpression(_) => Form::IfExpr,
        // Luau
        Expression::TypeCast(_) => Form::Cast,
        Expression::Nil(_)
        | Expression::False(_)
        | Expression::True(_)
        | Expression::Number(_)
        | Expression::Integer(_)
        | Expression::StringLiteral(_)
        | Expression::VarArg(_)
        | Expression::FunctionDef(_)
        | Expression::Name(_)
        | Expression::Suffixed(_)
        | Expression::Parenthesized(_)
        | Expression::TableConstructor(_)
        | Expression::InterpolatedString(_)
        | Expression::Error(_) => Form::Simple,
    }
}

pub(crate) fn first_operand_needs_parens(run: &BinaryOp) -> bool {
    let precedence = run.precedence().0;
    let binds_looser = match form(&run.first) {
        Form::Run(inner) => inner <= precedence,
        Form::Unary => precedence > UnOp::PRECEDENCE,
        Form::Simple | Form::IfExpr | Form::Cast => false,
    };
    binds_looser || ends_in_if_expr(&run.first)
}

/// Only the last operand can end in an if-expression, which takes in
/// everything after it.
pub(crate) fn link_operand_needs_parens(
    precedence: u8,
    operand: &Expression,
    is_last: bool,
) -> bool {
    let binds_looser = matches!(form(operand), Form::Run(inner) if inner <= precedence);
    binds_looser || (!is_last && ends_in_if_expr(operand))
}

pub(crate) fn unary_operand_needs_parens(operand: &Expression) -> bool {
    matches!(form(operand), Form::Run(inner) if inner < UnOp::PRECEDENCE)
}

pub(crate) fn cast_operand_needs_parens(operand: &Expression) -> bool {
    match form(operand) {
        Form::Simple => false,
        Form::Run(_) | Form::Unary | Form::IfExpr | Form::Cast => true,
    }
}

/// What the rules for where a type needs parentheses look at.
#[derive(Clone, Copy)]
enum TypeForm {
    /// Anything a `?` can follow without parentheses.
    Simple,
    Optional,
    Function,
    Compound,
}

const fn type_form(type_value: &Type) -> TypeForm {
    match type_value {
        Type::Name { .. }
        | Type::Named(_)
        | Type::Typeof(_)
        | Type::Table(_)
        | Type::Singleton(_)
        | Type::Parenthesized(_)
        | Type::Error(_) => TypeForm::Simple,
        Type::Optional(_) => TypeForm::Optional,
        Type::Function(_) => TypeForm::Function,
        Type::Union(_)
        | Type::Intersection(_)
        | Type::Pack(_)
        | Type::Variadic(_)
        | Type::GenericPack(_) => TypeForm::Compound,
    }
}

pub(crate) const fn optional_inner_needs_parens(inner: &Type) -> bool {
    !matches!(type_form(inner), TypeForm::Simple)
}

/// A function type's return type takes in what follows it, so only the last
/// member can be one.
pub(crate) const fn union_member_needs_parens(member: &Type, is_last: bool) -> bool {
    match type_form(member) {
        TypeForm::Simple | TypeForm::Optional => false,
        TypeForm::Function => !is_last,
        TypeForm::Compound => true,
    }
}

/// As in a union, but a `?` beside `&` needs parentheses too.
pub(crate) const fn intersection_member_needs_parens(member: &Type, is_last: bool) -> bool {
    match type_form(member) {
        TypeForm::Simple => false,
        TypeForm::Function => !is_last,
        TypeForm::Optional | TypeForm::Compound => true,
    }
}

struct DepthMeter {
    depth: usize,
    deepest: usize,
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
        visit_stmt_attributes(self, stmt);
        self.walk_stmt(stmt);
    }

    fn visit_expr(&mut self, expr: &'ast Expression) {
        visit_expr_attributes(self, expr);
        self.walk_expr(expr);
    }
}

struct OverflowFinder {
    depth: usize,
    limit: usize,
    /// The innermost statement, expression, or type the walk is in.
    node: Span,
    spans: Vec<Span>,
}

impl<'ast> Visitor<'ast> for OverflowFinder {
    fn descend(&mut self) -> bool {
        self.depth = self.depth.saturating_add(1);
        let fits = self.depth <= self.limit;
        if !fits {
            self.spans.push(self.node);
        }
        fits
    }

    fn ascend(&mut self) {
        self.depth = self.depth.saturating_sub(1);
    }

    fn visit_stmt(&mut self, stmt: &'ast Statement) {
        let outer = mem::replace(&mut self.node, stmt.span());
        visit_stmt_attributes(self, stmt);
        self.walk_stmt(stmt);
        self.node = outer;
    }

    fn visit_expr(&mut self, expr: &'ast Expression) {
        let outer = mem::replace(&mut self.node, expr.span());
        visit_expr_attributes(self, expr);
        self.walk_expr(expr);
        self.node = outer;
    }

    fn visit_type(&mut self, type_value: &'ast Type) {
        let outer = mem::replace(&mut self.node, type_value.span());
        self.walk_type(type_value);
        self.node = outer;
    }
}

fn visit_stmt_attributes<'ast, Meter>(meter: &mut Meter, stmt: &'ast Statement)
where
    Meter: Visitor<'ast>,
{
    if let Statement::FunctionDecl(function_decl) = stmt {
        visit_attributes(meter, &function_decl.attributes);
    }
    if let Statement::LocalFunction(local_function) = stmt {
        visit_attributes(meter, &local_function.attributes);
    }
}

fn visit_expr_attributes<'ast, Meter>(meter: &mut Meter, expr: &'ast Expression)
where
    Meter: Visitor<'ast>,
{
    if let Expression::FunctionDef(function_def) = expr {
        visit_attributes(meter, &function_def.attributes);
    }
}

/// The walkers skip attribute arguments, each of which the parser reads as
/// a level under the function they sit on.
fn visit_attributes<'ast, Meter>(meter: &mut Meter, attributes: &'ast [FunctionAttribute])
where
    Meter: Visitor<'ast>,
{
    for args in attributes.iter().filter_map(|attribute| attribute.args.as_ref()) {
        for arg in args.iter() {
            if meter.descend() {
                meter.visit_expr(arg);
            }
            meter.ascend();
        }
    }
}

#[cfg(test)]
mod tests {
    use luck_token::{BinOp, LuaVersion};

    use super::*;
    use crate::expr::{BinaryLink, UnaryOp};
    use crate::one_or_more::OneOrMore;
    use crate::synth::{FieldSpec, FunctionSignature, Synth};

    fn return_depth(expr: Expression) -> usize {
        let synth = Synth::new();
        block_nesting_depth(&synth.block(Vec::new(), Some(synth.return_(vec![expr]))))
    }

    fn run(first: Expression, op: BinOp, operand: Expression) -> Expression {
        Expression::BinaryOp(Box::new(BinaryOp {
            span: Span::default(),
            first,
            links: OneOrMore::new(BinaryLink { op, operand }),
        }))
    }

    #[test]
    fn no_dialect_nests_past_the_cap() {
        for version in LuaVersion::ALL {
            assert!(version.max_nesting_depth() <= MAX_NESTING_DEPTH, "{version:?}");
        }
    }

    #[test]
    fn a_statement_and_its_expression_are_a_level_each() {
        let synth = Synth::new();
        assert_eq!(expr_nesting_depth(&synth.name_expr("x")), 0);
        assert_eq!(expr_nesting_depth(&synth.number("1")), 0);
        assert_eq!(return_depth(synth.name_expr("x")), 2);
        assert_eq!(block_nesting_depth(&synth.block(Vec::new(), None)), 0);
    }

    #[test]
    fn each_nested_expression_is_one_level() {
        let synth = Synth::new();
        let leaf = || synth.name_expr("x");
        for (label, expr) in [
            ("parentheses", synth.parenthesized(leaf())),
            ("unary operator", synth.unary_op(UnOp::Not, leaf())),
            ("binary operator", synth.binary_op(leaf(), BinOp::Add, leaf())),
            ("call", synth.call(leaf(), vec![leaf()])),
            ("index", synth.index(leaf(), leaf())),
            ("instantiation", synth.type_instantiation(leaf(), vec![synth.ty_named("T")])),
            ("table", synth.array(vec![leaf()])),
        ] {
            assert_eq!(expr_nesting_depth(&expr), 1, "{label}");
        }
    }

    #[test]
    fn a_suffix_or_constructor_without_expressions_opens_no_level() {
        let synth = Synth::new();
        let leaf = || synth.name_expr("x");
        for (label, expr) in [
            ("string call", synth.call_string(leaf(), "s")),
            ("field", synth.field(leaf(), "y")),
            ("empty table", synth.table(Vec::new())),
            ("empty call", synth.call(leaf(), Vec::new())),
        ] {
            assert_eq!(expr_nesting_depth(&expr), 0, "{label}");
        }
    }

    #[test]
    fn a_table_argument_opens_no_level_of_its_own() {
        let synth = Synth::new();
        let bare = synth
            .call_table(synth.name_expr("f"), vec![FieldSpec::Positional(synth.name_expr("x"))]);
        let parenthesized =
            synth.call(synth.name_expr("f"), vec![synth.array(vec![synth.name_expr("x")])]);
        assert_eq!(expr_nesting_depth(&bare), 1);
        assert_eq!(expr_nesting_depth(&parenthesized), 2);
    }

    #[test]
    fn a_function_body_counts_its_statements() {
        let synth = Synth::new();
        let empty = synth.function_def(&[], synth.block(Vec::new(), None));
        assert_eq!(expr_nesting_depth(&empty), 0);
        let returning = synth.function_def(
            &[],
            synth.block(Vec::new(), Some(synth.return_(vec![synth.name_expr("x")]))),
        );
        assert_eq!(expr_nesting_depth(&returning), 2);
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
        assert_eq!(expr_nesting_depth(&sum), 1);
        assert_eq!(expr_nesting_depth(&path), 0);
        assert_eq!(expr_nesting_depth(&concat), 1);
        assert_eq!(return_depth(sum), 3);
    }

    #[test]
    fn a_run_of_question_marks_opens_no_level() {
        let synth = Synth::new();
        let twice = synth.ty_optional(synth.ty_optional(synth.ty_named("T")));
        assert_eq!(type_nesting_depth(&twice), 0);
    }

    #[test]
    fn a_bare_run_nested_in_a_run_counts_its_own_level() {
        // `(a .. b) .. c` built with no parentheses prints `a .. b .. c`,
        // which reads back as one run, so the tree is never shallower than
        // what it prints.
        let leaf = |name: &str| Synth::new().name_expr(name);
        let nested = run(run(leaf("a"), BinOp::Concat, leaf("b")), BinOp::Concat, leaf("c"));
        assert_eq!(expr_nesting_depth(&nested), 2);
    }

    #[test]
    fn a_shape_the_parser_reads_only_in_parentheses_counts_them() {
        let synth = Synth::new();
        let leaf = |name: &str| synth.name_expr(name);
        let looser = || run(leaf("a"), BinOp::Add, leaf("b"));
        let if_expr = || synth.if_expr(leaf("c"), leaf("t"), Vec::new(), leaf("e"));
        for (label, expr, depth) in [
            ("looser first operand", run(looser(), BinOp::Mul, leaf("c")), 2),
            ("looser operand", run(leaf("c"), BinOp::Mul, looser()), 3),
            ("looser unary operand", synth.unary_op(UnOp::Neg, looser()), 3),
            ("if-expression first", run(if_expr(), BinOp::Add, leaf("x")), 2),
            ("if-expression last", run(leaf("x"), BinOp::Add, if_expr()), 2),
        ] {
            assert_eq!(expr_nesting_depth(&expr), depth, "{label}");
        }
    }

    #[test]
    fn a_type_counts_the_levels_it_nests() {
        let synth = Synth::new();
        assert_eq!(type_nesting_depth(&synth.ty_named("T")), 0);
        let nested = synth.ty_optional(synth.ty_table_array(synth.ty_named("T")));
        assert_eq!(type_nesting_depth(&nested), 1);
        let cast = synth.type_cast(synth.name_expr("x"), nested.clone());
        assert_eq!(expr_nesting_depth(&cast), 1 + type_nesting_depth(&nested));
    }

    #[test]
    fn spans_past_the_cap_name_each_construct_it_refuses() {
        let synth = Synth::new();
        let groups = |count: usize| {
            let mut expr = synth.name_expr("x");
            for _ in 0..count {
                expr = synth.parenthesized(expr);
            }
            expr
        };
        // The `level`th group from the outside.
        let group_span = |expr: &Expression, level: usize| {
            let mut current = expr;
            for _ in 1..level {
                if let Expression::Parenthesized(parenthesized) = current {
                    current = &parenthesized.expr;
                }
            }
            current.span()
        };
        let (deep, deeper) = (groups(10), groups(12));
        let expected = [group_span(&deep, 8), group_span(&deeper, 8)];
        let pair = synth.call(synth.name_expr("f"), vec![deep, deeper]);
        let block = synth.block(vec![synth.call_stmt(pair)], None);
        // The statement and each argument take two levels, so seven groups
        // of each argument fit under the cap from nine levels below it.
        let cap = MAX_NESTING_DEPTH;
        assert_eq!(spans_past_nesting_cap(&block, cap - 9, cap), expected);
        assert_eq!(spans_past_nesting_cap(&block, cap - 14, cap), []);
    }

    #[test]
    fn nested_blocks_count_one_level_each() {
        let synth = Synth::new();
        let mut block = synth.block(Vec::new(), None);
        for _ in 0..9u32 {
            block = synth.block(vec![synth.do_block(block)], None);
        }
        assert_eq!(block_nesting_depth(&block), 9);
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
        assert_eq!(expr_nesting_depth(&function), 3);
    }
}
