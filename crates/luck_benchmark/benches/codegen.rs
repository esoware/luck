use luck_benchmark::corpus::test_files;
use luck_benchmark::{BenchmarkId, Criterion, criterion_group, criterion_main};

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

    group.finish();
}

criterion_group!(codegen, bench_codegen);
criterion_main!(codegen);
