use luck_benchmark::corpus::{test_files, test_projects};
use luck_benchmark::{BenchmarkId, Criterion, black_box, criterion_group, criterion_main};

fn bench_semantic(criterion: &mut Criterion) {
    let mut group = criterion.benchmark_group("semantic");

    for file in test_files() {
        let id = BenchmarkId::from_parameter(file.file_name);
        let version = file.version;
        // Parse outside the measured section so the bench isolates scope
        // analysis; `analyze` borrows the AST and does not mutate it.
        let parse_result = luck_parser::parse(&file.source_text, version);
        assert!(parse_result.errors.is_empty(), "{:?}", parse_result.errors);
        group.bench_function(id, |b| {
            b.iter(|| luck_semantic::analyze(black_box(&parse_result.block), version));
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
                    black_box(luck_semantic::analyze(
                        black_box(&parse_result.block),
                        version,
                    ));
                }
            });
        });
    }

    group.finish();
}

criterion_group!(semantic, bench_semantic);
criterion_main!(semantic);
