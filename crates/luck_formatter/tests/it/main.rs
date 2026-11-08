#![expect(
    clippy::missing_panics_doc,
    reason = "a test panics to fail, so a Panics section says nothing"
)]

mod common;

mod comments;
mod exprs;
mod fixtures;
mod functions;
mod idempotency;
mod luau;
mod polish;
mod stmts;
mod synthetic;
mod tables;
