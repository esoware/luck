use luck_ast::{
    AstKind, AstType, Block, Expression, FunctionArgs, FunctionBody, FunctionSignature,
    GenericTypeList, GetSpan as _, InterpolatedStringPart, Name, Statement, Synth,
    TableConstructor, Type, Visit,
};
use luck_token::{BinOp, UnOp};

use crate::common::Fixture;

#[derive(Debug, Default, PartialEq, Eq)]
struct HookCounts {
    statements: usize,
    expressions: usize,
    calls: usize,
    tables: usize,
    bodies: usize,
    generic_lists: usize,
}

impl<'ast> Visit<'ast> for HookCounts {
    fn visit_stmt(&mut self, stmt: &'ast Statement<'ast>) {
        self.statements = self.statements.saturating_add(1);
        self.walk_stmt(stmt);
    }

    fn visit_expr(&mut self, expr: &'ast Expression<'ast>) {
        self.expressions = self.expressions.saturating_add(1);
        self.walk_expr(expr);
    }

    fn visit_function_args(&mut self, function_args: &'ast FunctionArgs<'ast>) {
        self.calls = self.calls.saturating_add(1);
        self.walk_function_args(function_args);
    }

    fn visit_table_constructor(&mut self, table_constructor: &'ast TableConstructor<'ast>) {
        self.tables = self.tables.saturating_add(1);
        self.walk_table_constructor(table_constructor);
    }

    fn visit_function_body(&mut self, function_body: &'ast FunctionBody<'ast>) {
        self.bodies = self.bodies.saturating_add(1);
        self.walk_function_body(function_body);
    }

    fn visit_generic_type_list(&mut self, generic_type_list: &'ast GenericTypeList<'ast>) {
        self.generic_lists = self.generic_lists.saturating_add(1);
        self.walk_generic_type_list(generic_type_list);
    }
}

fn hook_counts(block: &Block<'_>) -> HookCounts {
    let mut counts = HookCounts::default();
    counts.visit_block(block);
    counts
}

fn expr_count<'ast, Stmts>(synth: Synth<'ast>, stmts: Stmts) -> usize
where
    Stmts: IntoIterator<Item = Statement<'ast>>,
    Stmts::IntoIter: ExactSizeIterator,
{
    hook_counts(&synth.block(stmts)).expressions
}

#[test]
fn reaches_binary_operands() {
    // local x = 1 + 1: the sum and both operands.
    let fixture = Fixture::default();
    let synth = fixture.synth();
    let sum = synth.binary_op(synth.number_i64(1), BinOp::Add, synth.number_i64(1));
    assert_eq!(expr_count(synth, [synth.local(&["x"], [sum])]), 3);
}

#[test]
fn reaches_table_fields() {
    // local t = {1, 1, 1}: the constructor and its three values.
    let fixture = Fixture::default();
    let synth = fixture.synth();
    let table = synth.array([synth.number_i64(1), synth.number_i64(1), synth.number_i64(1)]);
    assert_eq!(expr_count(synth, [synth.local(&["t"], [table])]), 4);
}

#[test]
fn visit_stmt_reaches_the_return_that_ends_a_block() {
    let fixture = Fixture::default();
    let synth = fixture.synth();
    let block = synth.block([
        synth.local(&["x"], [synth.number_i64(1)]),
        synth.local(&["y"], [synth.number_i64(1)]),
        synth.return_([synth.name_expr("x")]),
    ]);
    assert_eq!(hook_counts(&block).statements, 3);
}

#[test]
fn reaches_function_bodies() {
    // local function f(a) return a + 1 end: the sum and both operands.
    let fixture = Fixture::default();
    let synth = fixture.synth();
    let sum = synth.binary_op(synth.name_expr("a"), BinOp::Add, synth.number_i64(1));
    let body = synth.block([synth.return_([sum])]);
    assert_eq!(expr_count(synth, [synth.local_function("f", &["a"], body)]), 3);
}

