// Puts the shared helpers under clippy.toml's `allow-unwrap-in-tests`, which
// otherwise reaches only the `#[test]` functions of an integration test binary.
#![cfg(test)]

mod common;

mod cancel;
mod check;
mod definitions;
mod describe;
mod invalidate;
mod luau_config;
