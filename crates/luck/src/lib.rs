//! # luck
//!
//! The whole luck toolchain behind one dependency, for build tools and
//! editors that would rather not track each `luck_*` crate. The package
//! publishes as `luck_luau`, but the library is named `luck`.
//!
//! The parse core is always present. Every other stage sits behind its own
//! feature, and `full` enables all of them:
//!
//! - `allocator` - The arena syntax trees live in, and arenas for threads
//!   (always on)
//! - `token` - Spans, tokens, literal value semantics (always on)
//! - `lexer` - Tokenizer (always on)
//! - `ast` - AST nodes and their builder, `Visit`, `VisitMut`, `AstKind`,
//!   `GetSpan`, `ContentEq` (always on)
//! - `parser` - Pratt + recursive-descent parser (always on)
//! - `diagnostics` - The diagnostic every stage reports problems with, and
//!   its numbered constructors (always on)
//! - `codegen` - Compact printer (feature `codegen`)
//! - `resolver` - Module resolution, source loading, path form (feature
//!   `resolver`)
//! - `bundler` - Dependency graph and lazy-loader bundling (feature `bundler`,
//!   enables `resolver` and `minifier`)
//! - `minifier` - AST transform pipeline (feature `minifier`)
//! - `formatter` - Prettier-style formatter (feature `formatter`)
//! - `semantic` - Scoping, compile-time checks, the node table, and `require`
//!   sites, in one pass (feature `semantic`)
//! - `stdlib` - The standard library from Luau definition files, Luau's with
//!   Roblox's API or a host's definitions over it, and what a chunk reaches
//!   in it (feature `stdlib`, enables `semantic`)
//! - `linter` - Rule engine with auto-fixes (feature `linter`, enables
//!   `stdlib`)
//! - `type_checker` - Luau's own type checker, its C++ Analysis library
//!   (feature `type_checker`)
//! - `config` - `luck.json` and `.editorconfig` loading (feature `config`,
//!   enables the stages it configures)
//!
//! # Arenas
//!
//! A tree lives in an [`allocator::Allocator`] the caller owns, and borrows
//! the source it was parsed from, so both outlive it. Every stage that builds
//! nodes takes the arena first and builds them there. Nothing in an arena is
//! dropped on its own: [`reset`](allocator::Allocator::reset) frees it all at
//! once and keeps the memory, so one arena serves file after file. Threads
//! take arenas from an [`allocator::AllocatorPool`], whose guard resets each
//! one and hands it back when it drops. The bundler reads modules on rayon's
//! pool, each into the arena its thread gets from an
//! [`allocator::ThreadAllocators`].
//!
//! # Usage
//!
//! Parse, then reuse the arena for the next file:
//!
//! ```
//! use luck::allocator::Allocator;
//!
//! let mut allocator = Allocator::new();
//! for source in [&b"local x = 1"[..], b"return x"] {
//!     let parsed = luck::parser::parse(&allocator, source);
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
//!
//! let pool = AllocatorPool::new(2);
//! thread::scope(|scope| {
//!     for source in [&b"local x = 1"[..], b"return 2"] {
//!         let pool = &pool;
//!         scope.spawn(move || {
//!             let allocator = pool.get();
//!             let parsed = luck::parser::parse(&allocator, source);
//!             assert!(parsed.errors.is_empty());
//!         });
//!     }
//! });
//! ```
//!
//! Format, with feature `formatter`. `formatter::format_parsed` and
//! `formatter::format_range` take a parse a caller already holds, made with
//! `parser::parse_with_tokens`:
//!
//! ```
//! # #[cfg(feature = "formatter")] {
//! use luck::allocator::Allocator;
//! use luck::formatter::FormatOptions;
//!
//! let allocator = Allocator::new();
//! let formatted = luck::formatter::format(&allocator, b"local x=1", &FormatOptions::default());
//! assert_eq!(formatted.output, b"local x = 1\n");
//! # }
//! ```
//!
//! Minify, with feature `minifier`:
//!
//! ```
//! # #[cfg(feature = "minifier")] {
//! use luck::allocator::Allocator;
//! use luck::minifier::TransformConfig;
//!
//! let allocator = Allocator::new();
//! let minified = luck::minifier::minify(&allocator, b"return 1 + 2", &TransformConfig::default());
//! assert_eq!(minified.as_deref(), Ok(&b"return 3"[..]));
//! # }
//! ```
//!
//! Lint, with feature `linter`, against a standard library. A
//! `linter::Linter` compiles its settings once and lints parse after parse,
//! and its `fix` applies the fixes of the kinds asked for until none is left,
//! returning the fixed source's parse and diagnostics:
//!
//! ```
//! # #[cfg(feature = "linter")] {
//! use luck::allocator::Allocator;
//! use luck::linter::{FixKind, LintConfig, Linter};
//!
//! let allocator = Allocator::new();
//! let linter = Linter::new(LintConfig::default());
//! let stdlib = luck::stdlib::luau_library();
//! let parse = luck::parser::parse(&allocator, b"print(undefined_name)");
//! let diags = linter.run(&parse, stdlib, FixKind::empty(), None);
//! assert_eq!(diags[0].code, "undefined_variable");
//!
//! let parse = luck::parser::parse(&allocator, b"local unused = 1");
//! let fixed = linter.fix(&allocator, parse, stdlib, FixKind::all(), None);
//! assert_eq!(fixed.map(|fixed| fixed.parse.source), Ok(&b"local _unused = 1"[..]));
//! # }
//! ```
//!
//! Print a tree back compactly, with feature `codegen`:
//!
//! ```
//! # #[cfg(feature = "codegen")] {
//! use luck::allocator::Allocator;
//! use luck::codegen::LiteralSpelling;
//!
//! let allocator = Allocator::new();
//! let parsed = luck::parser::parse(&allocator, b"local x = ( a + b ) * ( c )");
//! let output = luck::codegen::compact(&parsed.chunk, LiteralSpelling::AsWritten);
//! assert_eq!(output, b"local x=(a+b)*c");
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
//!
//! let options = BundleOptions {
//!     project_root: Path::new("src"),
//!     dynamic_require: DynamicRequire::default(),
//!     luau_config: None,
//! };
//! let arenas = ThreadAllocators::new();
//! let bundle = luck::bundler::bundle(&arenas, Path::new("src/main.luau"), &options).unwrap();
//! std::io::Write::write_all(&mut std::io::stdout(), &bundle.output).unwrap();
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
pub use luck_diagnostics as diagnostics;
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
#[cfg(feature = "stdlib")]
pub use luck_stdlib as stdlib;
pub use luck_token as token;
#[cfg(feature = "type_checker")]
pub use luck_type_checker as type_checker;

pub use version::VERSION;
