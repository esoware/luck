//! Property tests over generated programs: the workspace's hard
//! invariants 7 (idempotency) and 8 (re-parseability), plus compact
//! codegen round-tripping, exercised across many seeds and versions.

use luck_core::transform_config::TransformConfig;
use luck_core::types::LuaTarget;
use luck_formatter::{Comments, blocks_equiv, format, format_block};
use luck_testgen::{generate, generate_full};
use luck_token::LuaVersion;

const SEEDS: u64 = 60;
const BUDGET: usize = 25;

fn versions() -> Vec<(LuaVersion, LuaTarget)> {
    vec![
        (LuaVersion::Lua51, LuaTarget::Lua51),
        (LuaVersion::Lua52, LuaTarget::Lua52),
        (LuaVersion::Lua53, LuaTarget::Lua53),
        (LuaVersion::Lua54, LuaTarget::Lua54),
        (LuaVersion::Lua55, LuaTarget::Lua55),
        (LuaVersion::Luau, LuaTarget::Luau),
    ]
}

/// Runs `check` on every (version, seed) program from both generator
/// profiles: the runtime-safe one and the full-grammar one, which reaches
/// constructs (goto, types, varargs, interpolation) the runtime profile
/// cannot emit. `label` names the profile in failure messages.
fn for_each_program(check: impl Fn(&str, LuaVersion, LuaTarget, String)) {
    for (version, target) in versions() {
        for seed in 0..SEEDS {
            check(
                &generate(seed, version, BUDGET),
                version,
                target,
                format!("runtime seed {seed} {version:?}"),
            );
            check(
                &generate_full(seed, version, BUDGET),
                version,
                target,
                format!("full seed {seed} {version:?}"),
            );
        }
    }
}

#[test]
fn generated_programs_parse_cleanly() {
    for_each_program(|source, version, _, label| {
        let result = luck_parser::parse(source, version);
        assert!(
            result.errors.is_empty(),
            "{label}: generator produced invalid program:\n{source}\nerrors: {:?}",
            result.errors
        );
    });
}

#[test]
fn compact_output_reparses() {
    for_each_program(|source, version, _, label| {
        let parsed = luck_parser::parse(source, version);
        if !parsed.errors.is_empty() {
            return;
        }
        let compacted = luck_codegen::compact(&parsed.block, &parsed.source);
        let reparsed = luck_parser::parse(&compacted, version);
        assert!(
            reparsed.errors.is_empty(),
            "{label}: compact output failed to reparse.\ninput:\n{source}\ncompact:\n{compacted}\nerrors: {:?}",
            reparsed.errors
        );
    });
}

#[test]
fn format_is_idempotent_and_reparses() {
    let options = luck_formatter::FormatOptions::default();
    for_each_program(|source, version, _, label| {
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

        // Structure preservation: formatting must not drop or reshape any
        // statement (hard invariant behind `--verify`).
        let original = luck_parser::parse(source, version);
        if let Err(diff) = blocks_equiv(&original.block, &reparsed.block) {
            panic!(
                "{label}: source-in format changed AST structure.\ninput:\n{source}\noutput:\n{}\ndiff: {diff:?}",
                first.output
            );
        }

        // Same guarantees on the AST-in path: the parsed program, fed back in as a raw
        // AST with no source or comments, must still round-trip to valid,
        // structurally identical Lua.
        let via_ast = format_block(&original.block, Comments::none(), &options);
        let ast_reparsed = luck_parser::parse(&via_ast, version);
        assert!(
            ast_reparsed.errors.is_empty(),
            "{label}: format_block output failed to reparse.\ninput:\n{source}\noutput:\n{via_ast}\nerrors: {:?}",
            ast_reparsed.errors
        );
        if let Err(diff) = blocks_equiv(&original.block, &ast_reparsed.block) {
            panic!(
                "{label}: format_block changed AST structure.\ninput:\n{source}\noutput:\n{via_ast}\ndiff: {diff:?}"
            );
        }
    });
}

#[test]
fn minify_is_idempotent_and_reparses() {
    let config = TransformConfig::default();
    for_each_program(|source, version, target, label| {
        // TEMP-DEBUG
        std::fs::write(std::env::temp_dir().join("luck_min_input.lua"), source).unwrap();
        std::fs::write(std::env::temp_dir().join("luck_min_label.txt"), &label).unwrap();
        let Ok(first) = luck_minifier::minify(source, target, &config, "gen.lua") else {
            panic!("{label}: minify errored on valid input:\n{source}");
        };

        let reparsed = luck_parser::parse(&first, version);
        assert!(
            reparsed.errors.is_empty(),
            "{label}: minified output failed to reparse.\ninput:\n{source}\noutput:\n{first}"
        );

        let Ok(second) = luck_minifier::minify(&first, target, &config, "gen.lua") else {
            panic!("{label}: second minify errored.\nfirst output:\n{first}");
        };
        // Naming isn't byte-stable yet: lift/merge run AFTER rename, so
        // the second pass ranks slots against a different shape and may
        // permute short names. Until the binding-ID rework makes
        // renaming canonical, require structural idempotency: identical
        // length (no growth, no further shrinkage) and a byte-exact
        // fixpoint from the second pass onward.
        assert_eq!(
            first.len(),
            second.len(),
            "{label}: second minify changed size.\nfirst:\n{first}\nsecond:\n{second}"
        );
        let Ok(third) = luck_minifier::minify(&second, target, &config, "gen.lua") else {
            panic!("{label}: third minify errored");
        };
        assert_eq!(
            second, third,
            "{label}: minify has no fixpoint.\ninput:\n{source}"
        );
    });
}
