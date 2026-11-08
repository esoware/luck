#![expect(
    clippy::missing_panics_doc,
    reason = "a test panics to fail, so a Panics section says nothing"
)]

use luck_benchmark::{
    BenchmarkId, Criterion, black_box, criterion_group, criterion_main, test_files, test_projects,
};
use luck_core::TransformConfig;

fn bench_minifier(criterion: &mut Criterion) {
    let mut group = criterion.benchmark_group("minifier");

    for file in test_files() {
        let id = BenchmarkId::from_parameter(file.file_name);
        let source_text = file.source_text.as_str();
        let version = file.target.lua_version();
        let config = TransformConfig::default();
        let minify =
            || luck_minifier::minify(black_box(source_text), version, &config, "bench.lua");
        let minified = minify();
        assert!(minified.is_ok(), "{:?}", minified.err());
        // `minify` has no parsed-AST entry point, so the measured section
        // includes the initial parse; the transform fixpoint dominates.
        group.bench_function(id, |bencher| {
            bencher.iter(minify);
        });
    }

    for project in test_projects() {
        let id = BenchmarkId::from_parameter(project.name);
        let version = project.target.lua_version();
        let config = TransformConfig::default();
        let files = project.files;
        for (name, source_text) in &files {
            let minified = luck_minifier::minify(source_text.as_str(), version, &config, name);
            assert!(minified.is_ok(), "{name}: {:?}", minified.err());
        }
        group.bench_function(id, |bencher| {
            bencher.iter(|| {
                for (name, source_text) in &files {
                    let _unused = black_box(luck_minifier::minify(
                        black_box(source_text.as_str()),
                        version,
                        &config,
                        name,
                    ));
                }
            });
        });
    }

    group.finish();
}

criterion_group!(minifier, bench_minifier);
criterion_main!(minifier);
