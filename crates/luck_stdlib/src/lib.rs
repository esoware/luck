//! # `luck_stdlib`
//!
//! The standard library of each target luck reads, Lua 5.1 to 5.5, Luau, and
//! Roblox, and what a chunk's names and calls reach in it.
//!
//! [`library_for`] returns a target's [`StdlibLibrary`], a static catalog
//! built into the crate, with signatures, overloads, value shapes, and
//! deprecations. [`analyze`] builds a chunk's `luck_semantic::Semantic` and
//! reads it against one, so rules can ask
//! [`SemanticAnalysis::is_known_global`] whether a name is a stdlib or
//! configured global and resolve calls to stdlib entries.
//!
//! # Usage
//!
//! ```
//! use luck_allocator::Allocator;
//! use luck_token::LuaTarget;
//!
//! let allocator = Allocator::new();
//! let target = LuaTarget::Lua54;
//! let source = b"local greeting = 1 print(greeting)";
//! let parsed = luck_parser::parse(&allocator, source, target.version());
//! let chunk = allocator.alloc(parsed.chunk);
//! let (analysis, errors) = luck_stdlib::analyze(chunk, parsed.counts, target);
//! assert!(errors.is_empty());
//! let scoping = analysis.scoping();
//! let greeting = scoping.symbols()[0].id;
//! assert_eq!(scoping.resolved_references(greeting).len(), 1);
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
mod stdlib_catalog;
mod stdlib_model;

pub use analysis::{SemanticAnalysis, analyze};
pub use resolve::{ChainCallees, ResolvedCallee};
pub use stdlib_catalog::library_for;
pub use stdlib_model::{
    StdlibArgKind, StdlibConstant, StdlibDeprecation, StdlibEntry, StdlibFunction, StdlibLibrary,
    StdlibMap, StdlibNamespace, StdlibParam, StdlibReplacement, StdlibShape, StdlibSignature,
    StdlibValue, expand_replace_template,
};
