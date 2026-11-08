//! Deterministic random-program generators for property tests and bench
//! corpora.
//!
//! [`generate`] programs are:
//! - syntactically valid for the requested `LuaVersion`;
//! - runtime-safe: every variable is initialized, arithmetic operands are
//!   numbers, loops are bounded, no library calls with observable
//!   nondeterminism (`pairs` order, time, GC);
//! - observable: values are `print`ed so behavior differences show up on
//!   stdout when original and transformed programs are executed.
//!
//! [`generate_full`] gives up runtime safety to reach the whole grammar.

#![cfg_attr(
    test,
    expect(
        clippy::missing_panics_doc,
        reason = "a test panics to fail, so a Panics section says nothing"
    )
)]

mod full;
mod rng;
mod runtime;

pub use full::{generate_full, generate_full_embeddable};
pub use runtime::generate;
