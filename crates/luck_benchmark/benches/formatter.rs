use luck_benchmark::{
    BenchmarkId, Criterion, black_box, criterion_group, criterion_main, test_files, test_projects,
};
use luck_formatter::{Comments, FormatOptions, format_block};

fn bench_formatter(criterion: &mut Criterion) {
    let mut group = criterion.benchmark_group("formatter");

    for file in test_files() {
        let id = BenchmarkId::from_parameter(file.file_name);
        let source_text = file.source_text.as_str();
        let version = file.target.lua_version();
        let options = FormatOptions::default();
        let parse_result = file.parse();
        group.bench_function(id, |bencher| {
            // `format_block` consumes its `Comments`, so rebuild them in
            // setup each iteration; the measured section is formatting only.
            bencher.iter_with_setup_wrapper(|runner| {
                let comments = Comments::from_source(&parse_result.comments, source_text, version);
                runner.run(|| format_block(&parse_result.block, comments, &options));
            });
        });
    }

    for project in test_projects() {
        let id = BenchmarkId::from_parameter(project.name);
        let version = project.target.lua_version();
        let options = FormatOptions::default();
        let parse_results = project.parse_files();
        group.bench_function(id, |bencher| {
            bencher.iter_with_setup_wrapper(|runner| {
                let per_file_comments: Vec<Comments> = parse_results
                    .iter()
                    .map(|parse_result| {
                        Comments::from_source(&parse_result.comments, &parse_result.source, version)
                    })
                    .collect();
                runner.run(|| {
                    for (parse_result, comments) in parse_results.iter().zip(per_file_comments) {
                        black_box(format_block(&parse_result.block, comments, &options));
                    }
                });
            });
        });
    }

    group.finish();
}

criterion_group!(formatter, bench_formatter);
criterion_main!(formatter);
