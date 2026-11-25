// Puts the shared helpers under clippy.toml's `allow-unwrap-in-tests`, which
// otherwise reaches only the `#[test]` functions of an integration test binary.
#![cfg(test)]

mod common;

mod ast_types_bitset;
mod checker;
mod counts;
mod nodes;
mod requires;
mod scoping;
mod stack;
mod tables;
