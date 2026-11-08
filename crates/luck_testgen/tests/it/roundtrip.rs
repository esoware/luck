//! Property tests over generated programs: formatter and minifier
//! idempotency, re-parseable output, and compact codegen round-tripping,
//! across many seeds and versions.

#![expect(
    clippy::missing_panics_doc,
    reason = "a test panics to fail, so a Panics section says nothing"
)]

use luck_core::TransformConfig;
use luck_formatter::{Comments, FormatOptions, blocks_equivalent, format, format_block};
use luck_testgen::{generate, generate_full};
use luck_token::LuaVersion;

const SEED_COUNT: u64 = 60;
const STATEMENT_BUDGET: usize = 25;

/// Runs `check` on every (version, seed) program from both generator profiles.
///
/// The full-grammar profile reaches constructs (goto, types, varargs,
/// interpolation) the runtime-safe profile can't emit. The last argument
/// labels the program's profile, seed, and version for failure messages.
fn for_each_program(check: impl Fn(&str, LuaVersion, String)) {
    for version in LuaVersion::ALL {
        for seed in 0..SEED_COUNT {
            check(
                &generate(seed, version, STATEMENT_BUDGET),
                version,
                format!("runtime seed {seed} {version:?}"),
            );
            check(
                &generate_full(seed, version, STATEMENT_BUDGET),
                version,
                format!("full seed {seed} {version:?}"),
            );
        }
    }
}

#[test]
fn generated_programs_parse_cleanly() {
    for_each_program(|source, version, label| {
        let parsed = luck_parser::parse(source, version);
        assert!(
            parsed.errors.is_empty(),
            "{label}: generator produced invalid program:\n{source}\nerrors: {:?}",
            parsed.errors
        );
    });
}

#[test]
fn compact_output_reparses() {
    for_each_program(|source, version, label| {
        let parsed = luck_parser::parse(source, version);
        if !parsed.errors.is_empty() {
            return;
        }
        let compacted = luck_codegen::compact(&parsed.block);
        let reparsed = luck_parser::parse(&compacted, version);
        assert!(
            reparsed.errors.is_empty(),
            "{label}: compact output failed to reparse.\ninput:\n{source}\ncompact:\n{compacted}\nerrors: {:?}",
            reparsed.errors
        );
    });
}

#[test]
fn format_is_idempotent_reparses_and_keeps_ast_structure() {
    let options = FormatOptions::default();
    for_each_program(|source, version, label| {
        let first = format(source, version, &options);
        assert!(
            first.errors.is_empty(),
            "{label}: format errored:\n{source}\nerrors: {:?}",
            first.errors
        );

        let reparsed = luck_parser::parse(&first.output, version);
        assert!(
            reparsed.errors.is_empty(),
            "{label}: formatted output failed to reparse.\ninput:\n{source}\noutput:\n{}",
            first.output
        );

        let second = format(&first.output, version, &options);
        assert_eq!(
            first.output, second.output,
            "{label}: format not idempotent.\ninput:\n{source}"
        );

        // Formatting must not drop or reshape any statement, the same check
        // `luck fmt --verify` runs.
        let original = luck_parser::parse(source, version);
        if let Err(diff) = blocks_equivalent(&original.block, &reparsed.block, version) {
            panic!(
                "{label}: source-in format changed AST structure.\ninput:\n{source}\noutput:\n{}\ndiff: {diff:?}",
                first.output
            );
        }
    });
}

/// The AST-in path: the parsed program, fed back as a bare AST with no
/// source or comments, must still print valid, structurally identical Lua.
#[test]
fn format_block_without_source_reparses_and_keeps_ast_structure() {
    let options = FormatOptions::default();
    for_each_program(|source, version, label| {
        let original = luck_parser::parse(source, version);
        if !original.errors.is_empty() {
            return;
        }
        let via_ast = format_block(&original.block, Comments::none(), &options);
        let reparsed = luck_parser::parse(&via_ast, version);
        assert!(
            reparsed.errors.is_empty(),
            "{label}: format_block output failed to reparse.\ninput:\n{source}\noutput:\n{via_ast}\nerrors: {:?}",
            reparsed.errors
        );
        if let Err(diff) = blocks_equivalent(&original.block, &reparsed.block, version) {
            panic!(
                "{label}: format_block changed AST structure.\ninput:\n{source}\noutput:\n{via_ast}\ndiff: {diff:?}"
            );
        }
    });
}

#[test]
fn minify_reparses_and_reaches_a_fixpoint_by_the_second_pass() {
    let config = TransformConfig::default();
    for_each_program(|source, version, label| {
        let Ok(first) = luck_minifier::minify(source, version, &config, "gen.lua") else {
            panic!("{label}: minify errored on valid input:\n{source}");
        };

        let reparsed = luck_parser::parse(&first, version);
        assert!(
            reparsed.errors.is_empty(),
            "{label}: minified output failed to reparse.\ninput:\n{source}\noutput:\n{first}"
        );

        let Ok(second) = luck_minifier::minify(&first, version, &config, "gen.lua") else {
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
        let Ok(third) = luck_minifier::minify(&second, version, &config, "gen.lua") else {
            panic!("{label}: third minify errored");
        };
        assert_eq!(second, third, "{label}: minify has no fixpoint.\ninput:\n{source}");
    });
}
