//! # `luck_stdlib`
//!
//! A standard library built from Luau definition files: Luau's own, with the
//! definitions a project loads layered over it, such as Roblox's API or a
//! host application's, and what a chunk's names and calls reach in it.
//!
//! [`luau_library`] returns Luau's [`StdlibLibrary`], with signatures,
//! overloads, value shapes, and deprecations, which it builds from the
//! crate's definition files the first time a process asks for it, and
//! [`roblox_library`] Luau's with Roblox's API at a [`SecurityLevel`] over
//! it. A [`Stdlib`] is one a clone shares, which [`Stdlib::with_roblox`] and
//! [`Stdlib::layer`] layer Roblox's API and a definition file over.
//! [`analyze`] builds a chunk's `luck_semantic::Semantic` and reads it
//! against a library, so rules can ask [`SemanticAnalysis::is_known_global`]
//! whether a name is a stdlib or configured global and resolve calls to
//! stdlib entries, whichever library it is.
//!
//! [`roblox_definitions`] gives Roblox's API as the definitions a type
//! checker loads over Luau's built-in ones, as a script at a
//! [`SecurityLevel`] reaches it, derived from one committed definition file,
//! and [`luau_definitions`] the definition file [`luau_library`] is built
//! from.
//!
//! # Usage
//!
//! ```
//! use luck_allocator::Allocator;
//! use luck_stdlib::Stdlib;
//!
//! let stdlib = Stdlib::luau()
//!     .layer("declare app: { onStart: () -> () }")
//!     .expect("the definition parses");
//!
//! let allocator = Allocator::new();
//! let source = b"local greeting = 1 print(greeting, app)";
//! let parsed = luck_parser::parse(&allocator, source);
//! let chunk = allocator.alloc(parsed.chunk);
//! let (analysis, errors) = luck_stdlib::analyze(chunk, parsed.counts, stdlib.library());
//! assert!(errors.is_empty());
//! let scoping = analysis.scoping();
//! let greeting = scoping.symbols()[0].id;
//! assert_eq!(scoping.resolved_references(greeting).len(), 1);
//! assert!(analysis.is_known_global("print"));
//! assert!(analysis.is_known_global("app"));
//! ```

mod analysis;
mod definitions;
mod resolve;
mod security_level;
mod stdlib;
mod stdlib_map;
mod stdlib_model;
mod translate;

pub use analysis::{SemanticAnalysis, analyze};
pub use definitions::{luau_definitions, roblox_definitions};
pub use resolve::{ChainCallees, ResolvedCallee};
pub use security_level::SecurityLevel;
pub use stdlib::{Stdlib, luau_library, roblox_library};
pub use stdlib_map::StdlibMap;
pub use stdlib_model::{
    StdlibArgKind, StdlibDeprecation, StdlibEntry, StdlibFunction, StdlibLibrary, StdlibNamespace,
    StdlibParam, StdlibShape, StdlibSignature, StdlibValue,
};
