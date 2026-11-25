use luck_allocator::Allocator;
use luck_ast::Chunk;
use luck_codegen::LiteralSpelling;

use crate::binding::{NameIndexes, analyze};
use crate::compress::{Compressor, compress};
use crate::transform_config::TransformConfig;

/// Every pass off, for a test to turn on the ones it covers.
pub(crate) const NO_PASSES: TransformConfig = TransformConfig {
    strip_types: false,
    remove_dead_code: false,
    simplify_statements: false,
    fold_constants: false,
    inline_locals: false,
    merge_locals: false,
    simplify_indexes: false,
    simplify_parens: false,
    rename_locals: false,
    lift_locals: false,
    rename_globals: false,
};

pub(crate) const RENAMING: TransformConfig =
    TransformConfig { rename_locals: true, rename_globals: true, ..NO_PASSES };

pub(crate) fn parse<'ast>(allocator: &'ast Allocator, source: &'ast str) -> Chunk<'ast> {
    let result = luck_parser::parse(allocator, source.as_bytes());
    assert!(result.errors.is_empty(), "parse failed: {:?}\n{source}", result.errors);
    result.chunk
}

/// Minifies `source` with `config`, and checks that the output reparses and
/// minifies to itself.
pub(crate) fn minify_with(source: &str, config: &TransformConfig) -> String {
    let allocator = Allocator::new();
    let minified = crate::minify(&allocator, source.as_bytes(), config)
        .unwrap_or_else(|errors| panic!("parse failed: {errors:?}\n{source}"));
    let output = String::from_utf8(minified).expect("a source that's UTF-8 minifies as UTF-8");
    let again = crate::minify(&allocator, output.as_bytes(), config)
        .unwrap_or_else(|errors| panic!("output must reparse: {errors:?}\n{output}"));
    assert_eq!(again, output.as_bytes(), "minifying the output again changed it\n{source}");
    output
}

pub(crate) fn assert_outputs(apply: fn(&str) -> String, cases: &[(&str, &str)]) {
    for &(source, expected) in cases {
        assert_eq!(apply(source), expected, "source: {source}");
    }
}

pub(crate) fn assert_renames(cases: &[(&str, &str)]) {
    for &(source, expected) in cases {
        let result = minify_with(source, &RENAMING);
        assert_eq!(result, expected, "source: {source}");
    }
}

/// One compression pass of `source`, printed.
pub(crate) fn walk_once(source: &str, config: TransformConfig) -> String {
    let allocator = Allocator::new();
    let result = luck_parser::parse(&allocator, source.as_bytes());
    let mut chunk = result.chunk;
    let mut scoping = analyze(&chunk, result.counts, None);
    Compressor::new(&allocator, config, &mut scoping).pass(&mut chunk.block);
    String::from_utf8(luck_codegen::compact(&chunk, LiteralSpelling::Shortest)).unwrap()
}

/// Compresses `source` as a minify does, without renaming.
pub(crate) fn compressed<'ast>(
    allocator: &'ast Allocator,
    source: &'ast str,
    config: TransformConfig,
) -> Chunk<'ast> {
    let result = luck_parser::parse(allocator, source.as_bytes());
    let mut chunk = result.chunk;
    let mut name_indexes = NameIndexes::new(result.counts.names);
    compress(allocator, &mut chunk, result.counts, config, &mut name_indexes);
    chunk
}
