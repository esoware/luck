// Puts the shared helpers under clippy.toml's `allow-unwrap-in-tests`, which
// otherwise reaches only the `#[test]` functions of an integration test binary.
#![cfg(test)]

mod common;

mod config;
mod directives;
mod fixes;
mod idiomatic;
mod rules;
mod stack;
