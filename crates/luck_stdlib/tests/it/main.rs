// Puts the shared helpers under clippy.toml's `allow-unwrap-in-tests`, which
// otherwise reaches only the `#[test]` functions of an integration test binary.
#![cfg(test)]

mod common;

mod analysis;
mod drift;
mod replace_template;
mod roblox_api;
mod roblox_definitions;
mod stdlib_catalog;
mod stdlib_definition;
mod variant;
