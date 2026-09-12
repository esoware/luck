//! # luck_codegen
//!
//! Code generation from Lua ASTs back to source text.
//!
//! - [`compact()`] - Minimal output, drops comments, spaces only where two
//!   adjacent pieces would otherwise merge into one token. Used after
//!   minification.
//!
//! # Usage
//!
//! ```
//! use luck_token::LuaVersion;
//!
//! let parsed = luck_parser::parse("local x = 1", LuaVersion::Lua54);
//! let output = luck_codegen::compact(&parsed.block, &parsed.source);
//! assert!(output.contains("x=1"));
//! ```

mod compact;
mod separator;

use luck_ast::Block;

/// Emit AST as minimal compact Lua code (no comments, minimal whitespace).
///
/// Only `source.len()` is used, as an output capacity hint. Pass `""` for
/// synthetic ASTs or comment-heavy input, or use [`compact_with_capacity`]
/// with an estimate such as a previous emitted length.
#[must_use]
pub fn compact(block: &Block, source: &str) -> String {
    compact_with_capacity(block, source.len())
}

/// Emit compact Lua code with an explicit initial output capacity in bytes.
/// The hint affects allocation only; it never limits or changes the output.
/// Use zero when no useful estimate is available.
#[must_use]
pub fn compact_with_capacity(block: &Block, capacity: usize) -> String {
    let mut printer = compact::CompactPrinter::new(capacity);
    printer.emit_block(block);
    printer.output()
}
