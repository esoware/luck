//! # `luck_stdlib`
//!
//! The standard library of each target luck reads, Lua 5.1 to 5.5, Luau, and
//! Roblox, the std definitions that layer a host's library over one, and
//! what a chunk's names and calls reach in it.
//!
//! [`library_for`] returns a target's built-in [`StdlibLibrary`], a static
//! library with signatures, overloads, value shapes, and deprecations, and
//! [`is_preloaded_module`] whether a `require` returns one of its libraries.
//! A [`StdlibDefinition`] is one std definition file, which describes a
//! library such as LOVE's or Neovim's in the format the built-in libraries
//! are written in, and [`Stdlib::layer`] layers it over a library. [`analyze`]
//! builds a chunk's `luck_semantic::Semantic` and reads it against a
//! library, so rules can ask [`SemanticAnalysis::is_known_global`] whether a
//! name is a stdlib or configured global and resolve calls to stdlib
//! entries, whichever library it is.
//!
//! # Usage
//!
//! ```
//! use luck_allocator::Allocator;
//! use luck_stdlib::{Stdlib, StdlibDefinition};
//! use luck_token::LuaTarget;
//!
//! let definition = StdlibDefinition::parse(
//!     r#"
//! [globals.love]
//! kind = "namespace"
//!
//! [globals.love.members.load]
//! kind = "function"
//! is_read_only = false
//! "#,
//! )
//! .expect("a valid std definition");
//! let stdlib = Stdlib::new(LuaTarget::Lua51).layer(&definition).expect("it layers");
//!
//! let allocator = Allocator::new();
//! let source = b"local greeting = 1 print(greeting, love)";
//! let parsed = luck_parser::parse(&allocator, source, stdlib.target().version());
//! let chunk = allocator.alloc(parsed.chunk);
//! let (analysis, errors) = luck_stdlib::analyze(chunk, parsed.counts, stdlib.library());
//! assert!(errors.is_empty());
//! let scoping = analysis.scoping();
//! let greeting = scoping.symbols()[0].id;
//! assert_eq!(scoping.resolved_references(greeting).len(), 1);
//! assert!(analysis.is_known_global("print"));
//! assert!(analysis.is_known_global("love"));
//! ```

mod analysis;
#[cfg(test)]
mod fixtures;
mod resolve;
mod stdlib;
mod stdlib_catalog;
mod stdlib_definition;
mod stdlib_model;

pub use analysis::{SemanticAnalysis, analyze};
pub use resolve::{ChainCallees, ResolvedCallee};
pub use stdlib::Stdlib;
pub use stdlib_catalog::{is_preloaded_module, library_for};
pub use stdlib_definition::StdlibDefinition;
pub use stdlib_model::{
    StdlibArgKind, StdlibConstant, StdlibDeprecation, StdlibEntry, StdlibFunction, StdlibLibrary,
    StdlibMap, StdlibNamespace, StdlibParam, StdlibReplacement, StdlibShape, StdlibSignature,
    StdlibValue, expand_replace_template,
};
