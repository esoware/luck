//! The Luau compiler's limits, which a merged or hoisted statement has to
//! stay under.

use luck_ast::FunctionBody;

/// Registers one function can use, the compiler's `kMaxRegisterCount`.
pub(crate) const MAX_REGISTERS: usize = 255;

/// Registers a numeric `for` loop takes: its limit, step, and index, which
/// its variable shares, and one more for the variable when the body assigns
/// it.
pub(crate) const NUMERIC_FOR_REGISTERS: usize = 4;

/// Registers a generic `for` loop takes: its generator, state, and index,
/// and one per variable, at least two, since the `pairs` and `ipairs` fast
/// paths assume two.
pub(crate) fn generic_for_registers(variables: usize) -> usize {
    3 + variables.max(2)
}

/// Registers one assignment target can hold until the statement stores it:
/// its value, and an indexed target's table and key.
const REGISTERS_PER_ASSIGNMENT_TARGET: usize = 3;

/// How many targets one assignment may have once locals hold `live_registers`.
pub(crate) const fn assignment_target_limit(live_registers: usize) -> usize {
    MAX_REGISTERS.saturating_sub(live_registers) / REGISTERS_PER_ASSIGNMENT_TARGET
}

/// A `:` method's `self` isn't visible from the body alone, so it's assumed
/// present. A vararg takes no register.
pub(crate) fn parameter_locals(body: &FunctionBody<'_>) -> usize {
    body.params.len() + 1
}
