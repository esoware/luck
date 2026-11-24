// Puts the shared helpers under clippy.toml's `allow-unwrap-in-tests`, which
// otherwise reaches only the `#[test]` functions of an integration test binary.
#![cfg(test)]

mod common;

mod constant_evaluation;
mod is_literal_value;
mod may_have_side_effects;
mod to_boolean;
