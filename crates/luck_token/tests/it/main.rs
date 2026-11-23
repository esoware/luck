#![expect(
    clippy::missing_panics_doc,
    reason = "a test panics to fail, so a Panics section says nothing"
)]

mod comment;
mod directive;
mod number;
mod op;
mod span;
mod string;
mod token;
mod version;
mod whitespace;
