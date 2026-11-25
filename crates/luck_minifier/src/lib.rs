//! # `luck_minifier`
//!
//! Luau minification on the arena tree, in place, as `oxc_minifier` minifies
//! JavaScript.
//!
//! [`minify`] parses source into the caller's arena and [`minify_chunk`]
//! takes a tree. Both rewrite the tree in place and print it once, with
//! [`luck_codegen::compact_into`].
//!
//! The work runs in three phases:
//!
//! - **Type stripping**, a tree's annotations, casts, and `type`
//!   declarations gone before anything else reads it.
//! - **Compression**, which reads the tree's bindings from `luck_semantic`'s
//!   scoping and runs passes of an in-place walk until one changes nothing:
//!   constant folding, dead-code and dead-local removal, inlining single-use
//!   locals, merging neighboring locals, `if` and `return` simplification,
//!   and shorter indexes and parentheses. A rewrite that drops code marks
//!   the references it held, so every decision reads the tree as it
//!   stands. Then each function's declarations lift to one `local` at its
//!   top, and the passes run again on what that leaves.
//! - **Renaming**, which gives locals the shortest names their scopes allow,
//!   from a fresh analysis of the tree and an alphabet in the order its
//!   characters are frequent in minified Luau, as `oxc_mangler` names them.
//!
//! A rewrite asks `luck_value` what an expression evaluates to, whether it's
//! truthy, and whether evaluating it can do anything, with the scoping
//! telling a local's read from a global's.
//!
//! [`TransformConfig`] turns each rewrite on or off. `luck_codegen` prints
//! every literal in the shortest spelling of its value, as
//! `LiteralSpelling::Shortest` asks.
//!
//! # Usage
//!
//! ```
//! use luck_allocator::Allocator;
//! use luck_minifier::TransformConfig;
//!
//! let allocator = Allocator::new();
//! let config = TransformConfig::default();
//! let output = luck_minifier::minify(&allocator, b"return 1 + 2", &config);
//! assert_eq!(output.as_deref(), Ok(&b"return 3"[..]));
//! ```

mod binding;
mod compress;
mod expr;
#[cfg(test)]
mod fixtures;
mod limits;
mod pipeline;
mod rename;
mod strip_types;
mod transform_config;

pub use pipeline::{minify, minify_chunk};
pub use transform_config::TransformConfig;
