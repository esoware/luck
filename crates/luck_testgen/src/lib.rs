//! The test inputs luck's crates share.
//!
//! - [`generate_runtime_safe`] writes a random program that runs without
//!   error and prints what it computes, and [`generate_full_grammar`] one
//!   that reaches the whole grammar instead. A seed generates the same
//!   program on every platform, so it decides what every property test
//!   covers.
//! - [`sources_at_the_nesting_cap`] and the other sources nest to and far
//!   past [`luck_ast::MAX_NESTING_DEPTH`], and [`on_the_smallest_stack`] and
//!   [`with_the_smallest_stack`] run a stage on the stack luck asks of its
//!   threads.
//! - [`luau_fixtures`] and [`fixture_files`] walk the shared fixture tree.
//! - [`tree_shape`](fn@tree_shape) checks that a printer kept a tree's
//!   meaning, by comparing the tree with the parse of what it printed.

mod fixtures;
mod full_grammar;
mod rng;
mod runtime_safe;
mod stack;
mod tree_shape;

pub use fixtures::{LuauFixture, fixture_files, luau_fixtures};
pub use full_grammar::generate_full_grammar;
pub use runtime_safe::generate_runtime_safe;
pub use stack::{
    broken_sources, deep_and_long_sources, nested, on_the_smallest_stack,
    sources_at_the_nesting_cap, sources_past_the_nesting_cap, with_the_smallest_stack,
};
pub use tree_shape::tree_shape;
