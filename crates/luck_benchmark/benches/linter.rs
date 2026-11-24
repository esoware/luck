use luck_allocator::Allocator;
use luck_benchmark::{BenchmarkId, Criterion, black_box, criterion_group, criterion_main};
use luck_benchmark_files::{test_files, test_projects};
use luck_linter::{CategoryLevels, FixKind, LintConfig, RuleLevel};

/// Each iteration analyzes and lints a parse made once, since the linter's
/// node table is built in the analysis walk.
fn bench_linter(criterion: &mut Criterion) {
    let config = LintConfig {
        categories: CategoryLevels {
            correctness: Some(RuleLevel::Warn),
            suspicious: Some(RuleLevel::Warn),
            style: Some(RuleLevel::Warn),
            performance: Some(RuleLevel::Warn),
        },
        ..LintConfig::default()
    };
    let mut group = criterion.benchmark_group("linter");

    for file in test_files() {
        let id = BenchmarkId::from_parameter(file.file_name);
        let allocator = Allocator::new();
        let parse_result = file.parse(&allocator);
        group.bench_function(id, |bencher| {
            bencher.iter(|| {
                luck_linter::lint_parsed(
                    black_box(&parse_result),
                    file.target,
                    &config,
                    FixKind::empty(),
                )
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
                    black_box(luck_linter::lint_parsed(
                        black_box(parse_result),
                        project.target,
                        &config,
                        FixKind::empty(),
                    ));
                }
            });
        });
    }

    group.finish();
}

criterion_group!(linter, bench_linter);
criterion_main!(linter);
