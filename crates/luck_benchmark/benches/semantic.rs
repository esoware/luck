use luck_allocator::Allocator;
use luck_benchmark::{BenchmarkId, Criterion, black_box, criterion_group, criterion_main};
use luck_benchmark_files::{test_files, test_projects};

/// Each iteration analyzes a parse made once, since analysis only reads the
/// tree.
fn bench_semantic(criterion: &mut Criterion) {
    let mut group = criterion.benchmark_group("semantic");

    for file in test_files() {
        let id = BenchmarkId::from_parameter(file.file_name);
        let version = file.target.lua_version();
        let environment = file.target.stdlib_environment();
        let allocator = Allocator::new();
        let parse_result = file.parse(&allocator);
        group.bench_function(id, |bencher| {
            bencher.iter(|| {
                luck_semantic::analyze(
                    black_box(&parse_result.block),
                    parse_result.counts,
                    version,
                    environment,
                )
            });
        });
    }

    for project in test_projects() {
        let id = BenchmarkId::from_parameter(project.name);
        let version = project.target.lua_version();
        let environment = project.target.stdlib_environment();
        let allocator = Allocator::new();
        let parse_results = project.parse_files(&allocator);
        group.bench_function(id, |bencher| {
            bencher.iter(|| {
                for parse_result in &parse_results {
                    black_box(luck_semantic::analyze(
                        black_box(&parse_result.block),
                        parse_result.counts,
                        version,
                        environment,
                    ));
                }
            });
        });
    }

    group.finish();
}

criterion_group!(semantic, bench_semantic);
criterion_main!(semantic);
