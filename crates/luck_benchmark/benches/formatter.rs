use luck_allocator::Allocator;
use luck_benchmark::{BenchmarkId, Criterion, black_box, criterion_group, criterion_main};
use luck_benchmark_files::{test_files, test_projects};
use luck_formatter::{FormatOptions, format_parsed};

/// Each iteration formats a parse made once, since formatting only reads the
/// tree, into a second arena reset first, as a host formatting the tree it
/// holds does.
fn bench_formatter(criterion: &mut Criterion) {
    let mut group = criterion.benchmark_group("formatter");
    let options = FormatOptions::default();

    for file in test_files() {
        let id = BenchmarkId::from_parameter(file.file_name);
        let allocator = Allocator::new();
        let parse_result = file.parse(&allocator);
        let mut scratch = Allocator::new();
        group.bench_function(id, |bencher| {
            bencher.iter(|| {
                scratch.reset();
                format_parsed(&scratch, black_box(&parse_result), &options)
            });
        });
    }

    for project in test_projects() {
        let id = BenchmarkId::from_parameter(project.name);
        let allocator = Allocator::new();
        let parse_results = project.parse_files(&allocator);
        let mut scratch = Allocator::new();
        group.bench_function(id, |bencher| {
            bencher.iter(|| {
                for parse_result in &parse_results {
                    scratch.reset();
                    black_box(format_parsed(&scratch, black_box(parse_result), &options));
                }
            });
        });
    }

    group.finish();
}

criterion_group!(formatter, bench_formatter);
criterion_main!(formatter);
