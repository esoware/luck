//! # `luck_semantic`
//!
//! Scope analysis, symbol resolution, and stdlib definitions for Lua and
//! Luau.
//!
//! [`analyze`] builds a [`ScopeTree`] of declarations, references,
//! shadowing, and upvalue captures. The stdlib catalog is a
//! [`StdlibLibrary`] per environment. Rules read its entries through
//! [`SemanticAnalysis::stdlib`] and ask
//! [`SemanticAnalysis::is_known_global`] whether a name is a stdlib or
//! configured global.
//!
//! # Usage
//!
//! ```
//! use luck_token::LuaVersion;
//!
//! let parsed = luck_parser::parse("local greeting = 1", LuaVersion::Lua54);
//! let analysis = luck_semantic::analyze(&parsed.block, LuaVersion::Lua54);
//! assert!(analysis.is_known_global("print"));
//! ```

#![cfg_attr(
    test,
    expect(
        clippy::missing_panics_doc,
        reason = "a test panics to fail, so a Panics section says nothing"
    )
)]

extern crate alloc;

mod analysis;
mod builder;
mod resolve;
mod scope;
mod stdlib_load;
mod stdlib_model;

pub use analysis::{ResolvedCallee, SemanticAnalysis, analyze, analyze_with_environment};
pub use scope::{
    Reference, ReferenceId, ReferenceKind, Scope, ScopeId, ScopeKind, ScopeTree, Symbol, SymbolId,
    SymbolKind,
};
pub use stdlib_model::{
    StdlibArgKind, StdlibConstant, StdlibDeprecation, StdlibEntry, StdlibFunction, StdlibLibrary,
    StdlibNamespace, StdlibParam, StdlibShape, StdlibSignature, StdlibValue,
    expand_replace_template, library_for,
};
