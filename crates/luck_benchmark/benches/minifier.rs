use luck_allocator::Allocator;
use luck_benchmark::{BenchmarkId, Criterion, criterion_group, criterion_main};
use luck_benchmark_files::{test_files, test_projects};
use luck_minifier::TransformConfig;

/// Minifying rewrites the tree in place, so each iteration resets the arena
/// and parses afresh into it, and only the minification that follows, in the
/// same arena, is timed.
fn bench_minifier(criterion: &mut Criterion) {
    let mut group = criterion.benchmark_group("minifier");
    let config = TransformConfig::default();

    for file in test_files() {
        let id = BenchmarkId::from_parameter(file.file_name);
        let version = file.target.lua_version();
        let mut allocator = Allocator::new();
        group.bench_function(id, |bencher| {
            bencher.iter_with_setup_wrapper(|runner| {
                allocator.reset();
                let mut parse_result = file.parse(&allocator);
                let _minified = runner.run(|| {
                    luck_minifier::minify_block(
                        &allocator,
                        &mut parse_result.block,
                        version,
                        &config,
                    )
                });
            });
        });
    }

    for project in test_projects() {
        let id = BenchmarkId::from_parameter(project.name);
        let version = project.target.lua_version();
        let mut allocator = Allocator::new();
        group.bench_function(id, |bencher| {
            bencher.iter_with_setup_wrapper(|runner| {
                allocator.reset();
                let mut parse_results = project.parse_files(&allocator);
                runner.run(|| {
                    parse_results
                        .iter_mut()
                        .map(|parse_result| {
                            luck_minifier::minify_block(
                                &allocator,
                                &mut parse_result.block,
                                version,
                                &config,
                            )
                        })
                        .collect::<Vec<_>>()
                });
            });
        });
    }

    group.finish();
}

criterion_group!(minifier, bench_minifier);
criterion_main!(minifier);
