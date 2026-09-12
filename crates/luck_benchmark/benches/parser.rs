use luck_benchmark::corpus::{test_files, test_projects};
use luck_benchmark::{BenchmarkId, Criterion, black_box, criterion_group, criterion_main};

fn bench_parser(criterion: &mut Criterion) {
    let mut group = criterion.benchmark_group("parser");

    for file in test_files() {
        let id = BenchmarkId::from_parameter(file.file_name);
        let source_text = file.source_text.as_str();
        let version = file.version;
        group.bench_function(id, |b| {
            b.iter(|| luck_parser::parse(black_box(source_text), version));
        });
    }

    for project in test_projects() {
        let id = BenchmarkId::from_parameter(project.name);
        let version = project.version;
        let files = project.files;
        group.bench_function(id, |b| {
            b.iter(|| {
                for (_, source_text) in &files {
                    black_box(luck_parser::parse(black_box(source_text.as_str()), version));
                }
            });
        });
    }

    group.finish();
}

fn bench_parser_owned_source(criterion: &mut Criterion) {
    let mut group = criterion.benchmark_group("parser_owned_source");

    for file in test_files() {
        let id = BenchmarkId::from_parameter(file.file_name);
        let mut source_text = file.source_text;
        let version = file.version;
        group.bench_function(id, |b| {
            b.iter(|| {
                let parsed = black_box(luck_parser::parse(
                    black_box(std::mem::take(&mut source_text)),
                    version,
                ));
                source_text = parsed.source;
            });
        });
    }

    for project in test_projects() {
        let id = BenchmarkId::from_parameter(project.name);
        let version = project.version;
        let mut files = project.files;
        group.bench_function(id, |b| {
            b.iter(|| {
                for (_, source_text) in &mut files {
                    let parsed = black_box(luck_parser::parse(
                        black_box(std::mem::take(source_text)),
                        version,
                    ));
                    *source_text = parsed.source;
                }
            });
        });
    }

    group.finish();
}

criterion_group!(parser, bench_parser, bench_parser_owned_source);
criterion_main!(parser);
