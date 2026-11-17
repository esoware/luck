//! # `luck_codegen`
//!
//! Prints a Lua or Luau syntax tree as the shortest source that reads back
//! as the same tree.
//!
//! [`compact()`] writes no comments, a space only where two pieces would
//! otherwise lex as one token, and a `;` only before a statement that starts
//! with `(` and would otherwise call the one before. It never reads source
//! text, so a tree built in code prints too, and it prints each leaf's text
//! as the tree holds it, unchecked.
//!
//! It recurses once per level of nesting, so it takes a tree within
//! `luck_ast::MAX_NESTING_DEPTH`, as `luck_parser` builds. Measure a tree
//! built some other way with `luck_ast::block_nesting_depth` first.
//!
//! # Usage
//!
//! ```
//! use luck_allocator::Allocator;
//! use luck_ast::{SpanCounter, Synth};
//! use luck_token::BinOp;
//!
//! let allocator = Allocator::new();
//! let spans = SpanCounter::new();
//! let synth = Synth::new(&allocator, &spans);
//! let sum = synth.binary_op(synth.name_expr("a"), BinOp::Sub, synth.number(".5"));
//! let block = synth.block([synth.local(&["x"], [sum])], Some(synth.return_([synth.name_expr("x")])));
//! assert_eq!(luck_codegen::compact(&block), "local x=a-.5 return x");
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
