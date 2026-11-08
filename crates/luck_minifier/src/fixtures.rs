//! Helpers the passes' unit tests share.

use luck_ast::Block;
use luck_token::LuaVersion;

/// Parses `source` under `version`, failing the test on any parse error.
pub(crate) fn parse(source: &str, version: LuaVersion) -> Block {
    let result = luck_parser::parse(source, version);
    assert!(result.errors.is_empty(), "parse failed: {:?}\n{source}", result.errors);
    result.block
}

/// Runs `pass` over `source` parsed under `version`, and prints the result
/// compactly.
pub(crate) fn run_pass(
    source: &str,
    version: LuaVersion,
    pass: impl FnOnce(Block) -> Block,
) -> String {
    luck_codegen::compact(&pass(parse(source, version)))
}

pub(crate) fn assert_reparses(output: &str, version: LuaVersion) {
    let result = luck_parser::parse(output, version);
    assert!(result.errors.is_empty(), "output must reparse: {:?}\n{output}", result.errors);
}
