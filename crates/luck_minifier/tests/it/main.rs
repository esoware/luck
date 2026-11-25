// Puts the shared helpers under clippy.toml's `allow-unwrap-in-tests`, which
// otherwise reaches only the `#[test]` functions of an integration test binary.
#![cfg(test)]

mod common;

mod correctness;
mod dialects;
mod generated;
mod output;
mod passes;
mod renaming;
mod stack;
