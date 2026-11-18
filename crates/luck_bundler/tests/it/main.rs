// Lets clippy.toml's allow-*-in-tests settings cover the shared helpers, not
// just the `#[test]` functions.
#![cfg(test)]

mod common;

mod bugs;
mod bundle;
mod depth;
mod stack;
