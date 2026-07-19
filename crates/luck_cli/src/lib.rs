//! # luck_cli
//!
//! Command-line interface for the luck bundler and minifier.
//!
//! Subcommands: `init`, `build`, `check`, `lint`, `fmt`, `lsp`, and per-target
//! `bundle`/`minify`/`graph`.
//! Built with [`clap`].

mod cli;
mod render;

pub use cli::{EXIT_FAILURE, EXIT_SUCCESS, EXIT_USAGE, run};
