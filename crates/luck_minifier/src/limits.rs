//! The reference compilers' limits, which a merged or hoisted statement has
//! to stay under.

use luck_ast::FunctionBody;

/// Locals active at once in one function: `MAXVARS` in PUC Lua and
/// `kMaxLocalCount` in Luau.
pub(crate) const MAX_ACTIVE_LOCALS: usize = 200;

/// Registers one function can use on the strictest target. 5.1's and 5.2's
/// `MAXSTACK` is 250, and `luaK_checkstack` rejects reaching it.
pub(crate) const MAX_REGISTERS: usize = 249;

/// A numeric `for` loop's hidden locals, before its variable.
pub(crate) const NUMERIC_FOR_STATE_LOCALS: usize = 3;

/// A generic `for` loop's hidden locals, before its variables: 5.4's four,
/// the most any target holds.
pub(crate) const GENERIC_FOR_STATE_LOCALS: usize = 4;

/// Registers one assignment target can hold until the statement stores it:
/// its value, and on 5.4 `_ENV` and the key of a global whose name is past
/// the 255 constants an instruction can address.
const REGISTERS_PER_ASSIGNMENT_TARGET: usize = 3;

/// How many targets one assignment may have once locals hold `live_registers`.
///
/// This is well under the 200 C levels `restassign` recurses through, so the
/// target count needs no limit of its own.
pub(crate) const fn assignment_target_limit(live_registers: usize) -> usize {
    MAX_REGISTERS.saturating_sub(live_registers).div_euclid(REGISTERS_PER_ASSIGNMENT_TARGET)
}

/// Locals and registers `body`'s parameters hold.
///
/// A `:` method's `self` and a named vararg aren't all visible from the body
/// alone, so both are assumed present.
pub(crate) const fn parameter_locals(body: &FunctionBody) -> usize {
    body.params.len().saturating_add(2)
}
