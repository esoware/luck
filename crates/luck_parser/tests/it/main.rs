// Lets clippy.toml's allow-*-in-tests settings cover the shared helpers, not
// just the `#[test]` functions.
#![cfg(test)]
#![expect(clippy::missing_panics_doc, reason = "a Panics section says nothing about a test")]

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
mod validate;
mod version;
