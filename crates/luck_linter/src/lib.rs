//! # `luck_linter`
//!
//! Lua/Luau linter with scope-aware analysis, auto-fixes, and suppression
//! comments.
//!
//! Rules fall into four categories: correctness, suspicious, style, and
//! performance. Only correctness rules run by default. [`RuleId::ALL`] lists
//! every rule.
//!
//! A [`Linter`] compiles a [`LintConfig`] once and lints file after file.
//!
//! # Usage
//!
//! ```
//! use luck_allocator::Allocator;
//! use luck_linter::{FixKind, LintConfig, Linter};
//! use luck_stdlib::library_for;
//! use luck_token::LuaTarget;
//!
//! let allocator = Allocator::new();
//! let linter = Linter::new(LintConfig::default());
//! let parse = luck_parser::parse(&allocator, b"print(1)", LuaTarget::Lua54.version());
//! let diags = linter.run(&parse, library_for(LuaTarget::Lua54), FixKind::empty());
//! assert!(diags.is_empty());
//! ```

#![cfg_attr(
    test,
    expect(
        clippy::missing_panics_doc,
        reason = "a test panics to fail, so a Panics section says nothing"
    )
)]

extern crate alloc;

mod binary;
mod call;
mod cfg;
mod config;
mod context;
mod diagnostic;
mod directives;
mod fix;
#[cfg(test)]
mod fixtures;
mod format_pattern;
mod function_node;
mod linter;
mod local_flow;
mod number;
mod roblox;
mod rule;
mod rules;
mod suggest;
mod underscore_fix;

pub use config::{CategoryLevels, LintConfig, RuleLevel, RuleSettings};
pub use diagnostic::{Category, Message};
pub use fix::{Fix, FixError, FixKind, Fixed, TextEdit, non_overlapping_edits};
pub use linter::{Analysis, Linter};
pub use rules::RuleId;
