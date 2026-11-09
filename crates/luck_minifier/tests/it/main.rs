// Puts the shared helpers under clippy.toml's allow-*-in-tests settings, which
// otherwise reach only `#[test]` functions in an integration test binary.
#![cfg(test)]
#![expect(
    clippy::missing_panics_doc,
    reason = "a test panics to fail, so a Panics section says nothing"
)]

mod common;

mod correctness;
mod dialects;
mod output;
mod passes;
mod renaming;
