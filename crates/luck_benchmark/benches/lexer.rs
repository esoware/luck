use luck_benchmark::corpus::{test_files, test_projects};
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

    // Whole-project runs: many small-to-medium files per iteration, the
    // shape a `luck` invocation actually sees.
    for project in test_projects() {
        let id = BenchmarkId::from_parameter(project.name);
        let version = project.version;
        let files = project.files;
        group.bench_function(id, |b| {
            b.iter(|| {
                for (_, source_text) in &files {
                    black_box(luck_lexer::lex(black_box(source_text.as_str()), version));
                }
            });
        });
    }

    group.finish();
}

criterion_group!(lexer, bench_lexer);
criterion_main!(lexer);
