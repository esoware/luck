//! Deterministic random-program generators for property tests and bench
//! corpora, plus the walker and dialect rules for the shared fixture tree.
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
//! fits the smallest stack luck runs on.

#![cfg_attr(
    test,
    expect(
        clippy::missing_panics_doc,
        reason = "a test panics to fail, so a Panics section says nothing"
    )
)]

mod fixtures;
mod full;
mod rng;
mod runtime;
mod stack;

pub use fixtures::{collect_fixture_files, detect_version, is_error_fixture};
pub use full::{generate_full, generate_full_embeddable};
pub use runtime::generate_runtime_safe;
pub use stack::{
    SMALLEST_STACK_BYTES, broken_sources, deep_and_long_sources, long_lists, long_runs,
    on_the_smallest_stack, sources_at_the_nesting_cap, sources_past_the_nesting_cap,
};
