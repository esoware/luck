// Puts the shared helpers under clippy.toml's `allow-unwrap-in-tests`, which
// otherwise reaches only the `#[test]` functions of an integration test binary.
#![cfg(test)]

mod common;

mod allocator;
mod boxed;
mod list;
mod pool;
mod thin_bytes;
mod thin_str;
mod tree;
