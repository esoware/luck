use luck_benchmark::corpus::test_files;
use luck_benchmark::{BenchmarkId, Criterion, black_box, criterion_group, criterion_main};
use luck_linter::LintConfig;

fn bench_linter(criterion: &mut Criterion) {
    let mut group = criterion.benchmark_group("linter");

    for file in test_files() {
        let id = BenchmarkId::from_parameter(file.file_name);
        let version = file.version;
        let environment = file.target.stdlib_environment();
        let config = LintConfig::default();
        // Parse outside the measured section; `lint_parsed` is the
        // parse-once entry point long-lived hosts use, so the bench
        // measures semantic analysis + rule dispatch only.
        let parse_result = luck_parser::parse(&file.source_text, version);
        assert!(parse_result.errors.is_empty(), "{:?}", parse_result.errors);
        group.bench_function(id, |b| {
            b.iter(|| {
                luck_linter::lint_parsed(black_box(&parse_result), version, environment, &config)
            });
        });
    }

    group.finish();
}

criterion_group!(linter, bench_linter);
criterion_main!(linter);
