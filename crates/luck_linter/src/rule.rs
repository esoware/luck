use luck_diagnostics::Severity;
use luck_semantic::{AstNode, AstTypesBitset};

use crate::context::{ContextHost, LintContext};
use crate::diagnostic::Category;
use crate::fix::FixKind;

/// A rule's identity and defaults. Its `RuleRunner` impl does the checking.
pub trait Rule: Send + Sync {
    /// Unique name, such as `unused_variable`, used in config and directives.
    fn name(&self) -> &'static str;

    fn category(&self) -> Category;

    fn default_severity(&self) -> Severity;

    fn description(&self) -> &'static str;

    /// The kinds the rule's fixes come in, empty when it offers none. Some
    /// of its diagnostics may still have no fix.
    fn fix_kind(&self) -> FixKind;

    /// The rule's source on GitHub, at the release this linter is, where its
    /// doc comment and its `flags_` and `ignores_` tests document it.
    fn docs_url(&self) -> &'static str;
}

/// The last argument, when there is one, lists the kinds the rule's fixes
/// come in, as in `Fix | Suggestion`.
macro_rules! impl_rule {
    (
        $rule:ident,
        $name:literal,
        $category:ident,
        $severity:ident,
        $description:literal
        $(, $($fix_kind:ident)|+)?
    ) => {
        impl $crate::rule::Rule for $rule {
            fn name(&self) -> &'static str {
                $name
            }
            fn category(&self) -> $crate::diagnostic::Category {
                $crate::diagnostic::Category::$category
            }
            fn default_severity(&self) -> luck_diagnostics::Severity {
                luck_diagnostics::Severity::$severity
            }
            fn description(&self) -> &'static str {
                $description
            }
            fn fix_kind(&self) -> $crate::fix::FixKind {
                $crate::fix::FixKind::empty() $($(.union($crate::fix::FixKind::$fix_kind))+)?
            }
            fn docs_url(&self) -> &'static str {
                concat!(
                    env!("CARGO_PKG_REPOSITORY"),
                    "/blob/v",
                    env!("CARGO_PKG_VERSION"),
                    "/crates/luck_linter/src/rules/",
                    $name,
                    ".rs",
                )
            }
        }
    };
}

pub(crate) use impl_rule;

/// How a rule checks a file, as oxlint's `Rule` does.
///
/// A rule implements one of two hooks: [`Self::run`], on each node of the
/// types [`Self::node_types`] lists, all rules in one pass over the node
/// table, or [`Self::run_once`], once per file.
pub(crate) trait RuleRunner: Rule {
    /// The types of node [`Self::run`] acts on. A file that holds none of
    /// them skips the rule. Listing an extra type is harmless, and leaving
    /// one out skips those nodes, which the driver's debug check catches.
    fn node_types(&self) -> AstTypesBitset {
        AstTypesBitset::new()
    }

    /// Whether the rule can report anything in this file, as a Roblox rule
    /// can't in a file of any other target.
    fn should_run(&self, _host: &ContextHost<'_>) -> bool {
        true
    }

    /// Checks `node`, which it matches by kind itself, since the driver's
    /// debug check hands it every node of the tree.
    fn run<'ast>(&self, _node: &AstNode<'ast>, _context: &LintContext<'ast>) {}

    fn run_once(&self, _context: &LintContext<'_>) {}
}
