//! # `luck_cli`
//!
//! The `luck` command-line interface: `init`, `build`, `bundle`, `minify`,
//! `graph`, `lint`, `fmt`, `check`, and `lsp`.
//!
//! `cli` holds the [`clap`] model and dispatch, and each command's arguments
//! and handler live in its own module under `commands`.

#![expect(
    clippy::print_stderr,
    clippy::print_stdout,
    reason = "a CLI prints, and println! panicking on a closed pipe is accepted"
)]
#![cfg_attr(
    test,
    expect(
        clippy::missing_panics_doc,
        reason = "a test panics to fail, so a Panics section says nothing"
    )
)]

extern crate alloc;

mod cli;
mod commands;
mod exit;
mod minify_flags;
mod output;
mod project;
mod render;
mod verbosity;

use std::process::ExitCode;

#[cfg(not(any(target_arch = "arm", target_arch = "riscv64", target_family = "wasm")))]
#[global_allocator]
static GLOBAL: mimalloc_safe::MiMalloc = mimalloc_safe::MiMalloc;

/// Every stage fits in 1 MB of stack, a Windows main thread's, however deep
/// the source nests, so each thread runs on the stack it starts with.
fn main() -> ExitCode {
    cli::run()
}
