use luck_diagnostics::Tag;

use crate::context::LintContext;
use crate::diagnostic::Finding;
use crate::rule::{Rule, declare_lint_rule};

/// Semantic analysis resolves each `goto` to the label it jumps to, as
/// `lparser.c` does, so a label is used exactly when one reaches it.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct UnusedLabel;

declare_lint_rule!(
    UnusedLabel,
    Style,
    Warning,
    "label declared but never targeted by a `goto` in the same function"
);

impl Rule for UnusedLabel {
    fn run_once(&self, context: &LintContext<'_>) {
        let tree = context.scoping();
        for label in tree.labels().iter().filter(|label| !label.is_used) {
            context.diagnostic(
                Finding::new(
                    format!("label `{}` is never targeted by a `goto`", tree.name(label.name)),
                    label.span,
                )
                .with_tag(Tag::Unnecessary)
                .with_help("remove the label or add a `goto` that targets it"),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use luck_token::LuaVersion;

    use crate::diagnostic::Message;
    use crate::fixtures::run_rule;
    use crate::rules::RuleId;

    fn run(source: &str) -> Vec<Message> {
        run_rule(RuleId::UnusedLabel, source, LuaVersion::Lua54)
    }

    #[test]
    fn ignores_label_with_matching_goto() {
        let diags = run("do ::a:: print(1) goto a end");
        assert!(diags.is_empty(), "{diags:?}");
    }

    #[test]
    fn flags_label_with_no_goto() {
        let diags = run("do ::a:: print(1) end");
        assert_eq!(diags.len(), 1, "{diags:?}");
        assert!(diags[0].message.contains("`a`"), "{diags:?}");
    }

    #[test]
    fn flags_label_in_different_function() {
        // g's goto targets g's own label (a cross-function goto would
        // not parse); f's label has no goto anywhere.
        let diags = run("function f() ::a:: print() end function g() ::a:: goto a end");
        assert_eq!(diags.len(), 1, "{diags:?}");
    }

    #[test]
    fn ignores_goto_in_inner_block() {
        let diags = run("do ::a:: do print() goto a end end");
        assert!(diags.is_empty(), "{diags:?}");
    }

    #[test]
    fn flags_unused_top_level_label() {
        let diags = run("::start:: print(1)");
        assert_eq!(diags.len(), 1, "{diags:?}");
    }

    #[test]
    fn ignores_top_level_label_used() {
        let diags = run("::start:: print(1) goto start");
        assert!(diags.is_empty(), "{diags:?}");
    }

    #[test]
    fn flags_label_unreachable_from_inner_function() {
        // The inner goto binds to the inner function's own label; the
        // outer label stays unused (labels are not visible across
        // function boundaries).
        let diags = run("::a:: local f = function() ::a:: goto a end");
        assert_eq!(diags.len(), 1, "outer label is unreachable from inner function: {diags:?}");
    }
}
