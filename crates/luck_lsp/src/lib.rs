//! # `luck_lsp`
//!
//! Language Server Protocol implementation for the luck toolchain.
//!
//! Capabilities:
//! - text document sync (open, change, close)
//! - lint diagnostics on open and change, backed by [`luck_linter`], and
//!   each `require` that doesn't resolve, as `luck build` reports it
//! - formatting + range formatting, backed by [`luck_formatter`]
//! - hover, completion, signature help, go-to-definition, references,
//!   rename, document and workspace symbols, code actions (per-diagnostic
//!   fixes, `source.fixAll.luck`, and rule-disable comments), semantic
//!   tokens, document highlights, folding ranges, selection ranges, and
//!   document links
//! - definition, references, and rename of a module's members across the
//!   workspace folders
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
//! The workspace is the client's folders. Each file's record holds what the
//! other files see of it: the modules it requires, through the semantic
//! analysis's `require` sites and its project's resolver, and where it names
//! a member of a module's table, the one it returns or one it requires. An
//! open document's record comes from its analysis, and every other file's
//! is read from disk once, as idle workers walk the folders ahead of the
//! first request that needs them, until the file watcher says it changed. A
//! cross-file request reads every record, between which it stops once the
//! client cancels it.
//!
//! [`serve_stdio`] serves over stdio, as `luck lsp` does, [`serve_socket`]
//! over TCP for debugging, and [`serve`] over any `lsp_server` connection.
//!
//! # Usage
//!
//! The server runs every job on the workers it starts, as many as its caller
//! asks for, each with [`luck_ast::MIN_STACK_BYTES`] of stack.

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
mod module_record;
mod pool;
mod providers;
mod server;
mod session;
mod settings;
mod transport;
mod workspace;

pub use server::serve;
pub use transport::{serve_socket, serve_stdio};
