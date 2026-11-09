use crate::diagnostic::Finding;
use crate::rule::{LintContext, WholeRule, impl_rule};
use crate::rules::{is_registered_rule_name, registered_rule_names};
use crate::suggest::closest;
use crate::suppression::{DirectiveRule, parse_directive};

/// Flags unknown rule names in lint directives. A typo like
/// `-- luck: allow(unsued_variable)` would otherwise do nothing.
#[derive(Debug, Clone, Copy)]
pub(crate) struct InvalidLintFilter;

impl_rule!(
    InvalidLintFilter,
    "invalid_lint_filter",
    Correctness,
    Warning,
    "lint directive names a rule that doesn't exist"
);

impl WholeRule for InvalidLintFilter {
    fn check(&self, context: &LintContext<'_>) -> Vec<Finding> {
        let mut findings = Vec::new();

        for comment in context.comments {
            // A directive that a bad trailing word disables still has its
            // names checked.
            let Some(directive) =
                parse_directive(comment.span.text(context.source), comment.span.start)
            else {
                continue;
            };
            for (rule, name_span) in directive.rules {
                let DirectiveRule::Named(name) = rule else {
                    continue;
                };
                if is_registered_rule_name(name) {
                    continue;
                }

                let (message, help) = closest_match(name, registered_rule_names()).map_or_else(
                    || {
                        (
                            format!("unknown lint rule `{name}`"),
                            "no lint rule has this name".to_owned(),
                        )
                    },
                    |suggestion| {
                        (
                            format!("unknown lint rule `{name}`, did you mean `{suggestion}`?"),
                            format!("rename it to `{suggestion}`"),
                        )
                    },
                );

                findings.push(Finding::new(message, name_span).with_help(help));
            }
        }

        findings
    }
}

/// A far-off "did you mean" is worse than none.
fn closest_match(
    needle: &str,
    candidates: impl IntoIterator<Item = &'static str>,
) -> Option<&'static str> {
    // Up to a third of the longer name, and never less than 2.
    closest(needle, candidates, |distance, candidate| {
        distance <= 2 || distance.saturating_mul(3) <= needle.len().max(candidate.len())
    })
}

#[cfg(test)]
mod tests {
    use luck_token::LuaVersion;

    use super::*;
    use crate::fixtures::run_rule;

    #[test]
    fn closest_match_picks_near_neighbor() {
        assert_eq!(
            closest_match("unsued_variable", ["unused_variable", "shadowing"]),
            Some("unused_variable")
        );
    }

    #[test]
    fn closest_match_none_for_far_strings() {
        assert!(closest_match("xx", ["unused_variable"]).is_none());
    }

    #[test]
    fn flags_unknown_name() {
        let diags = run_rule(
            &InvalidLintFilter,
            "-- luck: allow(not_a_real_rule)\nlocal _x = 1",
            LuaVersion::Lua54,
        );
        assert_eq!(diags.len(), 1, "{diags:?}");
        assert!(diags[0].message.contains("not_a_real_rule"));
    }

    #[test]
    fn flags_unknown_name_in_disabled_directive() {
        let diags = run_rule(
            &InvalidLintFilter,
            "-- luck: allow(not_a_real_rule) strat\nlocal _x = 1",
            LuaVersion::Lua54,
        );
        assert_eq!(diags.len(), 1, "{diags:?}");
    }

    #[test]
    fn ignores_wildcard() {
        let diags =
            run_rule(&InvalidLintFilter, "-- luck: allow(*)\nlocal _x = 1", LuaVersion::Lua54);
        assert!(diags.is_empty(), "{diags:?}");
    }

    #[test]
    fn ignores_known_name() {
        let diags = run_rule(
            &InvalidLintFilter,
            "-- luck: allow(unused_variable)\nlocal _x = 1",
            LuaVersion::Lua54,
        );
        assert!(diags.is_empty(), "{diags:?}");
    }
}
