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
//! use luck_token::LuaVersion;
//! use luck_linter::LintConfig;
//!
//! let diags = luck_linter::lint("print(1)", LuaVersion::Lua54, &LintConfig::default());
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
mod bus;
mod call;
mod cfg;
mod config;
mod diagnostic;
mod driver;
mod fix;
#[cfg(test)]
mod fixtures;
mod format_pattern;
mod function_node;
mod local_flow;
mod node_type;
mod nodes;
mod number;
mod roblox;
mod rule;
mod rules;
mod suggest;
mod suppression;
mod underscore_fix;
mod uninitialized;

pub use config::{
    CategoryLevels, CyclomaticComplexityOptions, LintConfig, RestrictedModulePathsOptions,
    RuleLevel, RuleSetting, RuleSettings,
};
pub use diagnostic::{Category, LintDiagnostic, Severity};
pub use driver::{lint, lint_analyzed, lint_parsed, lint_target};
pub use fix::{
    FIXPOINT_BUDGET, Fix, FixpointError, TextEdit, apply_fixes, apply_fixes_fixpoint,
    non_overlapping_edits,
};
pub use rule::Rule;
pub use rules::{rules, unknown_rule_names};
