use luck_allocator::Allocator;
use luck_benchmark::{BenchmarkId, Criterion, black_box, criterion_group, criterion_main};
use luck_benchmark_files::{test_files, test_projects};

/// Each iteration prints a parse made once, since printing only reads the
/// tree.
fn bench_codegen(criterion: &mut Criterion) {
    let mut group = criterion.benchmark_group("codegen");

    for file in test_files() {
        let id = BenchmarkId::from_parameter(file.file_name);
        let allocator = Allocator::new();
        let parse_result = file.parse(&allocator);
        group.bench_function(id, |bencher| {
            bencher.iter_with_large_drop(|| {
                luck_codegen::compact_with_capacity(&parse_result.chunk, parse_result.source.len())
            });
        });
    }

    for project in test_projects() {
        let id = BenchmarkId::from_parameter(project.name);
        let allocator = Allocator::new();
        let parse_results = project.parse_files(&allocator);
        group.bench_function(id, |bencher| {
            bencher.iter(|| {
                for parse_result in &parse_results {
                    black_box(luck_codegen::compact_with_capacity(
                        black_box(&parse_result.chunk),
                        parse_result.source.len(),
                    ));
                }
            });
        });
    }

    group.finish();
}

criterion_group!(codegen, bench_codegen);
criterion_main!(codegen);
