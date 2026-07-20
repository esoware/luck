//! # luck_cli
//!
//! Command-line interface for the luck bundler and minifier.
//!
//! Subcommands: `init`, `build`, `check`, `lint`, `fmt`, `lsp`, and per-target
//! `bundle`/`minify`/`graph`. Built with [`clap`]. The parse surface and
//! dispatch live in [`args`]; each subcommand's arguments and handler live in
//! its own module under [`commands`].

mod args;
mod commands;
mod minify_flags;
mod output;
mod project;
mod render;

pub use args::run;

/// Exit codes. 0 = success; 1 = the operation ran but found problems
/// (diagnostics, lint findings, parse/build failure); 2 = usage/config error
/// (bad args, missing/invalid config, path not found).
pub const EXIT_SUCCESS: u8 = 0;
pub const EXIT_FAILURE: u8 = 1;
pub const EXIT_USAGE: u8 = 2;

/// Controls how much output the CLI prints during builds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Verbosity {
    Quiet,
    Normal,
}
