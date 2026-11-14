// Lets clippy.toml's allow-*-in-tests settings cover the shared helpers, not
// just the `#[test]` functions.
#![cfg(test)]
#![expect(clippy::missing_panics_doc, reason = "a Panics section says nothing about a test")]

mod full_grammar;
mod roundtrip;
mod runtime_safe;
mod stack;
mod unprintable;
