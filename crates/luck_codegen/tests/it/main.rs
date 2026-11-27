// Puts the shared helpers under clippy.toml's `allow-unwrap-in-tests`, which
// otherwise reaches only the `#[test]` functions of an integration test binary.
#![cfg(test)]

mod common;

mod compact;
mod definitions;
mod fixtures;
mod generated;
mod literal;
mod separator;
mod stack;
