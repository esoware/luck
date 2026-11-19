//! # `luck_minifier`
//!
//! Lua and Luau minification on the arena tree, in place.
//!
//! [`minify`] parses source into the caller's arena and [`minify_chunk`]
//! takes a tree. Both rewrite the tree in place and print it once, with
//! [`luck_codegen::compact`].
//!
//! The work runs in three phases, each once:
//!
//! - **Binding**, one walk that resolves every name to its binding, writes
//!   the binding into the name, so the later phases tell apart two locals
//!   that share a name however the tree is rewritten, and counts each
//!   binding's references.
//! - **Compression**, one in-place walk that runs every rewrite as it leaves
//!   each node: constant folding, dead-code and dead-local removal, inlining
//!   single-use locals, merging neighboring locals, `if` and `return`
//!   simplification, shorter indexes and parentheses, and lifting nested
//!   declarations to the top of their function. Each rewrite keeps the
//!   binding walk's counts true, so the walk reaches the fixpoint, and walks
//!   again only for a rewrite it exposed too late to make.
//! - **Renaming**, which gives locals the shortest names their scopes allow.
//!
//! [`TransformConfig`] turns each rewrite on or off. `luck_codegen` prints
//! every literal in the shortest spelling of its value.
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

mod bind;
mod binding;
mod compress;
mod expr;
#[cfg(test)]
mod fixtures;
mod limits;
mod name_table;
mod pipeline;
mod rename_globals;
mod rename_locals;
mod transform_config;

pub use pipeline::{minify, minify_chunk};
pub use transform_config::TransformConfig;
