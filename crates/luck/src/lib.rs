//! # luck
//!
//! The whole luck toolchain behind one dependency, for build tools and
//! editors that would rather not track each `luck_*` crate. The package
//! publishes as `luck_lua`, but the library is named `luck`.
//!
//! The parse core is always present. Every other stage sits behind its own
//! feature, and `full` enables all of them:
//!
//! - `allocator` - The arena syntax trees live in, and arenas for threads
//!   (always on)
//! - `token` - Spans, `LuaVersion`, tokens, literal value semantics (always on)
//! - `lexer` - Tokenizer (always on)
//! - `ast` - AST nodes and their builder, `Visit`, `VisitMut`, `AstKind`,
//!   `GetSpan`, `ContentEq` (always on)
//! - `parser` - Pratt + recursive-descent parser (always on)
//! - `core` - Dialect targets, diagnostics (always on)
//! - `codegen` - Compact printer (feature `codegen`)
//! - `resolver` - Module resolution, source loading, path form (feature
//!   `resolver`)
//! - `bundler` - Dependency graph and lazy-loader bundling (feature `bundler`,
//!   enables `resolver` and `minifier`)
//! - `minifier` - AST transform pipeline (feature `minifier`)
//! - `formatter` - Prettier-style formatter (feature `formatter`)
//! - `semantic` - Scope tree + stdlib model (feature `semantic`)
//! - `linter` - Rule engine with auto-fixes (feature `linter`, enables
//!   `semantic`)
//! - `config` - `luck.json` and `.editorconfig` loading (feature `config`,
//!   enables the stages it configures)
//!
//! # Arenas
//!
//! A tree lives in an [`allocator::Allocator`] the caller owns, and borrows
//! the source it was parsed from, so both outlive it. Every stage takes the
//! arena first and builds what it needs there. Nothing in an arena is dropped
//! on its own: [`reset`](allocator::Allocator::reset) frees it all at once
//! and keeps the memory, so one arena serves file after file. Threads take
//! arenas from an [`allocator::AllocatorPool`], whose guard resets each one
//! and hands it back when it drops. The bundler reads modules on rayon's
//! pool, each into the arena its thread gets from an
//! [`allocator::ThreadAllocators`].
//!
//! # Usage
//!
//! Parse, then reuse the arena for the next file:
//!
//! ```
//! use luck::allocator::Allocator;
//! use luck::token::LuaVersion;
//!
//! let mut allocator = Allocator::new();
//! for source in ["local x = 1", "return x"] {
//!     let parsed = luck::parser::parse(&allocator, source, LuaVersion::Lua54);
//!     assert!(parsed.errors.is_empty());
//!     // The parse borrows the arena, so it goes first.
//!     drop(parsed);
//!     allocator.reset();
//! }
//! ```
//!
//! Share arenas across threads:
//!
//! ```
//! use std::thread;
//!
//! use luck::allocator::AllocatorPool;
//! use luck::token::LuaVersion;
//!
//! let pool = AllocatorPool::new(2);
//! thread::scope(|scope| {
//!     for source in ["local x = 1", "return 2"] {
//!         let pool = &pool;
//!         scope.spawn(move || {
//!             let allocator = pool.get();
//!             let parsed = luck::parser::parse(&allocator, source, LuaVersion::Lua54);
//!             assert!(parsed.errors.is_empty());
//!         });
//!     }
//! });
//! ```
//!
//! Format, with feature `formatter`. `formatter::format_parsed` and
//! `formatter::format_range` take a parse a caller already holds:
//!
//! ```
//! # #[cfg(feature = "formatter")] {
//! use luck::allocator::Allocator;
//! use luck::formatter::FormatOptions;
//! use luck::token::LuaVersion;
//!
//! let allocator = Allocator::new();
//! let formatted =
//!     luck::formatter::format(&allocator, "local x=1", LuaVersion::Lua54, &FormatOptions::default());
//! assert_eq!(formatted.output, "local x = 1\n");
//! # }
//! ```
//!
//! Minify, with feature `minifier`:
//!
//! ```
//! # #[cfg(feature = "minifier")] {
//! use luck::allocator::Allocator;
//! use luck::minifier::TransformConfig;
//! use luck::token::LuaVersion;
//!
//! let allocator = Allocator::new();
//! let minified = luck::minifier::minify(
//!     &allocator,
//!     "return 1 + 2",
//!     LuaVersion::Lua54,
//!     &TransformConfig::default(),
//! );
//! assert_eq!(minified.as_deref(), Ok("return 3"));
//! # }
//! ```
//!
//! Lint, with feature `linter`. `linter::lint_parsed` lints a parse a caller
//! already holds, and `linter::fix` applies the fixes of the kinds asked for
//! until none is left, returning the fixed source's parse and diagnostics:
//!
//! ```
//! # #[cfg(feature = "linter")] {
//! use luck::allocator::Allocator;
//! use luck::core::LuaTarget;
//! use luck::linter::{FixKind, LintConfig};
//!
//! let allocator = Allocator::new();
//! let config = LintConfig::default();
//! let source = "print(undefined_name)";
//! let diags = luck::linter::lint(&allocator, source, LuaTarget::Lua54, &config, FixKind::empty());
//! assert_eq!(diags[0].rule, "undefined_variable");
//!
//! let fixed = luck::linter::fix(&allocator, "unpack(t)", LuaTarget::Lua54, &config, FixKind::Fix);
//! assert_eq!(fixed.map(|fixed| fixed.parse.source), Ok("table.unpack(t)"));
//! # }
//! ```
//!
//! Print a tree back compactly, with feature `codegen`:
//!
//! ```
//! # #[cfg(feature = "codegen")] {
//! use luck::allocator::Allocator;
//! use luck::token::LuaVersion;
//!
//! let allocator = Allocator::new();
//! let parsed = luck::parser::parse(&allocator, "local x = ( 1 )", LuaVersion::Lua54);
//! assert_eq!(luck::codegen::compact(&parsed.chunk), "local x=(1)");
//! # }
//! ```
//!
//! Bundle, with feature `bundler`. Modules are read on rayon's pool, so give
//! its threads [`ast::MIN_STACK_BYTES`] of stack. The result borrows no arena,
//! so they can be reset or dropped right after:
//!
//! ```no_run
//! # #[cfg(feature = "bundler")] {
//! use std::path::Path;
//!
//! use luck::allocator::ThreadAllocators;
//! use luck::bundler::{BundleOptions, DynamicRequire};
//! use luck::token::LuaVersion;
//!
//! let options = BundleOptions {
//!     version: LuaVersion::Lua54,
//!     search_paths: &[],
//!     project_root: Path::new("src"),
//!     dynamic_require: DynamicRequire::default(),
//! };
//! let arenas = ThreadAllocators::new();
//! let bundle = luck::bundler::bundle(&arenas, Path::new("src/main.lua"), &options).unwrap();
//! println!("{}", bundle.output);
//! # }
//! ```

mod version;

pub use luck_allocator as allocator;
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
