// Puts the shared helpers under clippy.toml's `allow-unwrap-in-tests`, which
// otherwise reaches only the `#[test]` functions of an integration test binary.
#![cfg(test)]

mod common;

mod depth;
mod errors;
mod fixtures;
mod lua51;
mod lua52;
mod lua53;
mod lua54;
mod lua55;
mod luau;
mod luau_types;
mod parse_result;
mod stack;
mod version;
