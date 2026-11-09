use luck_benchmark::{
    BenchmarkId, Criterion, black_box, criterion_group, criterion_main, test_files, test_projects,
};

fn bench_semantic(criterion: &mut Criterion) {
    let mut group = criterion.benchmark_group("semantic");

    for file in test_files() {
        let id = BenchmarkId::from_parameter(file.file_name);
        let version = file.target.lua_version();
        // Parse outside the measured section so the bench isolates scope
        // analysis. `analyze` only borrows the AST, so one parse serves every
        // iteration.
        let parse_result = file.parse();
        group.bench_function(id, |bencher| {
            bencher.iter(|| luck_semantic::analyze(black_box(&parse_result.block), version));
        });
    }

    for project in test_projects() {
        let id = BenchmarkId::from_parameter(project.name);
        let version = project.target.lua_version();
        let parse_results = project.parse_files();
        group.bench_function(id, |bencher| {
            bencher.iter(|| {
                for parse_result in &parse_results {
                    black_box(luck_semantic::analyze(black_box(&parse_result.block), version));
                }
            });
        });
    }

    group.finish();
}

criterion_group!(semantic, bench_semantic);
criterion_main!(semantic);
