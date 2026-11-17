use luck_allocator::Allocator;
use luck_ast::Block;
use luck_token::LuaVersion;

use crate::compress::Compressor;
use crate::transform_config::TransformConfig;

/// Every pass off, for a test to turn on the ones it covers.
pub(crate) const NO_PASSES: TransformConfig = TransformConfig {
    remove_dead_code: false,
    simplify_statements: false,
    fold_constants: false,
    inline_locals: false,
    merge_locals: false,
    simplify_indexes: false,
    shorten_strings: false,
    shorten_numbers: false,
    simplify_parens: false,
    rename_locals: false,
    lift_locals: false,
    rename_globals: false,
};

pub(crate) const RENAMING: TransformConfig =
    TransformConfig { rename_locals: true, rename_globals: true, ..NO_PASSES };

pub(crate) fn parse<'ast>(
    allocator: &'ast Allocator,
    source: &'ast str,
    version: LuaVersion,
) -> Block<'ast> {
    let result = luck_parser::parse(allocator, source, version);
    assert!(result.errors.is_empty(), "parse failed: {:?}\n{source}", result.errors);
    result.block
}

/// Minifies `source` with `config`, and checks that the output reparses and
/// minifies to itself.
pub(crate) fn minify_with(source: &str, version: LuaVersion, config: &TransformConfig) -> String {
    let allocator = Allocator::new();
    let output = crate::minify(&allocator, source, version, config)
        .unwrap_or_else(|errors| panic!("parse failed: {errors:?}\n{source}"));
    let again = crate::minify(&allocator, &output, version, config)
        .unwrap_or_else(|errors| panic!("output must reparse: {errors:?}\n{output}"));
    assert_eq!(again, output, "minifying the output again changed it\n{source}");
    output
}

pub(crate) fn assert_outputs(apply: fn(&str) -> String, cases: &[(&str, &str)]) {
    for &(source, expected) in cases {
        assert_eq!(apply(source), expected, "source: {source}");
    }
}

pub(crate) fn assert_luau_renames(cases: &[(&str, &str)]) {
    for &(source, expected) in cases {
        let result = minify_with(source, LuaVersion::Luau, &RENAMING);
        assert_eq!(result, expected, "source: {source}");
    }
}

pub(crate) fn assert_lua55_renames(cases: &[(&str, &str)]) {
    for &(source, expected) in cases {
        let result = minify_with(source, LuaVersion::Lua55, &RENAMING);
        assert_eq!(result, expected, "source: {source}");
    }
}

/// One walk of `source`, printed.
pub(crate) fn walk_once(source: &str, config: TransformConfig) -> String {
    let allocator = Allocator::new();
    let mut block = parse(&allocator, source, LuaVersion::Lua54);
    Compressor::new(&allocator, LuaVersion::Lua54, config).walk(&mut block);
    luck_codegen::compact(&block)
}
