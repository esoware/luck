//! # `luck_lsp`
//!
//! Language Server Protocol implementation for the luck toolchain.
//!
//! Capabilities:
//! - text document sync (open, change, close)
//! - lint diagnostics on open and change, backed by [`luck_linter`]
//! - formatting + range formatting, backed by [`luck_formatter`]
//! - hover, completion, signature help, go-to-definition, references,
//!   rename, document and workspace symbols, code actions (per-diagnostic
//!   fixes, `source.fixAll.luck`, and rule-disable comments), semantic
//!   tokens, document highlights, folding ranges, selection ranges, and
//!   document links
//! - custom requests: `luck/syntaxTree`, `luck/fixAllWorkspace`
//! - positions in UTF-8, UTF-16, or UTF-32, whichever `positionEncoding`
//!   settles
//!
//! One main loop takes the client's messages in order and touches no tree.
//! Each version of an open document is parsed and analyzed once, in an arena
//! of its own, by the first job on the worker pool that needs it, and every
//! request runs on that pool against the version current when it arrived.
//! A version an edit or a close replaces stops its lint, and a request the
//! client cancels before it starts never runs. Each project keeps one
//! resolver per scheme, which the client's file watcher clears.
//!
//! A request at a position finds the innermost node there in the analysis's
//! [`luck_semantic::AstNodes`] and walks up through its parents, and a name
//! finds the local it declares or refers to by its symbol and reference.
//!
//! [`serve_stdio`] serves over stdio, as `luck lsp` does, [`serve_socket`]
//! over TCP for debugging, and [`serve`] over any `lsp_server` connection.
//!
//! # Usage
//!
//! The server starts its own workers with [`luck_ast::MIN_STACK_BYTES`] of
//! stack. A large file's lint runs on rayon's global pool, so a host builds
//! that pool with `ThreadPoolBuilder::stack_size(MIN_STACK_BYTES)` first.

#![cfg_attr(
    test,
    expect(
        clippy::missing_panics_doc,
        reason = "a test panics to fail, so a Panics section says nothing"
    )
)]

extern crate alloc;

mod analyzed;
mod document;
#[cfg(test)]
mod fixtures;
mod line_index;
mod pool;
mod providers;
mod server;
mod session;
mod settings;
mod transport;

pub use server::serve;
pub use transport::{serve_socket, serve_stdio};
