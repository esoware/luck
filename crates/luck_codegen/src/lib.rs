//! # `luck_codegen`
//!
//! Prints a Lua or Luau syntax tree as the shortest source that reads back
//! as the same tree.
//!
//! [`compact()`] writes no comments, a space only where two pieces would
//! otherwise lex as one token, and a `;` only before a statement that starts
//! with `(` and would otherwise call the one before. It writes the
//! parentheses the tree keeps, and derives the rest from where each
//! expression goes, the fewest that read back as the same tree. It spells
//! each literal as the tree spells it, unchecked, or with
//! [`LiteralSpelling::Shortest`], the shortest way its value reads back in
//! the chunk's version when that's shorter, as a minifier wants. It never
//! reads source text, so a tree built in code prints too, its literals
//! spelled from their values. It writes bytes, as a source is, which are
//! UTF-8 unless a literal as the tree spells it isn't.
//!
//! It recurses once per level of nesting, so it takes a tree within
//! `luck_ast::MAX_NESTING_DEPTH`, as `luck_parser` builds. Measure a tree
//! built some other way with `luck_ast::block_nesting_depth` first.
//!
//! # Usage
//!
//! ```
//! use luck_allocator::Allocator;
//! use luck_ast::{Chunk, SpanCounter, Synth};
//! use luck_codegen::LiteralSpelling;
//! use luck_token::{BinOp, LuaVersion};
//!
//! let allocator = Allocator::new();
//! let spans = SpanCounter::new();
//! let synth = Synth::new(&allocator, &spans);
//! let sum = synth.binary_op(synth.name_expr("a"), BinOp::Sub, synth.number_f64(0.5));
//! let block = synth.block([synth.local(&["x"], [sum]), synth.return_([synth.name_expr("x")])]);
//! let chunk = Chunk::new(LuaVersion::Lua54, block);
//! let output = luck_codegen::compact(&chunk, LiteralSpelling::AsWritten);
//! assert_eq!(output, b"local x=a-.5 return x");
//! ```

mod compact;
mod separator;

pub use compact::{LiteralSpelling, compact, compact_into};
