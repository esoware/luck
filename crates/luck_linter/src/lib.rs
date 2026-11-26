//! # `luck_linter`
//!
//! Luau linter with scope-aware analysis, auto-fixes, and suppression
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
//! use luck_stdlib::luau_library;
//!
//! let allocator = Allocator::new();
//! let linter = Linter::new(LintConfig::default());
//! let parse = luck_parser::parse(&allocator, b"print(1)");
//! let diags = linter.run(&parse, luau_library(), FixKind::empty(), None);
//! assert!(diags.is_empty());
//! ```

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
mod rule;
mod rules;
mod suggest;
mod timing;
mod underscore_fix;

pub use config::{CategoryLevels, LintConfig, RuleLevel, RuleSettings};
pub use diagnostic::Message;
pub use fix::{Fix, FixError, FixKind, Fixed, TextEdit, non_overlapping_edits};
pub use linter::{Analysis, Linter};
pub use rule::Category;
pub use rules::RuleId;
pub use timing::{RuleTiming, RuleTimings};
