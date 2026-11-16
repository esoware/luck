use luck_benchmark::{
    BenchmarkId, Criterion, black_box, criterion_group, criterion_main, test_files, test_projects,
};
use luck_formatter::{FormatOptions, format_parsed};

fn bench_formatter(criterion: &mut Criterion) {
    let mut group = criterion.benchmark_group("formatter");
    let options = FormatOptions::default();

    for file in test_files() {
        let id = BenchmarkId::from_parameter(file.file_name);
        let version = file.target.lua_version();
        let parse_result = file.parse();
        group.bench_function(id, |bencher| {
            bencher.iter(|| format_parsed(black_box(&parse_result), version, &options));
        });
    }

    for project in test_projects() {
        let id = BenchmarkId::from_parameter(project.name);
        let version = project.target.lua_version();
        let parse_results = project.parse_files();
        group.bench_function(id, |bencher| {
            bencher.iter(|| {
                for parse_result in &parse_results {
                    black_box(format_parsed(black_box(parse_result), version, &options));
                }
            });
        });
    }

    group.finish();
}

criterion_group!(formatter, bench_formatter);
criterion_main!(formatter);
