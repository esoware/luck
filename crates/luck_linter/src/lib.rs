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

mod bus;
mod call;
mod cfg;
mod diagnostic;
mod driver;
mod fix;
#[cfg(test)]
mod fixtures;
mod format_pattern;
mod nodes;
mod path;
mod roblox;
mod rule;
mod rules;
mod suggest;
mod suppression;
mod uninitialized;

/// Lint config types, defined in `luck_core` with the rest of `luck.json`.
pub use luck_core::{LintConfig, RuleSetting};

pub use diagnostic::{Category, LintDiagnostic, Severity};
pub use driver::{lint, lint_parsed, lint_target};
pub use fix::{FIXPOINT_BUDGET, Fix, FixpointError, TextEdit, apply_fixes, apply_fixes_fixpoint};
pub use rule::Rule;
pub use rules::{rules, unknown_rule_names};
