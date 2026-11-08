//! Post-parse validations that need scope context.

mod const_writes;
mod continue_until;
mod gotos;

use luck_ast::Block;
use luck_token::LuaVersion;

use crate::ParseError;

/// Compile-time scope checks that Lua performs and [`parse`](crate::parse)
/// skips.
///
/// Covers writes to read-only bindings, goto/label resolution, and Luau's
/// continue/until rule. They need extra AST walks, so transform pipelines
/// skip them and `luck check` opts in. Run it only on a clean parse, since a
/// recovered AST produces misleading follow-on errors.
#[must_use]
pub fn validate(block: &Block, version: LuaVersion) -> Vec<ParseError> {
    let mut errors = Vec::new();
    if version.has_attributes() || version.is_luau() {
        errors.extend(const_writes::check(block, version));
    }
    if version.has_goto() {
        errors.extend(gotos::check(block));
    }
    if version.has_continue() {
        errors.extend(continue_until::check(block));
    }
    errors
}
