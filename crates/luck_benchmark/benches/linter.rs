#![expect(
    clippy::missing_panics_doc,
    reason = "a test panics to fail, so a Panics section says nothing"
)]

use luck_benchmark::{
    BenchmarkId, Criterion, black_box, criterion_group, criterion_main, test_files, test_projects,
};
use luck_linter::{CategoryLevels, LintConfig, RuleLevel};
use rayon::ThreadPoolBuilder;

fn bench_linter(criterion: &mut Criterion) {
    // A pool of only the bench thread runs the large-file joins and chunks
    // inline, so no work waits on another thread or depends on the runner's
    // core count.
    let pool = ThreadPoolBuilder::new().num_threads(1).use_current_thread().build_global();
    assert!(pool.is_ok(), "{:?}", pool.err());

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
        let parse_result = file.parse();
        let semantic = luck_semantic::analyze_with_environment(
            &parse_result.block,
            file.target.lua_version(),
            file.target.stdlib_environment(),
        );
        group.bench_function(id, |bencher| {
            bencher
                .iter(|| luck_linter::lint_analyzed(black_box(&parse_result), &semantic, &config));
        });
    }

    for project in test_projects() {
        let id = BenchmarkId::from_parameter(project.name);
        let version = project.target.lua_version();
        let environment = project.target.stdlib_environment();
        let analyzed_files: Vec<_> = project
            .parse_files()
            .into_iter()
            .map(|parse_result| {
                let semantic = luck_semantic::analyze_with_environment(
                    &parse_result.block,
                    version,
                    environment,
                );
                (parse_result, semantic)
            })
            .collect();
        group.bench_function(id, |bencher| {
            bencher.iter(|| {
                for (parse_result, semantic) in &analyzed_files {
                    black_box(luck_linter::lint_analyzed(
                        black_box(parse_result),
                        semantic,
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