#[test]
fn reaches_if_conditions_and_branches() {
    // if x then y = 1 else y = nil end: x, 1, and nil.
    let fixture = Fixture::default();
    let synth = fixture.synth();
    let then_block = synth.block([synth.assign([synth.name_var("y")], [synth.number_i64(1)])]);
    let else_block = synth.block([synth.assign([synth.name_var("y")], [synth.nil()])]);
    let if_stmt = synth.if_(synth.name_expr("x"), then_block, [], Some(else_block));
    assert_eq!(expr_count(synth, [if_stmt]), 3);
}

#[test]
fn reaches_numeric_for_bounds_and_step() {
    // for i = 1, 10, 2 do end
    let fixture = Fixture::default();
    let synth = fixture.synth();
    let numeric_for = synth.numeric_for(
        synth.param("i"),
        synth.number_i64(1),
        synth.number_i64(10),
        Some(synth.number_i64(2)),
        synth.block([]),
    );
    assert_eq!(expr_count(synth, [numeric_for]), 3);
}

#[test]
fn reaches_interpolated_expressions_and_attribute_arguments() {
    // local f = @[deprecated("x")] function() return `{a}{b}` end: the
    // function, its argument, the string, and both holes.
    let fixture = Fixture::default();
    let synth = fixture.synth();
    let interpolated = synth.interpolated_string([
        InterpolatedStringPart::Expr(synth.name_expr("a")),
        InterpolatedStringPart::Expr(synth.name_expr("b")),
    ]);
    let function = synth.function_def_full(
        [synth.function_attribute_with_args("deprecated", [synth.string("x")])],
        FunctionSignature::default(),
        synth.block([synth.return_([interpolated])]),
    );
    assert_eq!(expr_count(synth, [synth.local(&["f"], [function])]), 5);
}

#[test]
fn a_call_statement_visits_its_callee_and_arguments_but_no_call_expr() {
    // f(x): the call is a statement here, so only `f` and `x` count.
    let fixture = Fixture::default();
    let synth = fixture.synth();
    let call = synth.call(synth.name_expr("f"), [synth.name_expr("x")]);
    assert_eq!(expr_count(synth, [synth.call_stmt(call)]), 2);
}

#[test]
fn hooks_fire_in_stmt_and_expr_positions() {
    // f{}(function<T>() end); local t = {}
    let fixture = Fixture::default();
    let synth = fixture.synth();
    let generic_function = synth.function_def_full(
        [],
        FunctionSignature {
            generics: Some(synth.generic_type_list([("T", false)])),
            ..FunctionSignature::default()
        },
        synth.block([]),
    );
    let table_call = synth.call_table(synth.name_expr("f"), []);
    let outer_call = synth.call(table_call, [generic_function]);
    let block = synth.block([synth.call_stmt(outer_call), synth.local(&["t"], [synth.table([])])]);
    let counts = hook_counts(&block);
    assert_eq!(counts.calls, 2, "{counts:?}");
    assert_eq!(counts.tables, 2, "{counts:?}");
    assert_eq!(counts.bodies, 1, "{counts:?}");
    assert_eq!(counts.generic_lists, 1, "{counts:?}");
}

/// Every name's text, in the order the walk reaches them.
#[derive(Default)]
struct NameTexts<'ast>(Vec<&'ast str>);

impl<'ast> Visit<'ast> for NameTexts<'ast> {
    fn visit_name(&mut self, name: &'ast Name<'ast>) {
        self.0.push(name.text.as_str());
    }
}

