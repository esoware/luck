#![expect(
    clippy::missing_panics_doc,
    reason = "a test panics to fail, so a Panics section says nothing"
)]

mod common;

mod comments;
mod equivalence;
mod exprs;
mod fixtures;
mod format_options;
mod functions;
mod idempotency;
mod luau;
mod polish;
mod stmts;
mod synthetic;
mod tables;
