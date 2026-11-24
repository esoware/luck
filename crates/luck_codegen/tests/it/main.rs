// Puts the shared helpers under clippy.toml's `allow-unwrap-in-tests`, which
// otherwise reaches only the `#[test]` functions of an integration test binary.
#![cfg(test)]

mod common;

mod compact;
mod fixtures;
mod generated;
mod separator;
mod stack;
