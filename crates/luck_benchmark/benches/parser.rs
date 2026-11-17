use luck_allocator::Allocator;
use luck_benchmark::{
    BenchmarkId, Criterion, black_box, criterion_group, criterion_main, test_files, test_projects,
};

fn bench_parser(criterion: &mut Criterion) {
    let mut group = criterion.benchmark_group("parser");

    for file in test_files() {
        let id = BenchmarkId::from_parameter(file.file_name);
        let source_text = file.source_text.as_str();
        let version = file.target.lua_version();
        // One arena reset between parses, as a tool reuses it across files.
        let mut allocator = Allocator::new();
        group.bench_function(id, |bencher| {
            bencher.iter(|| {
                allocator.reset();
                black_box(luck_parser::parse(&allocator, black_box(source_text), version));
            });
        });
    }

    for project in test_projects() {
        let id = BenchmarkId::from_parameter(project.name);
        let version = project.target.lua_version();
        let files = project.files;
        let mut allocator = Allocator::new();
        group.bench_function(id, |bencher| {
            bencher.iter(|| {
                for file in &files {
                    allocator.reset();
                    let source_text = black_box(file.source_text.as_str());
                    black_box(luck_parser::parse(&allocator, source_text, version));
                }
            });
        });
    }

    group.finish();
}

criterion_group!(parser, bench_parser);
criterion_main!(parser);
