use luck_allocator::Allocator;
use luck_formatter::{Comments, FormatOptions, format, format_chunk};
use luck_minifier::TransformConfig;
use luck_parser::ParseResult;
use luck_testgen::{generate_full_grammar, generate_runtime_safe, tree_shape};

const SEED_COUNT: u64 = 60;
const STATEMENT_BUDGET: usize = 25;

struct Program {
    source: String,
    /// The program's profile and seed, for failure messages.
    label: String,
}

/// The full-grammar profile reaches constructs (types, varargs,
/// interpolation) the runtime-safe profile can't emit.
fn programs() -> impl Iterator<Item = Program> {
    (0..SEED_COUNT).flat_map(|seed| {
        [
            Program {
                source: generate_runtime_safe(seed, STATEMENT_BUDGET),
                label: format!("runtime seed {seed}"),
            },
            Program {
                source: generate_full_grammar(seed, STATEMENT_BUDGET),
                label: format!("full seed {seed}"),
            },
        ]
    })
}

fn parse_cleanly<'ast>(
    allocator: &'ast Allocator,
    program: &Program,
    stage: &str,
    output: &'ast [u8],
) -> ParseResult<'ast> {
    let parsed = luck_parser::parse(allocator, output);
    assert!(
        parsed.errors.is_empty(),
        "{}: {stage} output failed to parse.\ninput:\n{}\noutput:\n{}\nerrors: {:?}",
        program.label,
        program.source,
        String::from_utf8_lossy(output),
        parsed.errors
    );
    parsed
}

/// The printer kept the program's meaning: what it printed parses back to the
/// tree it was given, up to what a printer may change.
fn assert_same_tree(
    program: &Program,
    stage: &str,
    before: &ParseResult<'_>,
    after: &ParseResult<'_>,
) {
    assert_eq!(
        tree_shape(&before.chunk),
        tree_shape(&after.chunk),
        "{}: {stage} changed the tree.\ninput:\n{}",
        program.label,
        program.source
    );
}

#[test]
fn format_reparses_and_is_idempotent() {
    let options = FormatOptions::default();
    let mut allocator = Allocator::new();
    for program in programs() {
        allocator.reset();
        let Program { source, label } = &program;
        let original = parse_cleanly(&allocator, &program, "generator", source.as_bytes());
        let first = format(&allocator, source.as_bytes(), &options);
        assert!(
            first.errors.is_empty(),
            "{label}: format errored:\n{source}\nerrors: {:?}",
            first.errors
        );
        let formatted = parse_cleanly(&allocator, &program, "format", &first.output);
        assert_same_tree(&program, "format", &original, &formatted);
        let second = format(&allocator, &first.output, &options);
        assert_eq!(
            first.output, second.output,
            "{label}: format not idempotent.\ninput:\n{source}"
        );
    }
}

/// The AST-in path: the parsed program, fed back as a bare AST with no
/// source or comments, must still print valid Luau that formats as itself.
#[test]
fn format_chunk_without_source_reparses_and_formats_as_itself() {
    let options = FormatOptions::default();
    let mut allocator = Allocator::new();
    for program in programs() {
        allocator.reset();
        let Program { source, label } = &program;
        let original = parse_cleanly(&allocator, &program, "generator", source.as_bytes());
        let via_ast = format_chunk(&allocator, &original.chunk, Comments::none(), &options)
            .expect("a parsed tree nests within the cap");
        let reprinted = parse_cleanly(&allocator, &program, "format_chunk", &via_ast);
        assert_same_tree(&program, "format_chunk", &original, &reprinted);
        let reformatted = format(&allocator, &via_ast, &options);
        assert_eq!(
            via_ast, reformatted.output,
            "{label}: format changed format_chunk's output.\ninput:\n{source}"
        );
    }
}

/// Minified code, dense and renamed, is the input a reader formats to read.
#[test]
fn formatting_minified_output_reparses_and_is_idempotent() {
    let options = FormatOptions::default();
    let config = TransformConfig::default();
    let mut allocator = Allocator::new();
    for program in programs() {
        allocator.reset();
        let Program { source, label } = &program;
        let minified = luck_minifier::minify(&allocator, source.as_bytes(), &config)
            .unwrap_or_else(|errors| panic!("{label}: minify errored: {errors:?}\n{source}"));
        let minified_parse = parse_cleanly(&allocator, &program, "minify", &minified);
        let first = format(&allocator, &minified, &options);
        let minified_text = String::from_utf8_lossy(&minified);
        assert!(
            first.errors.is_empty(),
            "{label}: format errored on minified output:\n{minified_text}\nerrors: {:?}",
            first.errors
        );
        let formatted = parse_cleanly(&allocator, &program, "format of minify", &first.output);
        assert_same_tree(&program, "format of minify", &minified_parse, &formatted);
        let second = format(&allocator, &first.output, &options);
        assert_eq!(
            first.output, second.output,
            "{label}: format not idempotent on minified output.\nminified:\n{minified_text}"
        );
    }
}
