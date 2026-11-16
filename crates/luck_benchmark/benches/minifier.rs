use luck_benchmark::{
    BatchSize, BenchmarkId, Criterion, criterion_group, criterion_main, test_files, test_projects,
};
use luck_minifier::TransformConfig;

fn bench_minifier(criterion: &mut Criterion) {
    let mut group = criterion.benchmark_group("minifier");
    let config = TransformConfig::default();

    for file in test_files() {
        let id = BenchmarkId::from_parameter(file.file_name);
        let version = file.target.lua_version();
        let parsed_block = file.parse().block;
        group.bench_function(id, |bencher| {
            bencher.iter_batched(
                || parsed_block.clone(),
                |block| luck_minifier::minify_block(block, version, &config),
                BatchSize::LargeInput,
            );
        });
    }

    for project in test_projects() {
        let id = BenchmarkId::from_parameter(project.name);
        let version = project.target.lua_version();
        let parsed_blocks: Vec<_> =
            project.parse_files().into_iter().map(|parse_result| parse_result.block).collect();
        group.bench_function(id, |bencher| {
            bencher.iter_batched(
                || (parsed_blocks.clone(), Vec::with_capacity(parsed_blocks.len())),
                |(blocks, mut outputs)| {
                    for block in blocks {
                        outputs.push(luck_minifier::minify_block(block, version, &config));
                    }
                    outputs
                },
                BatchSize::LargeInput,
            );
        });
    }

    group.finish();
}

criterion_group!(minifier, bench_minifier);
criterion_main!(minifier);
