use luck_allocator::Allocator;
use luck_benchmark::{BenchmarkId, Criterion, black_box, criterion_group, criterion_main};
use luck_benchmark_files::{test_files, test_projects};
use luck_parser::ParseResult;
use luck_semantic::{SemanticBuilder, SemanticBuilderReturn};

/// The semantic pass with every part it builds on request: the checker, the
/// node table, and the `require` sites.
fn build<'ast>(parse_result: &'ast ParseResult<'ast>) -> SemanticBuilderReturn<'ast> {
    SemanticBuilder::new()
        .with_counts(parse_result.counts)
        .with_check(true)
        .with_nodes(true)
        .with_requires(true)
        .build(&parse_result.chunk)
}

/// Each iteration analyzes a parse made once, since analysis only reads the
/// tree.
fn bench_semantic(criterion: &mut Criterion) {
    let mut group = criterion.benchmark_group("semantic");

    for file in test_files() {
        let id = BenchmarkId::from_parameter(file.file_name);
        let allocator = Allocator::new();
        let parse_result = file.parse(&allocator);
        group.bench_function(id, |bencher| {
            bencher.iter(|| build(black_box(&parse_result)));
        });
    }

    for project in test_projects() {
        let id = BenchmarkId::from_parameter(project.name);
        let allocator = Allocator::new();
        let parse_results = project.parse_files(&allocator);
        group.bench_function(id, |bencher| {
            bencher.iter(|| {
                for parse_result in &parse_results {
                    black_box(build(black_box(parse_result)));
                }
            });
        });
    }

    group.finish();
}

criterion_group!(semantic, bench_semantic);
criterion_main!(semantic);
