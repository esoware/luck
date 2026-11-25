// Puts the shared helpers under clippy.toml's `allow-unwrap-in-tests`, which
// otherwise reaches only the `#[test]` functions of an integration test binary.
#![cfg(test)]

mod common;

mod depth;
mod errors;
mod features;
mod fixtures;
mod grammar;
mod parse_result;
mod restrictions;
mod stack;
mod types;
