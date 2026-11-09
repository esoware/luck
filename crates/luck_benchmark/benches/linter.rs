use luck_benchmark::{
    BenchmarkId, Criterion, black_box, criterion_group, criterion_main, test_files, test_projects,
};
use luck_linter::LintConfig;

fn bench_linter(criterion: &mut Criterion) {
    let mut group = criterion.benchmark_group("linter");

    for file in test_files() {
        let id = BenchmarkId::from_parameter(file.file_name);
        let version = file.target.lua_version();
        let environment = file.target.stdlib_environment();
        let config = LintConfig::default();
        // Parse outside the measured section, so the bench measures semantic
        // analysis and rule dispatch only.
        let parse_result = file.parse();
        group.bench_function(id, |bencher| {
            bencher.iter(|| {
                luck_linter::lint_parsed(black_box(&parse_result), version, environment, &config)
            });
        });
    }

    for project in test_projects() {
        let id = BenchmarkId::from_parameter(project.name);
        let version = project.target.lua_version();
        let environment = project.target.stdlib_environment();
        let config = LintConfig::default();
        let parse_results = project.parse_files();
        group.bench_function(id, |bencher| {
            bencher.iter(|| {
                for parse_result in &parse_results {
                    black_box(luck_linter::lint_parsed(
                        black_box(parse_result),
                        version,
                        environment,
                        &config,
                    ));
                }
            });
        });
    }

    group.finish();
}

criterion_group!(linter, bench_linter);
criterion_main!(linter);
