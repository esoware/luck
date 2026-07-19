use luck_benchmark::corpus::{test_files, test_projects};
use luck_benchmark::{BenchmarkId, Criterion, black_box, criterion_group, criterion_main};

fn bench_codegen(criterion: &mut Criterion) {
    let mut group = criterion.benchmark_group("codegen");

    for file in test_files() {
        let id = BenchmarkId::from_parameter(file.file_name);
        let source_text = file.source_text.as_str();
        let parse_result = luck_parser::parse(source_text, file.version);
        assert!(parse_result.errors.is_empty(), "{:?}", parse_result.errors);
        group.bench_function(id, |b| {
            b.iter_with_large_drop(|| luck_codegen::compact(&parse_result.block, source_text));
        });
    }

    for project in test_projects() {
        let id = BenchmarkId::from_parameter(project.name);
        let version = project.version;
        let parses: Vec<_> = project
            .files
            .iter()
            .map(|(name, source_text)| {
                let parse_result = luck_parser::parse(source_text, version);
                assert!(
                    parse_result.errors.is_empty(),
                    "{name}: {:?}",
                    parse_result.errors
                );
                parse_result
            })
            .collect();
        group.bench_function(id, |b| {
            b.iter(|| {
                for parse_result in &parses {
                    black_box(luck_codegen::compact(
                        black_box(&parse_result.block),
                        &parse_result.source,
                    ));
                }
            });
        });
    }

    group.finish();
}

criterion_group!(codegen, bench_codegen);
criterion_main!(codegen);
