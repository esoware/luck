//! # `luck_lsp`
//!
//! Language Server Protocol implementation for the luck toolchain.
//!
//! Capabilities:
//! - text document sync (open, change, save, close)
//! - lint diagnostics on open, change, and save, backed by [`luck_linter`]
//! - formatting + range formatting, backed by [`luck_formatter`]
//! - hover, completion, signature help, go-to-definition, references,
//!   rename, document and workspace symbols, code actions (per-diagnostic
//!   fixes, `source.fixAll.luck`, and rule-disable comments), semantic
//!   tokens, document highlights, folding ranges, selection ranges, and
//!   document links
//! - custom requests: `luck/syntaxTree`, `luck/fixAllWorkspace`
//!
//! Each open document is parsed and analyzed once per version, in an arena
//! of its own that is freed when the version is replaced or closed, and
//! every request reads that version's tree and analysis.
//!
//! `luck lsp` serves it over stdio, or over TCP with `--socket <port>` for
//! debugging.
//!
//! # Usage
//!
//! ```no_run
//! let runtime = tokio::runtime::Runtime::new()?;
//! runtime.block_on(luck_lsp::serve_stdio());
//! # Ok::<(), std::io::Error>(())
//! ```

#![cfg_attr(
    test,
    expect(
        clippy::missing_panics_doc,
        reason = "a test panics to fail, so a Panics section says nothing"
    )
)]

extern crate alloc;

mod analyzed;
mod backend;
mod document;
#[cfg(test)]
mod fixtures;
mod line_index;
mod providers;
mod settings;
mod transport;

pub use backend::{Backend, Notifier};
pub use transport::{serve_socket, serve_stdio};
