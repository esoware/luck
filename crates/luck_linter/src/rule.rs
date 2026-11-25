use luck_diagnostics::Severity;
use luck_semantic::{AstNode, AstTypesBitset};

use crate::context::{ContextHost, LintContext};
use crate::fix::FixKind;

/// A group of lint rules, which `categories` sets the level of together.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Category {
    /// Outright wrong or useless code.
    Correctness,
    /// Most likely wrong or useless.
    Suspicious,
    /// Idiomatic code style.
    Style,
    /// Performance improvements.
    Performance,
}

impl Category {
    /// The name `luck.json`, `--rules`, and `--print-config` spell it with.
    pub const fn name(self) -> &'static str {
        match self {
            Self::Correctness => "correctness",
            Self::Suspicious => "suspicious",
            Self::Style => "style",
            Self::Performance => "performance",
        }
    }

    /// Parses a `-A`/`-W`/`-D` flag value, case-insensitively, or `None` when
    /// it names no category.
    pub fn from_name(name: &str) -> Option<Self> {
        match name.to_ascii_lowercase().as_str() {
            "correctness" => Some(Self::Correctness),
            "suspicious" => Some(Self::Suspicious),
            "style" => Some(Self::Style),
            "performance" | "perf" => Some(Self::Performance),
            _ => None,
        }
    }
}

/// A rule's identity and defaults, which [`declare_lint_rule!`] gives it, as
/// oxlint's `declare_oxc_lint!` gives its `RuleMeta`.
pub(crate) trait RuleMeta {
    const CATEGORY: Category;

    /// What the rule reports at while config sets no level for it or its
    /// category, which only a correctness rule runs under.
    const DEFAULT_SEVERITY: Severity;

    const DESCRIPTION: &'static str;

    /// The kinds the rule's fixes come in, empty when it offers none. Some
    /// of its diagnostics may still have no fix.
    const FIX: FixKind;

    /// What `[level, options]` gives the rule in `luck.json`, [`NoOptions`]
    /// for a rule that takes none.
    type Options: RuleOptions;

    /// The rule as `options` configure it. A rule no options configure is
    /// its `Default`.
    fn from_options(options: Self::Options) -> Self;
}

/// How a rule checks a file, as oxlint's `Rule` does.
///
/// A rule implements one of two hooks: [`Self::run`], on each node of the
/// types [`Self::NODE_TYPES`] lists, all rules in one pass over the node
/// table, or [`Self::run_once`], once per file.
pub(crate) trait Rule: RuleMeta + Default {
    /// The types of node [`Self::run`] acts on. A file that holds none of
    /// them skips the rule. Listing an extra type is harmless, and leaving
    /// one out skips those nodes, which the linter's debug check catches.
    const NODE_TYPES: AstTypesBitset = AstTypesBitset::new();

    /// Whether the rule can report anything in this file, as a Roblox rule
    /// can't in a file of any other target.
    fn should_run(&self, _host: &ContextHost<'_>) -> bool {
        true
    }

    /// Checks `node`, which it matches by kind itself, since the linter's
    /// debug check hands it every node of the tree.
    fn run<'ast>(&self, _node: &AstNode<'ast>, _context: &LintContext<'_, 'ast>) {}

    fn run_once(&self, _context: &LintContext<'_, '_>) {}
}

/// What `[level, options]` gives one rule.
pub(crate) trait RuleOptions {
    /// Whether these are [`NoOptions`], so the rule takes only a level.
    #[cfg(feature = "serde")]
    const IS_NONE: bool = false;

    /// `self` layered on `base`, as a `lint` section that `extends` another
    /// takes the options both give a rule.
    fn merge_onto(self, base: Self) -> Self;
}

/// The options of a rule that takes none, which no value is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Deserialize, serde::Serialize))]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
pub(crate) enum NoOptions {}

impl RuleOptions for NoOptions {
    #[cfg(feature = "serde")]
    const IS_NONE: bool = true;

    fn merge_onto(self, _base: Self) -> Self {
        match self {}
    }
}

/// Gives a rule its [`RuleMeta`], its name aside, which is its module's.
///
/// It takes the rule's category, default severity, and description, then,
/// when it has them, the kinds its fixes come in, as in
/// `fix = Fix | Suggestion`, and the type of its options, which build it
/// through `From`.
macro_rules! declare_lint_rule {
    (
        $rule:ident,
        $category:ident,
        $severity:ident,
        $description:literal
        $(, fix = $($fix_kind:ident)|+)?
        $(, options = $options:ty)?
        $(,)?
    ) => {
        impl $crate::rule::RuleMeta for $rule {
            const CATEGORY: $crate::rule::Category = $crate::rule::Category::$category;
            const DEFAULT_SEVERITY: luck_diagnostics::Severity =
                luck_diagnostics::Severity::$severity;
            const DESCRIPTION: &'static str = $description;
            const FIX: $crate::fix::FixKind =
                $crate::fix::FixKind::empty() $($(.union($crate::fix::FixKind::$fix_kind))+)?;

            $crate::rule::declare_lint_rule!(@options $($options)?);
        }
    };
    (@options) => {
        type Options = $crate::rule::NoOptions;

        fn from_options(options: Self::Options) -> Self {
            match options {}
        }
    };
    (@options $options:ty) => {
        type Options = $options;

        fn from_options(options: Self::Options) -> Self {
            Self::from(options)
        }
    };
}

pub(crate) use declare_lint_rule;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixtures::CATEGORIES;

    /// `name` is what `--rules` and `--print-config` print, so a `name` that
    /// `from_name` doesn't take back would break scripts feeding them to `-A`.
    #[test]
    fn category_names_round_trip_through_from_name() {
        for category in CATEGORIES {
            assert_eq!(Category::from_name(category.name()), Some(category));
        }
    }

    #[test]
    fn category_from_name_recognizes_categories_and_the_perf_alias() {
        assert_eq!(Category::from_name("style"), Some(Category::Style));
        assert_eq!(Category::from_name("perf"), Some(Category::Performance));
        assert_eq!(Category::from_name("undefined_variable"), None);
    }
}
