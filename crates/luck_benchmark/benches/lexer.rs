use luck_benchmark::{BenchmarkId, Criterion, black_box, criterion_group, criterion_main};
use luck_benchmark_files::{test_files, test_projects};

fn bench_lexer(criterion: &mut Criterion) {
    let mut group = criterion.benchmark_group("lexer");

    for file in test_files() {
        let id = BenchmarkId::from_parameter(file.file_name);
        let source_text = file.source_text.as_slice();
        group.bench_function(id, |bencher| {
            bencher.iter(|| luck_lexer::lex(black_box(source_text)));
        });
    }

    // Whole-project runs: many small-to-medium files per iteration, the
    // shape a `luck` invocation actually sees.
    for project in test_projects() {
        let id = BenchmarkId::from_parameter(project.name);
        group.bench_function(id, |bencher| {
            bencher.iter(|| {
                for file in &project.files {
                    black_box(luck_lexer::lex(black_box(file.source_text.as_slice())));
                }
            });
        });
    }

    group.finish();
}

criterion_group!(lexer, bench_lexer);
criterion_main!(lexer);
