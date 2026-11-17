//! # `luck_semantic`
//!
//! Scope analysis, symbol resolution, and stdlib definitions for Lua and
//! Luau.
//!
//! [`ScopeTree::build`] walks a tree once into flat tables of scopes,
//! symbols, and references, indexed by typed ids, and the chunk's distinct
//! names, which it borrows from the tree. A name in the tree finds its
//! reference with [`ScopeTree::reference_of`], a reference its symbol with
//! [`Reference::resolved`], and a symbol its references with
//! [`ScopeTree::resolved_references`].
//!
//! [`analyze`] reads that tree against one environment's [`StdlibLibrary`],
//! a static catalog, so rules can ask [`SemanticAnalysis::is_known_global`]
//! whether a name is a stdlib or configured global and resolve calls to
//! stdlib entries.
//!
//! # Usage
//!
//! ```
//! use luck_allocator::Allocator;
//! use luck_token::{LuaVersion, StdlibEnvironment};
//!
//! let allocator = Allocator::new();
//! let parsed = luck_parser::parse(&allocator, "local greeting = 1 print(greeting)", LuaVersion::Lua54);
//! let block = allocator.alloc(parsed.block);
//! let analysis =
//!     luck_semantic::analyze(block, parsed.counts, LuaVersion::Lua54, StdlibEnvironment::Standalone);
//! let tree = analysis.scope_tree();
//! let greeting = tree.symbols()[0].id;
//! assert_eq!(tree.resolved_references(greeting).len(), 1);
//! assert!(analysis.is_known_global("print"));
//! ```

#![cfg_attr(
    test,
    expect(
        clippy::missing_panics_doc,
        reason = "a test panics to fail, so a Panics section says nothing"
    )
)]

mod analysis;
#[cfg(test)]
mod fixtures;
mod resolve;
mod scope;
mod sink;
mod stdlib_catalog;
mod stdlib_model;

pub use analysis::{SemanticAnalysis, analyze, analyze_with};
pub use resolve::{ChainCallees, ResolvedCallee};
pub use scope::{
    NameId, Reference, ReferenceId, ReferenceKind, Scope, ScopeId, ScopeKind, ScopeTree, Symbol,
    SymbolId, SymbolKind,
};
pub use sink::NodeSink;
pub use stdlib_catalog::library_for;
pub use stdlib_model::{
    StdlibArgKind, StdlibConstant, StdlibDeprecation, StdlibEntry, StdlibFunction, StdlibLibrary,
    StdlibMap, StdlibNamespace, StdlibParam, StdlibShape, StdlibSignature, StdlibValue,
    expand_replace_template,
};
