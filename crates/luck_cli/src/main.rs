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
use std::thread::Builder;

use crate::exit::Exit;

#[cfg(not(any(target_arch = "arm", target_arch = "riscv64", target_family = "wasm")))]
#[global_allocator]
static GLOBAL: mimalloc_safe::MiMalloc = mimalloc_safe::MiMalloc;

/// Parsing and every AST pass recurse, so deeply nested sources can outgrow
/// the main thread's stack, which is only 1 MB on Windows.
const WORKER_STACK_BYTES: usize = 16 * 1024 * 1024;

/// # Panics
///
/// Panics if the OS can't start the worker thread.
fn main() -> ExitCode {
    #[expect(clippy::expect_used, reason = "the CLI can't run without it")]
    let thread = Builder::new()
        .stack_size(WORKER_STACK_BYTES)
        .spawn(cli::run)
        .expect("failed to spawn worker thread");

    // A panic in the worker exits with `Exit::Failure`.
    thread.join().unwrap_or_else(|_| ExitCode::from(Exit::Failure))
}
