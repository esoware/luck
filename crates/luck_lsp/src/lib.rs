//! # luck_lsp
//!
//! Language Server Protocol implementation for the luck toolchain.
//!
//! Capabilities:
//! - text document sync (open, change, save, close)
//! - lint diagnostics on every change, backed by [`luck_linter`]
//! - formatting + range formatting, backed by [`luck_formatter`]
//! - hover, completion, signature help, document symbols,
//!   code actions (per-fix + source.fixAll.luck + rule-disable
//!   comments), semantic tokens, document highlights,
//!   folding ranges, selection ranges, document links
//! - custom requests: `luck/syntaxTree`, `luck/fixAllWorkspace`
//!
//! The LSP is exposed via `luck lsp`, which talks LSP over either stdio
//! (default) or TCP (`--socket <port>`, useful for editor integration during
//! development).
//!
//! # Usage
//!
//! ```
//! use luck_lsp::LineIndex;
//! use tower_lsp::lsp_types::Position;
//!
//! let index = LineIndex::new("local x\nlocal y");
//! assert_eq!(index.position("local x\nlocal y", 8), Position { line: 1, character: 0 });
//! ```

pub mod backend;
pub mod config;
pub mod diagnostics;
pub mod line_index;
pub mod providers;
pub mod serve;

pub use backend::{Backend, CapturedNotifier, DocumentState, Notifier, build_service};
pub use line_index::LineIndex;
pub use serve::{serve_socket, serve_stdio};
