//! # `luck_lsp`
//!
//! Language Server Protocol implementation for the luck toolchain.
//!
//! Capabilities:
//! - text document sync (open, change, close)
//! - lint diagnostics on open and change, backed by [`luck_linter`], each
//!   `require` that doesn't resolve, as `luck build` reports it, and type
//!   errors from Luau's own checker, [`luck_type_checker`]
//! - formatting + range formatting, backed by [`luck_formatter`]
//! - hover, completion, and signature help from Luau's types, with the
//!   documentation a definition file has, go-to-definition, references,
//!   rename, document and workspace symbols, code actions (per-diagnostic
//!   fixes, `source.fixAll.luck`, and rule-disable comments), semantic
//!   tokens, document highlights, folding ranges, selection ranges, and
//!   document links
//! - definition, references, and rename of a module's members across the
//!   workspace folders, and definition into the definition files, Luau's
//!   and Roblox's as `luck:` documents, served through
//!   `workspace/textDocumentContent`
//! - custom requests: `luck/syntaxTree`, `luck/fixAllWorkspace`, and the
//!   custom notification `luck/didChangeFiles`
//! - positions in UTF-8, UTF-16, or UTF-32, whichever `positionEncoding`
//!   settles
//!
//! One main loop takes the client's messages in order and touches no tree.
//! Each version of an open document is parsed and analyzed once, in an arena
//! of its own, by the first job on the worker pool that needs it, and every
//! request runs on that pool against the version current when it arrived.
//! A version an edit or a close replaces stops its lint, and a request the
//! client cancels before it starts never runs. The session keeps one
//! resolver, which the client's file watcher clears.
//!
//! Each change lints every open document on a job of its own, and starts a
//! generation, which checks them all through the checker of each project's
//! definitions, behind a lock, with the open documents' text in place of
//! their files, and cancels the checks of the last. A document's lints go out
//! as soon as they're done, with the type errors of the last check until the
//! new one's are, and nothing goes out that's what went out last.
//!
//! A request at a position finds the innermost node there in the analysis's
//! [`luck_semantic::AstNodes`] and walks up through its parents, and a name
//! finds the local it declares or refers to by its symbol and reference.
//!
//! The workspace is the client's folders. Each file's record holds what the
//! other files see of it: the modules it requires, through the semantic
//! analysis's `require` sites and the session's resolver, and where it names
//! a member of a module's table, the one it returns or one it requires. An
//! open document's record comes from its analysis, and every other file's
//! is read from its files once, as idle workers walk the folders ahead of the
//! first request that needs them, until the file watcher says it changed. A
//! cross-file request reads every record, between which it stops once the
//! client cancels it.
//!
//! Every source comes through a file system, the disk, or once the client
//! gives a config as its `initializationOptions`, the files the client holds,
//! its open documents and the ones it pushes with `luck/didChangeFiles`, by
//! paths made of their URIs. A `.d.luau` file is a definition file, which
//! parses as one and is never checked as a module, and which a project that
//! lists it reads as its open document holds it.
//!
//! [`serve_stdio`] serves over stdio, as `luck lsp` does, and [`serve`] over
//! any [`Connection`], such as one `Connection::listen` makes over TCP.
//!
//! # Usage
//!
//! The server runs every job on the workers it starts, as many as its caller
//! asks for, each with [`luck_ast::MIN_STACK_BYTES`] of stack.

mod analyzed;
mod capabilities;
mod definitions;
mod document;
mod files;
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
mod type_check;
mod workspace;

pub use lsp_server::Connection;
pub use server::serve;
pub use transport::serve_stdio;
