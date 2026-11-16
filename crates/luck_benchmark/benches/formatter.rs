#![expect(
    clippy::missing_panics_doc,
    reason = "a test panics to fail, so a Panics section says nothing"
)]

use luck_benchmark::{
    BenchmarkId, Criterion, ProjectFile, black_box, criterion_group, criterion_main, test_files,
    test_projects,
};
use luck_formatter::{FormatOptions, format};

fn bench_formatter(criterion: &mut Criterion) {
    let mut group = criterion.benchmark_group("formatter");

    for file in test_files() {
        let id = BenchmarkId::from_parameter(file.file_name);
        let source_text = file.source_text.as_str();
        let version = file.target.lua_version();
        let options = FormatOptions::default();
        let formatted = format(source_text, version, &options);
        assert!(formatted.errors.is_empty(), "{:?}", formatted.errors);
        group.bench_function(id, |bencher| {
            bencher.iter(|| format(black_box(source_text), version, &options));
        });
    }

    for project in test_projects() {
        let id = BenchmarkId::from_parameter(project.name);
        let version = project.target.lua_version();
        let options = FormatOptions::default();
        let files = project.files;
        for ProjectFile { relative_path, source_text } in &files {
            let formatted = format(source_text, version, &options);
            assert!(formatted.errors.is_empty(), "{relative_path}: {:?}", formatted.errors);
        }
        group.bench_function(id, |bencher| {
            bencher.iter(|| {
                for ProjectFile { source_text, .. } in &files {
                    black_box(format(black_box(source_text), version, &options));
                }
            });
        });
    }

    group.finish();
}

criterion_group!(formatter, bench_formatter);
criterion_main!(formatter);
