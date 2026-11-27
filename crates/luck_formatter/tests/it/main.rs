// Puts the shared helpers under clippy.toml's `allow-unwrap-in-tests`, which
// otherwise reaches only the `#[test]` functions of an integration test binary.
#![cfg(test)]

mod common;

mod comment_gaps;
mod comments;
mod definitions;
mod exprs;
mod fixtures;
mod functions;
mod idempotency;
mod polish;
mod stack;
mod stmts;
mod synthetic;
mod tables;
mod types;
