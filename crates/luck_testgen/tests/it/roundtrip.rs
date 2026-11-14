#![expect(
    clippy::missing_panics_doc,
    reason = "a test panics to fail, so a Panics section says nothing"
)]

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

fn parse_cleanly(program: &Program, stage: &str, output: &str) -> ParseResult {
    let parsed = luck_parser::parse(output, program.version);
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
fn generated_programs_parse_cleanly() {
    for program in programs() {
        parse_cleanly(&program, "generator", &program.source);
    }
}

#[test]
fn compact_output_reparses() {
    for program in programs() {
        let parsed = parse_cleanly(&program, "generator", &program.source);
        let compacted = luck_codegen::compact(&parsed.block).unwrap();
        parse_cleanly(&program, "compact", &compacted);
    }
}

#[test]
fn format_reparses_and_is_idempotent() {
    let options = FormatOptions::default();
    for program in programs() {
        let Program { source, version, label } = &program;
        let first = format(source, *version, &options);
        assert!(
            first.errors.is_empty(),
            "{label}: format errored:\n{source}\nerrors: {:?}",
            first.errors
        );

        parse_cleanly(&program, "format", &first.output);

        let second = format(&first.output, *version, &options);
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
    for program in programs() {
        let Program { source, version, label } = &program;
        let original = parse_cleanly(&program, "generator", source);
        let via_ast = format_block(&original.block, Comments::none(), &options).unwrap();
        parse_cleanly(&program, "format_block", &via_ast);
        let reformatted = format(&via_ast, *version, &options);
        assert_eq!(
            via_ast, reformatted.output,
            "{label}: format changed format_block's output.\ninput:\n{source}"
        );
    }
}

#[test]
fn minify_reparses_and_reaches_a_fixpoint_by_the_second_pass() {
    let config = TransformConfig::default();
    for program in programs() {
        let Program { source, version, label } = &program;
        let Ok(first) = luck_minifier::minify(source, *version, &config) else {
            panic!("{label}: minify errored on valid input:\n{source}");
        };

        parse_cleanly(&program, "minify", &first);

        let Ok(second) = luck_minifier::minify(&first, *version, &config) else {
            panic!("{label}: second minify errored.\nfirst output:\n{first}");
        };
        // Names aren't byte-stable: lift and merge run after rename, so the
        // second pass ranks slots against a different shape and may permute
        // short names. What holds is the length (no growth, no further
        // shrinkage) and a byte-exact fixpoint from the second pass on.
        assert_eq!(
            first.len(),
            second.len(),
            "{label}: second minify changed size.\nfirst:\n{first}\nsecond:\n{second}"
        );
        let Ok(third) = luck_minifier::minify(&second, *version, &config) else {
            panic!("{label}: third minify errored");
        };
        assert_eq!(second, third, "{label}: minify has no fixpoint.\ninput:\n{source}");
    }
}
