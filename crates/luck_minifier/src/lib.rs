//! # `luck_minifier`
//!
//! Lua and Luau minification on the arena tree, in place.
//!
//! [`minify`] parses source into the caller's arena and [`minify_block`]
//! takes a tree. Both rewrite the tree in place and print it once, with
//! [`luck_codegen::compact`].
//!
//! The work runs in two phases, repeated until neither changes the tree:
//!
//! - **Compression**, one in-place walk that runs every rewrite as it leaves
//!   each node, repeated until a walk changes nothing: constant folding,
//!   dead-code and dead-local removal, inlining single-use locals, merging
//!   neighboring locals, `if` and `return` simplification, and shorter
//!   literals, indexes, and parentheses. One scan before each walk finds the
//!   dead and inlinable locals.
//! - **Renaming**, which gives locals the shortest names their scopes allow,
//!   and then lifting, which hoists nested declarations to the top of their
//!   function in the walk of the next scan.
//!
//! [`TransformConfig`] turns each rewrite on or off.
//!
//! # Usage
//!
//! ```
//! use luck_allocator::Allocator;
//! use luck_minifier::TransformConfig;
//! use luck_token::LuaVersion;
//!
//! let allocator = Allocator::new();
//! let config = TransformConfig::default();
//! let output = luck_minifier::minify(&allocator, "return 1 + 2", LuaVersion::Lua54, &config);
//! assert_eq!(output.as_deref(), Ok("return 3"));
//! ```

#![cfg_attr(
    test,
    expect(
        clippy::missing_panics_doc,
        reason = "a test panics to fail, so a Panics section says nothing"
    )
)]

extern crate alloc;

mod binding;
mod compress;
mod explicit_self;
mod expr;
#[cfg(test)]
mod fixtures;
mod lift_locals;
mod limits;
mod name_table;
mod pipeline;
mod rename;
mod scan;
mod transform_config;

pub use pipeline::{minify, minify_block};
pub use transform_config::TransformConfig;
