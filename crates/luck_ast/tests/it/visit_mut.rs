use luck_ast::{
    AstType, Block, Expression, FunctionArgs, FunctionBody, Name, NameIndex, Statement,
    SuffixedExpression, Synth, TableConstructor, Visit, VisitMut,
};
use luck_token::BinOp;

use crate::common::Fixture;

struct NilToFalse;

impl<'ast> VisitMut<'ast> for NilToFalse {
    fn visit_expr(&mut self, expr: &mut Expression<'ast>) {
        self.walk_expr(expr);
        if let Expression::Nil(span) = *expr {
            *expr = Expression::False(span);
        }
    }
}

#[derive(Debug, Default, PartialEq, Eq)]
struct NilFalseCounts {
    nils: usize,
    falses: usize,
}

impl<'ast> Visit<'ast> for NilFalseCounts {
    fn visit_expr(&mut self, expr: &'ast Expression<'ast>) {
        if matches!(expr, Expression::Nil(_)) {
            self.nils += 1;
        }
        if matches!(expr, Expression::False(_)) {
            self.falses += 1;
        }
        self.walk_expr(expr);
    }
}

fn rewrite_counts<'ast, Stmts>(synth: Synth<'ast>, stmts: Stmts) -> NilFalseCounts
where
    Stmts: IntoIterator<Item = Statement<'ast>>,
    Stmts::IntoIter: ExactSizeIterator,
{
    let mut block = synth.block(stmts);
    NilToFalse.visit_block(&mut block);
    let mut counts = NilFalseCounts::default();
    counts.visit_block(&block);
    counts
}

const fn all_false(falses: usize) -> NilFalseCounts {
    NilFalseCounts { nils: 0, falses }
}

#[test]
fn rewrites_reach_local_initializers() {
    // local x = nil
    let fixture = Fixture::default();
    let synth = fixture.synth();
    assert_eq!(rewrite_counts(synth, [synth.local(&["x"], [synth.nil()])]), all_false(1));
}

#[test]
fn rewrites_reach_table_fields() {
    // local t = {nil, nil}
    let fixture = Fixture::default();
    let synth = fixture.synth();
    let table = synth.array([synth.nil(), synth.nil()]);
    assert_eq!(rewrite_counts(synth, [synth.local(&["t"], [table])]), all_false(2));
}

#[test]
fn rewrites_reach_function_returns() {
    // local function f() return nil end
    let fixture = Fixture::default();
    let synth = fixture.synth();
    let body = synth.block([synth.return_([synth.nil()])]);
    assert_eq!(rewrite_counts(synth, [synth.local_function("f", &[], body)]), all_false(1));
}

#[test]
fn rewrites_reach_if_conditions_and_branches() {
    // if nil then x = nil else y = nil end
    let fixture = Fixture::default();
    let synth = fixture.synth();
    let then_block = synth.block([synth.assign([synth.name_var("x")], [synth.nil()])]);
    let else_block = synth.block([synth.assign([synth.name_var("y")], [synth.nil()])]);
    let if_stmt = synth.if_(synth.nil(), then_block, [], Some(else_block));
    assert_eq!(rewrite_counts(synth, [if_stmt]), all_false(3));
}

#[test]
fn rewrites_reach_loop_conditions_bounds_and_steps() {
    // while nil do end; repeat until nil; for i = nil, nil, nil do end
    let fixture = Fixture::default();
    let synth = fixture.synth();
    let numeric_for = synth.numeric_for(
        synth.param("i"),
        synth.nil(),
        synth.nil(),
        Some(synth.nil()),
        synth.block([]),
    );
    let stmts = [
        synth.while_(synth.nil(), synth.block([])),
        synth.repeat_(synth.block([]), synth.nil()),
        numeric_for,
    ];
    assert_eq!(rewrite_counts(synth, stmts), all_false(5));
}

