//! # `luck_cli`
//!
//! The `luck` command-line interface: `init`, `build`, `bundle`, `minify`,
//! `graph`, `lint`, `fmt`, `check`, and `lsp`.
//!
//! `cli` holds the [`clap`] model's global flags, and `commands` the
//! subcommands and their dispatch, with each command's arguments and handler
//! in a module of its own.

#![expect(
    clippy::print_stderr,
    clippy::print_stdout,
    reason = "a CLI prints, and println! panicking on a closed pipe is accepted"
)]

mod cli;
mod commands;
mod exit;
mod minify_flags;
mod output;
mod project;
mod reporter;
mod thread_pool;
mod verbosity;

use std::process::ExitCode;
use std::{panic, thread};

use luck_ast::MIN_STACK_BYTES;

use crate::exit::operation_error;

#[cfg(not(any(target_arch = "arm", target_arch = "riscv64", target_family = "wasm")))]
#[global_allocator]
static GLOBAL: mimalloc_safe::MiMalloc = mimalloc_safe::MiMalloc;

/// A main thread gets the platform's stack, 1 MiB on Windows, so the command
/// runs on a thread with what every stage needs.
fn main() -> ExitCode {
    match thread::Builder::new().stack_size(MIN_STACK_BYTES).spawn(cli::run) {
        Ok(worker) => worker.join().unwrap_or_else(|payload| panic::resume_unwind(payload)),
        Err(e) => operation_error(format_args!("can't start a thread: {e}")).into(),
    }
}
