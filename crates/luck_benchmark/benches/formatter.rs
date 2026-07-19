use luck_benchmark::corpus::test_files;
use luck_benchmark::{BenchmarkId, Criterion, criterion_group, criterion_main};
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

    group.finish();
}

criterion_group!(formatter, bench_formatter);
criterion_main!(formatter);