#[test]
fn the_default_walk_leaves_the_tree_as_it_was() {
    struct Identity;
    impl VisitMut<'_> for Identity {}

    // local x = 1 + 2; a.b, c = x, x, built twice from one span counter
    // start, so the two trees are equal before the walk.
    fn build(fixture: &Fixture) -> Block<'_> {
        let synth = fixture.synth();
        let sum = synth.binary_op(synth.number_i64(1), BinOp::Add, synth.number_i64(2));
        let targets = [synth.field_var(synth.name_expr("a"), "b"), synth.name_var("c")];
        let values = [synth.name_expr("x"), synth.name_expr("x")];
        synth.block([synth.local(&["x"], [sum]), synth.assign(targets, values)])
    }

    let (walked_fixture, untouched_fixture) = (Fixture::default(), Fixture::default());
    let mut walked = build(&walked_fixture);
    Identity.visit_block(&mut walked);
    assert_eq!(walked, build(&untouched_fixture));
}

#[test]
fn a_rewrite_can_restructure_through_take() {
    // return (f()), with the parentheses removed in place: return f().
    struct Unparenthesize;

    impl<'ast> VisitMut<'ast> for Unparenthesize {
        fn visit_expr(&mut self, expr: &mut Expression<'ast>) {
            self.walk_expr(expr);
            if let Expression::Parenthesized(parenthesized) = expr {
                let inner = parenthesized.expr.take();
                *expr = inner;
            }
        }
    }

    let fixture = Fixture::default();
    let synth = fixture.synth();
    let truncated = synth.single_value(synth.call(synth.name_expr("f"), []));
    let mut block = synth.block([synth.return_([truncated])]);
    Unparenthesize.visit_block(&mut block);
    let [Statement::Return(returned)] = &*block.stmts else {
        panic!("expected a return");
    };
    assert!(matches!(*returned.exprs, [Expression::Suffixed(_)]));
}

#[test]
fn a_rewrite_reaches_every_name() {
    struct Renumber;

    impl<'ast> VisitMut<'ast> for Renumber {
        fn visit_name(&mut self, name: &mut Name<'ast>) {
            name.index = Some(NameIndex::new(7));
        }
    }

    #[derive(Default)]
    struct Indexes(Vec<Option<u32>>);

    impl<'ast> Visit<'ast> for Indexes {
        fn visit_name(&mut self, name: &'ast Name<'ast>) {
            self.0.push(name.index.map(NameIndex::get));
        }
    }

    // local f = t.a:m(x)
    let fixture = Fixture::default();
    let synth = fixture.synth();
    let call =
        synth.method_call(synth.field(synth.name_expr("t"), "a"), "m", [synth.name_expr("x")]);
    let mut block = synth.block([synth.local(&["f"], [call])]);
    Renumber.visit_block(&mut block);
    let mut indexes = Indexes::default();
    indexes.visit_block(&block);
    assert_eq!(indexes.0, [Some(7); 5]);
}

#[test]
fn hooks_fire_in_stmt_and_expr_positions() {
    #[derive(Debug, Default, PartialEq, Eq)]
    struct HookCounts {
        chains: usize,
        args: usize,
        tables: usize,
        bodies: usize,
        blocks: usize,
    }

    impl<'ast> VisitMut<'ast> for HookCounts {
        fn enter_node(&mut self, kind: AstType) {
            if kind == AstType::Block {
                self.blocks += 1;
            }
        }

        fn visit_suffixed_expr(&mut self, suffixed_expr: &mut SuffixedExpression<'ast>) {
            self.chains += 1;
            self.walk_suffixed_expr(suffixed_expr);
        }

        fn visit_function_args(&mut self, function_args: &mut FunctionArgs<'ast>) {
            self.args += 1;
            self.walk_function_args(function_args);
        }

        fn visit_table_constructor(&mut self, table_constructor: &mut TableConstructor<'ast>) {
            self.tables += 1;
            self.walk_table_constructor(table_constructor);
        }

        fn visit_function_body(&mut self, function_body: &mut FunctionBody<'ast>) {
            self.bodies += 1;
            self.walk_function_body(function_body);
        }
    }

    // f{}(function() end); local t = {}
    let fixture = Fixture::default();
    let synth = fixture.synth();
    let table_call = synth.call_table(synth.name_expr("f"), []);
    let function = synth.function_def(&[], synth.block([]));
    let mut block = synth.block([
        synth.call_stmt(synth.call(table_call, [function])),
        synth.local(&["t"], [synth.table([])]),
    ]);
    let mut counts = HookCounts::default();
    counts.visit_block(&mut block);
    assert_eq!(counts, HookCounts { chains: 1, args: 2, tables: 2, bodies: 1, blocks: 2 });
}
