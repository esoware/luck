//! # `luck_linter`
//!
//! Lua/Luau linter with scope-aware analysis, auto-fixes, and suppression
//! comments.
//!
//! Rules fall into four categories: correctness, suspicious, style, and
//! performance. Only correctness rules run by default. [`rules()`] lists every
//! registered rule.
//!
//! # Usage
//!
//! ```
//! use luck_allocator::Allocator;
//! use luck_linter::{FixKind, LintConfig};
//! use luck_stdlib::library_for;
//! use luck_token::LuaTarget;
//!
//! let allocator = Allocator::new();
//! let config = LintConfig::default();
//! let stdlib = library_for(LuaTarget::Lua54);
//! let diags = luck_linter::lint(&allocator, b"print(1)", stdlib, &config, FixKind::empty());
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
mod driver;
mod fix;
#[cfg(test)]
mod fixtures;
mod format_pattern;
mod function_node;
mod local_flow;
mod number;
mod roblox;
mod rule;
mod rules;
mod suggest;
mod underscore_fix;

pub use config::{
    CategoryLevels, CyclomaticComplexityOptions, LintConfig, RestrictedModulePathsOptions,
    RuleLevel, RuleSetting, RuleSettings,
};
pub use diagnostic::{Category, Message};
pub use driver::{Analysis, analyze, lint, lint_analyzed, lint_parsed};
pub use fix::{Fix, FixError, FixKind, Fixed, TextEdit, fix, non_overlapping_edits};
pub use rule::Rule;
pub use rules::{rule_named, rules};
