//! # luck
//!
//! The whole luck toolchain behind one dependency, for build tools and
//! editors that would rather not track each `luck_*` crate. The package
//! publishes as `luck_lua`, but the library is named `luck`.
//!
//! The parse core is always present. Every other stage sits behind its own
//! feature, and `full` enables all of them:
//!
//! - `token` - Spans, `LuaVersion`, tokens, literal value semantics (always on)
//! - `lexer` - Tokenizer (always on)
//! - `ast` - AST nodes, `Visitor`, `Transform`, `span()` accessors (always on)
//! - `parser` - Pratt + recursive-descent parser (always on)
//! - `core` - Dialect targets, diagnostics, path helpers (always on)
//! - `codegen` - Compact printer (feature `codegen`)
//! - `resolver` - Module resolution (feature `resolver`)
//! - `bundler` - Dependency graph and lazy-loader bundling (feature `bundler`,
//!   enables `resolver`)
//! - `minifier` - AST transform pipeline (feature `minifier`)
//! - `formatter` - Prettier-style formatter (feature `formatter`)
//! - `semantic` - Scope tree + stdlib model (feature `semantic`)
//! - `linter` - Rule engine with auto-fixes (feature `linter`, enables
//!   `semantic`)
//! - `config` - `luck.json` and `.editorconfig` loading (feature `config`,
//!   enables the stages it configures)
//!
//! # Usage
//!
//! ```
//! use luck::token::LuaVersion;
//!
//! let parsed = luck::parser::parse("local x = 1", LuaVersion::Lua54);
//! assert!(parsed.errors.is_empty());
//! ```

mod version;

pub use luck_ast as ast;
#[cfg(feature = "bundler")]
pub use luck_bundler as bundler;
#[cfg(feature = "codegen")]
pub use luck_codegen as codegen;
#[cfg(feature = "config")]
pub use luck_config as config;
pub use luck_core as core;
#[cfg(feature = "formatter")]
pub use luck_formatter as formatter;
pub use luck_lexer as lexer;
#[cfg(feature = "linter")]
pub use luck_linter as linter;
#[cfg(feature = "minifier")]
pub use luck_minifier as minifier;
pub use luck_parser as parser;
#[cfg(feature = "resolver")]
pub use luck_resolver as resolver;
#[cfg(feature = "semantic")]
pub use luck_semantic as semantic;
pub use luck_token as token;

pub use version::VERSION;
