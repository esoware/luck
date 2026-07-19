//! # luck
//!
//! Facade crate: the complete luck toolchain as a single dependency.
//! Embedders (build tools, editors) depend
//! on this one crate instead of tracking fourteen.
//!
//! - [`token`] - Spans, `LuaVersion`, tokens, literal value semantics
//! - [`lexer`] - Tokenizer
//! - [`ast`] - AST nodes, `Visitor`, `AstTransform`, `span()` queries
//! - [`parser`] - Pratt + recursive-descent parser
//! - [`codegen`] - Compact printer
//! - [`core`] - Shared types, config, diagnostics
//! - [`resolver`] - Module resolution
//! - [`bundler`] - Dependency graph and lazy-loader bundling
//! - [`minifier`] - AST transform pipeline
//! - [`formatter`] - Prettier-style formatter
//! - [`semantic`] - Scope tree + stdlib model
//! - [`linter`] - Rule engine with `--fix`
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
pub use luck_bundler as bundler;
pub use luck_codegen as codegen;
pub use luck_core as core;
pub use luck_formatter as formatter;
pub use luck_lexer as lexer;
pub use luck_linter as linter;
pub use luck_minifier as minifier;
pub use luck_parser as parser;
pub use luck_resolver as resolver;
pub use luck_semantic as semantic;
pub use luck_token as token;

pub const VERSION: &str = env!("CARGO_PKG_VERSION");
