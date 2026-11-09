//! Post-parse validations that need scope context.

mod continue_until;
mod gotos;
mod scope;

use luck_ast::Block;
use luck_token::LuaVersion;

pub(crate) use self::scope::check_type_functions;
use crate::ParseError;

/// Compile-time scope checks that Lua performs and [`parse`](crate::parse)
/// skips.
///
/// Covers writes to read-only bindings, names no Lua 5.5 `global` declaration
/// covers, goto/label resolution, and Luau's continue/until rule. They need an
/// extra AST walk, so transform pipelines skip them and `luck check` opts in.
/// Run it only on a clean parse, since a recovered AST produces misleading
/// follow-on errors.
#[must_use]
pub fn validate(block: &Block, version: LuaVersion) -> Vec<ParseError> {
    if version.has_goto() || version.has_read_only_bindings() || version.has_continue() {
        scope::check(block, version)
    } else {
        Vec::new()
    }
}
