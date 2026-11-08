use core::mem;

use luck_benchmark::{
    BenchmarkId, Criterion, black_box, criterion_group, criterion_main, test_files, test_projects,
};

fn bench_parser(criterion: &mut Criterion) {
    let mut group = criterion.benchmark_group("parser");

    for file in test_files() {
        let id = BenchmarkId::from_parameter(file.file_name);
        let source_text = file.source_text.as_str();
        let version = file.target.lua_version();
        group.bench_function(id, |bencher| {
            bencher.iter(|| luck_parser::parse(black_box(source_text), version));
        });
    }

    for project in test_projects() {
        let id = BenchmarkId::from_parameter(project.name);
        let version = project.target.lua_version();
        let files = project.files;
        group.bench_function(id, |bencher| {
            bencher.iter(|| {
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
        let version = file.target.lua_version();
        group.bench_function(id, |bencher| {
            bencher.iter(|| {
                let parsed =
                    black_box(luck_parser::parse(black_box(mem::take(&mut source_text)), version));
                source_text = parsed.source;
            });
        });
    }

    for project in test_projects() {
        let id = BenchmarkId::from_parameter(project.name);
        let version = project.target.lua_version();
        let mut files = project.files;
        group.bench_function(id, |bencher| {
            bencher.iter(|| {
                for (_, source_text) in &mut files {
                    let parsed =
                        black_box(luck_parser::parse(black_box(mem::take(source_text)), version));
                    *source_text = parsed.source;
                }
            });
        });
    }

    group.finish();
}

criterion_group!(parser, bench_parser, bench_parser_owned_source);
criterion_main!(parser);
