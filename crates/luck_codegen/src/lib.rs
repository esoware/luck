//! # `luck_codegen`
//!
//! Code generation from Lua ASTs back to source text.
//!
//! [`compact()`] prints the minimal code the minifier outputs: no comments,
//! and a space only where two adjacent pieces would otherwise merge into one
//! token. It never reads source text, so synthetic ASTs print too, and it
//! prints each leaf's text as the tree holds it, unchecked. A tree nested past
//! its dialect's `max_nesting_depth` prints output the parser refuses, so check
//! a tree built outside luck with `luck_ast::block_nesting_depth` first.
//!
//! # Usage
//!
//! ```
//! use luck_token::LuaVersion;
//!
//! let parsed = luck_parser::parse("local x = 1", LuaVersion::Lua54);
//! let output = luck_codegen::compact(&parsed.block);
//! assert!(output.contains("x=1"));
//! ```

#![cfg_attr(
    test,
    expect(
        clippy::missing_panics_doc,
        reason = "a test panics to fail, so a Panics section says nothing"
    )
)]

mod compact;
mod separator;

pub use compact::{compact, compact_with_capacity};