#[test]
fn reaches_every_name_in_source_order() {
    // local x <const> = t.a:m(y); goto done; ::done::
    let fixture = Fixture::default();
    let synth = fixture.synth();
    let call =
        synth.method_call(synth.field(synth.name_expr("t"), "a"), "m", [synth.name_expr("y")]);
    let local = synth.local_full([synth.attributed_name("x", None, Some("const"))], [call]);
    let block = synth.block([local, synth.goto_("done"), synth.label("done")]);
    let mut names = NameTexts::default();
    names.visit_block(&block);
    assert_eq!(names.0, ["x", "const", "t", "a", "m", "y", "done", "done"]);
}

#[test]
fn reaches_the_names_in_types() {
    // local v: Map<K, V>? = nil
    let fixture = Fixture::default();
    let synth = fixture.synth();
    let ty = synth.ty_optional(synth.ty_generic("Map", [synth.ty_named("K"), synth.ty_named("V")]));
    let local = synth.local_full([synth.attributed_name("v", Some(ty), None)], [synth.nil()]);
    let mut names = NameTexts::default();
    let block = synth.block([local]);
    names.visit_block(&block);
    assert_eq!(names.0, ["v", "Map", "K", "V"]);
}

/// The type of each node entered and left, in order, with `true` for an
/// entry.
#[derive(Default)]
struct Nodes(Vec<(AstType, bool)>);

impl<'ast> Visit<'ast> for Nodes {
    fn enter_node(&mut self, kind: AstKind<'ast>) {
        self.0.push((kind.ty(), true));
    }

    fn leave_node(&mut self, kind: AstKind<'ast>) {
        self.0.push((kind.ty(), false));
    }
}

#[test]
fn enters_and_leaves_each_node_around_what_it_holds() {
    // return -x
    let fixture = Fixture::default();
    let synth = fixture.synth();
    let negation = synth.unary_op(UnOp::Neg, synth.name_expr("x"));
    let block = synth.block([synth.return_([negation])]);
    let mut nodes = Nodes::default();
    nodes.visit_block(&block);
    assert_eq!(
        nodes.0,
        [
            (AstType::Block, true),
            (AstType::ReturnStatement, true),
            (AstType::UnaryOp, true),
            (AstType::Name, true),
            (AstType::Name, false),
            (AstType::UnaryOp, false),
            (AstType::ReturnStatement, false),
            (AstType::Block, false),
        ]
    );
}

#[test]
fn a_kind_spans_the_node_it_refers_to() {
    struct Spans(Vec<bool>);

    impl<'ast> Visit<'ast> for Spans {
        fn enter_node(&mut self, kind: AstKind<'ast>) {
            if let Some(binary_op) = kind.as_binary_op() {
                self.0.push(kind.span() == binary_op.span);
            }
        }
    }

    let fixture = Fixture::default();
    let synth = fixture.synth();
    let sum = synth.binary_op(synth.name_expr("a"), BinOp::Add, synth.name_expr("b"));
    let mut spans = Spans(Vec::new());
    spans.visit_expr(&sum);
    assert_eq!(spans.0, [true]);
}

#[test]
fn skipping_a_level_skips_what_it_holds() {
    struct Shallow(usize);

    impl<'ast> Visit<'ast> for Shallow {
        fn descend(&mut self) -> bool {
            false
        }

        fn visit_expr(&mut self, expr: &'ast Expression<'ast>) {
            self.0 = self.0.saturating_add(1);
            self.walk_expr(expr);
        }
    }

    // return (x): the block is a level, so nothing in it is reached.
    let fixture = Fixture::default();
    let synth = fixture.synth();
    let block = synth.block([synth.return_([synth.parenthesized(synth.name_expr("x"))])]);
    let mut shallow = Shallow(0);
    shallow.visit_block(&block);
    assert_eq!(shallow.0, 0);
}

#[test]
fn a_tree_is_send_and_sync() {
    fn assert_send_sync<T>()
    where
        T: Send + Sync,
    {
    }
    assert_send_sync::<Block<'_>>();
    assert_send_sync::<Expression<'_>>();
    assert_send_sync::<Type<'_>>();
    assert_send_sync::<AstKind<'_>>();
}
