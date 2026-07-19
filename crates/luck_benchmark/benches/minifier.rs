use luck_benchmark::corpus::test_files;
use luck_benchmark::{BenchmarkId, Criterion, black_box, criterion_group, criterion_main};
use luck_core::transform_config::TransformConfig;

fn bench_minifier(criterion: &mut Criterion) {
    let mut group = criterion.benchmark_group("minifier");

    for file in test_files() {
        let id = BenchmarkId::from_parameter(file.file_name);
        let source_text = file.source_text.as_str();
        let target = file.target;
        let config = TransformConfig::default();
        // `minify` has no parsed-AST entry point, so the measured section
        // includes the initial parse; the transform fixpoint dominates.
        group.bench_function(id, |b| {
            b.iter(|| {
                luck_minifier::minify(black_box(source_text), target, &config, "bench.lua")
                    .expect("bench corpus must minify")
            });
        });
    }

    group.finish();
}

criterion_group!(minifier, bench_minifier);
criterion_main!(minifier);
