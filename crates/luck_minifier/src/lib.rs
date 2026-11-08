//! # `luck_minifier`
//!
//! AST transform pipeline for Lua/Luau minification.
//!
//! Parses source, runs the enabled transforms, and prints the result with
//! [`luck_codegen::compact_with_capacity`]. Each pass takes and returns a
//! `Block` and is built on [`Transform`](luck_ast::Transform).
//!
//! ## Pipeline order
//!
//! `apply_core_transforms` and `apply_tail_transforms` hold the exact order.
//! The orderings that matter: `fold_constants` runs before and after
//! `inline_locals` (inlining exposes folds), `remove_dead_code` runs again
//! after the second fold (folds expose dead branches), `explicit_self` runs
//! just before `rename_locals`, and `rename_locals` is followed by
//! `lift_locals` and a final `merge_locals` that fuses lifted declarations.
//!
//! # Usage
//!
//! ```
//! use luck_core::TransformConfig;
//! use luck_token::LuaVersion;
//!
//! let output = luck_minifier::minify("return 1 + 2", LuaVersion::Lua54, &TransformConfig::default(), "input.lua").unwrap();
//! assert!(output.len() < "return 1 + 2".len());
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
mod expr;
#[cfg(test)]
mod fixtures;
mod pipeline;
mod transforms;

pub use pipeline::minify;
