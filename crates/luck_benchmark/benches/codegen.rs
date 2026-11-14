use luck_benchmark::{
    BenchmarkId, Criterion, black_box, criterion_group, criterion_main, test_files, test_projects,
};

fn bench_codegen(criterion: &mut Criterion) {
    let mut group = criterion.benchmark_group("codegen");

    for file in test_files() {
        let id = BenchmarkId::from_parameter(file.file_name);
        let source_text = file.source_text.as_str();
        let parse_result = file.parse();
        group.bench_function(id, |bencher| {
            bencher.iter_with_large_drop(|| {
                luck_codegen::compact_with_capacity(&parse_result.block, source_text.len())
            });
        });
    }

    for project in test_projects() {
        let id = BenchmarkId::from_parameter(project.name);
        let parse_results = project.parse_files();
        group.bench_function(id, |bencher| {
            bencher.iter(|| {
                for parse_result in &parse_results {
                    let _unused = black_box(luck_codegen::compact_with_capacity(
                        black_box(&parse_result.block),
                        parse_result.source.len(),
                    ));
                }
            });
        });
    }

    group.finish();
}

criterion_group!(codegen, bench_codegen);
criterion_main!(codegen);
