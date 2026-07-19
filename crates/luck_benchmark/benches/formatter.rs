use luck_benchmark::corpus::{test_files, test_projects};
use luck_benchmark::{BenchmarkId, Criterion, black_box, criterion_group, criterion_main};
use luck_formatter::{Comments, FormatOptions, format_block};

fn bench_formatter(criterion: &mut Criterion) {
    let mut group = criterion.benchmark_group("formatter");

    for file in test_files() {
        let id = BenchmarkId::from_parameter(file.file_name);
        let source_text = file.source_text.as_str();
        let options = FormatOptions::default();
        let parse_result = luck_parser::parse(source_text, file.version);
        assert!(parse_result.errors.is_empty(), "{:?}", parse_result.errors);
        group.bench_function(id, |b| {
            // `format_block` consumes its `Comments`, so rebuild them in
            // setup each iteration; the measured section is formatting only.
            b.iter_with_setup_wrapper(|runner| {
                let comments = Comments::from_source(&parse_result.comments, source_text);
                runner.run(|| format_block(&parse_result.block, comments, &options));
            });
        });
    }

    for project in test_projects() {
        let id = BenchmarkId::from_parameter(project.name);
        let version = project.version;
        let options = FormatOptions::default();
        let parses: Vec<_> = project
            .files
            .iter()
            .map(|(name, source_text)| {
                let parse_result = luck_parser::parse(source_text, version);
                assert!(
                    parse_result.errors.is_empty(),
                    "{name}: {:?}",
                    parse_result.errors
                );
                parse_result
            })
            .collect();
        group.bench_function(id, |b| {
            b.iter_with_setup_wrapper(|runner| {
                let per_file_comments: Vec<Comments> = parses
                    .iter()
                    .map(|parse_result| {
                        Comments::from_source(&parse_result.comments, &parse_result.source)
                    })
                    .collect();
                runner.run(|| {
                    for (parse_result, comments) in parses.iter().zip(per_file_comments) {
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
