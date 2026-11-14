#![expect(
    clippy::missing_panics_doc,
    reason = "a test panics to fail, so a Panics section says nothing"
)]

mod common;

mod comments;
mod interpolation;
mod numbers;
mod streaming;
mod strings;
mod tokens;
