//! Luau's compiler limits, from its `BytecodeBuilder.h`.

/// Locals active at once in one function: `kMaxLocalCount`.
pub const MAX_ACTIVE_LOCALS: usize = 200;

/// Registers one function can use: `kMaxRegisterCount`.
pub const MAX_REGISTERS: usize = 255;
