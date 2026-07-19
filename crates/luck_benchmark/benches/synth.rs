//! Programmatic AST construction (`luck_ast::synth`)

use luck_ast::shared::Block;
use luck_ast::stmt::Statement;
use luck_ast::synth::Synth;
use luck_benchmark::{BenchmarkId, Criterion, black_box, criterion_group, criterion_main};
use luck_formatter::{Comments, FormatOptions, format_block};
use luck_token::{BinOp, UnOp};

const MODULE_COUNT: usize = 150;

/// One OOP-style module: class table, constructor, two methods with loops
/// and conditionals.
fn build_module(synth: &Synth, module_index: usize) -> Vec<Statement> {
    let class = format!("Module{module_index}");

    let constructor_body = synth.block(
        vec![
            synth.local(
                &["self"],
                vec![synth.call(
                    synth.name_expr("setmetatable"),
                    vec![synth.table(vec![]), synth.name_expr(&class)],
                )],
            ),
            synth.assign(
                vec![synth.field_var(synth.name_expr("self"), "size")],
                vec![synth.binop(synth.name_expr("size"), BinOp::Or, synth.number_int(0))],
            ),
            synth.assign(
                vec![synth.field_var(synth.name_expr("self"), "items")],
                vec![synth.table(vec![])],
            ),
        ],
        Some(synth.return_(vec![synth.name_expr("self")])),
    );

    let push_body = synth.block(
        vec![synth.assign(
            vec![synth.index_var(
                synth.field(synth.name_expr("self"), "items"),
                synth.binop(
                    synth.unop(UnOp::Len, synth.field(synth.name_expr("self"), "items")),
                    BinOp::Add,
                    synth.number_int(1),
                ),
            )],
            vec![synth.name_expr("value")],
        )],
        Some(synth.return_(vec![synth.name_expr("self")])),
    );

    let loop_body = synth.block(
        vec![synth.assign(
            vec![synth.name_var("sum")],
            vec![synth.binop(
                synth.name_expr("sum"),
                BinOp::Add,
                synth.var_expr(synth.index_var(
                    synth.field(synth.name_expr("self"), "items"),
                    synth.name_expr("index"),
                )),
            )],
        )],
        None,
    );
    let total_body = synth.block(
        vec![
            synth.local(&["sum"], vec![synth.number_int(0)]),
            synth.numeric_for(
                synth.param("index"),
                synth.number_int(1),
                synth.unop(UnOp::Len, synth.field(synth.name_expr("self"), "items")),
                None,
                loop_body,
            ),
            synth.if_(
                synth.binop(
                    synth.name_expr("sum"),
                    BinOp::Gt,
                    synth.field(synth.name_expr("self"), "size"),
                ),
                synth.block(
                    vec![synth.call_stmt(synth.method_call_string(
                        synth.name_expr("self"),
                        "report",
                        "overflow",
                    ))],
                    None,
                ),
                vec![],
                None,
            ),
        ],
        Some(synth.return_(vec![synth.name_expr("sum")])),
    );

    vec![
        synth.local(&[&class], vec![synth.table(vec![])]),
        synth.assign(
            vec![synth.field_var(synth.name_expr(&class), "__index")],
            vec![synth.name_expr(&class)],
        ),
        synth.function_decl(&[&class, "new"], None, &["size"], constructor_body),
        synth.function_decl(&[&class], Some("push"), &["value"], push_body),
        synth.function_decl(&[&class], Some("total"), &[], total_body),
    ]
}

fn build_program(module_count: usize) -> Block {
    let synth = Synth::new();
    let mut stmts = Vec::new();
    for module_index in 0..module_count {
        stmts.extend(build_module(&synth, module_index));
    }
    let last = synth.return_(vec![synth.name_expr("Module0")]);
    synth.block(stmts, Some(last))
}

fn bench_synth(criterion: &mut Criterion) {
    let mut group = criterion.benchmark_group("synth");

    group.bench_function(BenchmarkId::from_parameter("build"), |b| {
        b.iter(|| black_box(build_program(black_box(MODULE_COUNT))));
    });

    let block = build_program(MODULE_COUNT);

    // Source-less emit: token-carried text only (hard invariant 11).
    group.bench_function(BenchmarkId::from_parameter("codegen"), |b| {
        b.iter_with_large_drop(|| luck_codegen::compact(black_box(&block), ""));
    });

    let options = FormatOptions::default();
    group.bench_function(BenchmarkId::from_parameter("format"), |b| {
        b.iter_with_setup_wrapper(|runner| {
            let comments = Comments::none();
            runner.run(|| format_block(black_box(&block), comments, &options));
        });
    });

    group.finish();
}

criterion_group!(synth, bench_synth);
criterion_main!(synth);
