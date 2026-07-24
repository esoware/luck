//! # luck
//!
//! Facade crate: the complete luck toolchain as a single dependency.
//! Embedders (build tools, editors) depend on this one crate instead of
//! tracking each `luck_*` crate in the workspace.
//!
//! The parse core is always present; downstream stages are behind features,
//! and `full` enables all of them:
//!
//! - `token` - Spans, `LuaVersion`, tokens, literal value semantics (always on)
//! - `lexer` - Tokenizer (always on)
//! - `ast` - AST nodes, `Visitor`, `AstTransform`, `span()` queries (always on)
//! - `parser` - Pratt + recursive-descent parser (always on)
//! - `core` - Shared types, config, diagnostics (always on)
//! - `codegen` - Compact printer (feature `codegen`)
//! - `resolver` - Module resolution (feature `resolver`)
//! - `bundler` - Dependency graph and lazy-loader bundling (feature `bundler`)
//! - `minifier` - AST transform pipeline (feature `minifier`)
//! - `formatter` - Prettier-style formatter (feature `formatter`)
//! - `semantic` - Scope tree + stdlib model (feature `semantic`)
//! - `linter` - Rule engine with `--fix` (feature `linter`)
//!
//! # Usage
//!
//! ```
//! use luck::token::LuaVersion;
//!
//! let result = luck::parser::parse("local x = 1", LuaVersion::Lua54);
//! assert!(result.errors.is_empty());
//! ```

pub use luck_ast as ast;
pub use luck_core as core;
pub use luck_lexer as lexer;
pub use luck_parser as parser;
pub use luck_token as token;

#[cfg(feature = "bundler")]
pub use luck_bundler as bundler;
#[cfg(feature = "codegen")]
pub use luck_codegen as codegen;
#[cfg(feature = "formatter")]
pub use luck_formatter as formatter;
#[cfg(feature = "linter")]
pub use luck_linter as linter;
#[cfg(feature = "minifier")]
pub use luck_minifier as minifier;
#[cfg(feature = "resolver")]
pub use luck_resolver as resolver;
#[cfg(feature = "semantic")]
pub use luck_semantic as semantic;

pub const VERSION: &str = env!("CARGO_PKG_VERSION");
