use luck_benchmark::corpus::test_files;
use luck_benchmark::{BenchmarkId, Criterion, black_box, criterion_group, criterion_main};

fn bench_lexer(criterion: &mut Criterion) {
    let mut group = criterion.benchmark_group("lexer");

    for file in test_files() {
        let id = BenchmarkId::from_parameter(file.file_name);
        let source_text = file.source_text.as_str();
        let version = file.version;
        group.bench_function(id, |b| {
            b.iter(|| luck_lexer::lex(black_box(source_text), version));
        });
    }

    group.finish();
}

criterion_group!(lexer, bench_lexer);
criterion_main!(lexer);
