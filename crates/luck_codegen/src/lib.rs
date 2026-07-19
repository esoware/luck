//! # luck_codegen
//!
//! Code generation from Lua ASTs back to source text.
//!
//! - [`compact()`] - Minimal output, strips comments, smart separators. Used after minification.
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
#[must_use]
pub fn compact(block: &Block, source: &str) -> String {
    let mut printer = compact::CompactPrinter::new(source);
    printer.emit_block(block);
    printer.output()
}
