//! # `luck_semantic`
//!
//! Semantic analysis of Lua 5.1 to 5.5 and Luau, in one walk over a tree, as
//! `oxc_semantic` does it.
//!
//! [`SemanticBuilder::build`] walks a [`Chunk`](luck_ast::Chunk) once, in
//! source order, with its version's scoping rules, into a [`Semantic`]. That
//! always holds its [`Scoping`]: flat tables of scopes, symbols, references,
//! and labels, indexed by typed ids, and the chunk's distinct names, which it
//! owns, so a pass that rewrites the tree can keep it. A name in the tree
//! finds what it declares, refers to, or labels by its
//! [`NameIndex`](luck_ast::NameIndex), with [`Scoping::symbol_of`],
//! [`Scoping::reference_of`], and [`Scoping::label_of`], a reference its
//! symbol with [`Reference::resolved`], and a symbol its references with
//! [`Scoping::resolved_references`].
//!
//! Each other part is built only when the caller asks for it:
//!
//! - [`SemanticBuilder::with_check`] reports the errors Lua rejects a chunk
//!   for at compile time beyond its grammar, such as a write to a `<const>`
//!   local, a `goto` no label takes, or a function with more locals than its
//!   compiler holds.
//! - [`SemanticBuilder::with_nodes`] builds [`AstNodes`], every node of the
//!   tree in source preorder with its parent and scope, which symbols,
//!   references, labels, and scopes point into by [`NodeId`], which knows
//!   the [`AstTypesBitset`] of the node types it holds, and in which
//!   [`AstNodes::node_at`] finds the innermost node at an offset.
//! - [`SemanticBuilder::with_requires`] finds every [`RequireSite`], a call
//!   of the host's `require`.
//!
//! Nothing here knows any target's standard library, which `luck_stdlib`
//! reads a chunk's semantic against.
//!
//! # Usage
//!
//! ```
//! use luck_allocator::Allocator;
//! use luck_semantic::SemanticBuilder;
//! use luck_token::LuaVersion;
//!
//! let allocator = Allocator::new();
//! let source = b"local greeting <const> = 1 greeting = print(greeting)";
//! let parsed = luck_parser::parse(&allocator, source, LuaVersion::Lua54);
//! let chunk = allocator.alloc(parsed.chunk);
//! let built = SemanticBuilder::new().with_counts(parsed.counts).with_check(true).build(chunk);
//! let scoping = built.semantic.scoping();
//! let greeting = scoping.symbols()[0].id;
//! assert_eq!(scoping.resolved_references(greeting).len(), 2);
//! assert_eq!(built.errors[0].message, "attempt to assign to const variable `greeting`");
//! ```

mod ast_types_bitset;
mod builder;
mod checker;
mod id;
mod names;
mod node;
mod require_site;
mod scoping;
mod semantic;

pub use ast_types_bitset::AstTypesBitset;
pub use builder::{SemanticBuilder, SemanticBuilderReturn};
pub use id::{LabelId, NameId, NodeId, ReferenceId, ScopeId, SymbolId};
pub use node::{AstNode, AstNodes};
pub use require_site::RequireSite;
pub use scoping::{Label, Reference, ReferenceKind, Scope, ScopeKind, Scoping, Symbol, SymbolKind};
pub use semantic::Semantic;
