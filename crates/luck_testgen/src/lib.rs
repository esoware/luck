//! Deterministic random-program generators for property tests and
//! benchmark files, plus the walker and dialect rules for the shared fixture
//! tree.
//!
//! [`generate_runtime_safe`] programs are:
//! - syntactically valid for the requested `LuaVersion`;
//! - runtime-safe: every variable is initialized, arithmetic operands are
//!   numbers, loops are bounded, no library calls with observable
//!   nondeterminism (`pairs` order, time, GC);
//! - observable: values are `print`ed so behavior differences show up on
//!   stdout when original and transformed programs are executed.
//!
//! [`generate_full`] gives up runtime safety to reach the whole grammar.
//!
//! [`deep_and_long_sources`] and [`on_the_smallest_stack`] check that a stage
//! fits the stack luck asks of its threads.
//!
//! [`tree_shape`](fn@tree_shape) checks that a printer kept a tree's
//! meaning, by comparing the tree with the parse of what it printed.

mod fixtures;
mod full;
mod rng;
mod runtime;
mod stack;
mod tree_shape;

pub use fixtures::{collect_fixture_files, detect_version, is_error_fixture};
pub use full::generate_full;
pub use runtime::generate_runtime_safe;
pub use stack::{
    broken_sources, deep_and_long_sources, on_the_smallest_stack, sources_at_the_nesting_cap,
    sources_past_the_nesting_cap, with_the_smallest_stack,
};
pub use tree_shape::tree_shape;
