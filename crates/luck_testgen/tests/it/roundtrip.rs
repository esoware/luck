use luck_allocator::Allocator;
use luck_formatter::{Comments, FormatOptions, format, format_block};
use luck_minifier::TransformConfig;
use luck_parser::ParseResult;
use luck_testgen::{generate_full, generate_runtime_safe};
use luck_token::LuaVersion;

const SEED_COUNT: u64 = 60;
const STATEMENT_BUDGET: usize = 25;

struct Program {
    source: String,
    version: LuaVersion,
    /// The program's profile, seed, and version, for failure messages.
    label: String,
}

/// The full-grammar profile reaches constructs (goto, types, varargs,
/// interpolation) the runtime-safe profile can't emit.
fn programs() -> impl Iterator<Item = Program> {
    LuaVersion::ALL.into_iter().flat_map(|version| {
        (0..SEED_COUNT).flat_map(move |seed| {
            [
                Program {
                    source: generate_runtime_safe(seed, version, STATEMENT_BUDGET),
                    version,
                    label: format!("runtime seed {seed} {version:?}"),
                },
                Program {
                    source: generate_full(seed, version, STATEMENT_BUDGET),
                    version,
                    label: format!("full seed {seed} {version:?}"),
                },
            ]
        })
    })
}

fn parse_cleanly<'ast>(
    allocator: &'ast Allocator,
    program: &Program,
    stage: &str,
    output: &'ast str,
) -> ParseResult<'ast> {
    let parsed = luck_parser::parse(allocator, output, program.version);
    assert!(
        parsed.errors.is_empty(),
        "{}: {stage} output failed to parse.\ninput:\n{}\noutput:\n{output}\nerrors: {:?}",
        program.label,
        program.source,
        parsed.errors
    );
    parsed
}

#[test]
fn format_reparses_and_is_idempotent() {
    let options = FormatOptions::default();
    let mut allocator = Allocator::new();
    for program in programs() {
        allocator.reset();
        let Program { source, version, label } = &program;
        let first = format(&allocator, source, *version, &options);
        assert!(
            first.errors.is_empty(),
            "{label}: format errored:\n{source}\nerrors: {:?}",
            first.errors
        );
        parse_cleanly(&allocator, &program, "format", &first.output);
        let second = format(&allocator, &first.output, *version, &options);
        assert_eq!(
            first.output, second.output,
            "{label}: format not idempotent.\ninput:\n{source}"
        );
    }
}

/// The AST-in path: the parsed program, fed back as a bare AST with no
/// source or comments, must still print valid Lua that formats as itself.
#[test]
fn format_block_without_source_reparses_and_formats_as_itself() {
    let options = FormatOptions::default();
    let mut allocator = Allocator::new();
    for program in programs() {
        allocator.reset();
        let Program { source, version, label } = &program;
        let original = parse_cleanly(&allocator, &program, "generator", source);
        let via_ast = format_block(&allocator, &original.block, Comments::none(), &options)
            .expect("a parsed tree nests within the cap");
        parse_cleanly(&allocator, &program, "format_block", &via_ast);
        let reformatted = format(&allocator, &via_ast, *version, &options);
        assert_eq!(
            via_ast, reformatted.output,
            "{label}: format changed format_block's output.\ninput:\n{source}"
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
        let Program { source, version, label } = &program;
        let minified = luck_minifier::minify(&allocator, source, *version, &config)
            .unwrap_or_else(|errors| panic!("{label}: minify errored: {errors:?}\n{source}"));
        let first = format(&allocator, &minified, *version, &options);
        assert!(
            first.errors.is_empty(),
            "{label}: format errored on minified output:\n{minified}\nerrors: {:?}",
            first.errors
        );
        parse_cleanly(&allocator, &program, "format of minify", &first.output);
        let second = format(&allocator, &first.output, *version, &options);
        assert_eq!(
            first.output, second.output,
            "{label}: format not idempotent on minified output.\nminified:\n{minified}"
        );
    }
}
