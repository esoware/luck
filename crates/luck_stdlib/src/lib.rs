//! # `luck_stdlib`
//!
//! The standard library of each variant, standalone Luau and Roblox, the std
//! definitions that layer a host's library over one, and what a chunk's names
//! and calls reach in it.
//!
//! [`library_for`] returns a variant's built-in [`StdlibLibrary`], a static
//! library with signatures, overloads, value shapes, and deprecations. A
//! [`StdlibDefinition`] is one std definition file, which describes a
//! library such as a host application's in the format the built-in libraries
//! are written in, and [`Stdlib::layer`] layers it over a library. [`analyze`]
//! builds a chunk's `luck_semantic::Semantic` and reads it against a
//! library, so rules can ask [`SemanticAnalysis::is_known_global`] whether a
//! name is a stdlib or configured global and resolve calls to stdlib
//! entries, whichever library it is.
//!
//! [`definitions_for`] gives the Luau definitions a type checker loads over
//! Luau's built-in ones for a variant: Roblox's API, as a script at a
//! [`SecurityLevel`] reaches it, derived from one committed definition file.
//!
//! # Usage
//!
//! ```
//! use luck_allocator::Allocator;
//! use luck_stdlib::{Stdlib, StdlibDefinition, Variant};
//!
//! let definition = StdlibDefinition::parse(
//!     r#"
//! [globals.app]
//! kind = "namespace"
//!
//! [globals.app.members.onStart]
//! kind = "function"
//! is_read_only = false
//! "#,
//! )
//! .expect("a valid std definition");
//! let stdlib = Stdlib::new(Variant::Standalone).layer(&definition).expect("it layers");
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
mod layering;
mod replace_template;
mod resolve;
mod security_level;
mod stdlib;
mod stdlib_catalog;
mod stdlib_definition;
mod stdlib_map;
mod stdlib_model;
mod variant;

pub use analysis::{SemanticAnalysis, analyze};
pub use definitions::definitions_for;
pub use replace_template::expand_replace_template;
pub use resolve::{ChainCallees, ResolvedCallee};
pub use security_level::SecurityLevel;
pub use stdlib::Stdlib;
pub use stdlib_catalog::library_for;
pub use stdlib_definition::StdlibDefinition;
pub use stdlib_map::StdlibMap;
pub use stdlib_model::{
    StdlibArgKind, StdlibConstant, StdlibDeprecation, StdlibEntry, StdlibFunction, StdlibLibrary,
    StdlibNamespace, StdlibParam, StdlibReplacement, StdlibShape, StdlibSignature, StdlibValue,
};
pub use variant::Variant;
