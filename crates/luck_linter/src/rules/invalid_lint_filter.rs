use crate::diagnostic::Finding;
use crate::directives::Target;
use crate::rule::{LintContext, Rule, WholeRule, impl_rule};
use crate::rules::{closest_rule_name, rules};

/// Flags a `luck:` comment that's no directive, and a directive's rule name
/// no rule has, such as `parse_error`. Either would otherwise do nothing.
#[derive(Debug, Clone, Copy)]
pub(crate) struct InvalidLintFilter;

impl_rule!(
    InvalidLintFilter,
    "invalid_lint_filter",
    Correctness,
    Warning,
    "malformed `luck:` directive, or a rule name in one that no rule has"
);

impl WholeRule for InvalidLintFilter {
    fn check(&self, context: &LintContext<'_>) -> Vec<Finding> {
        let directives = context.directives;
        let malformed =
            directives.errors().iter().map(|error| Finding::new(error.message.clone(), error.span));
        let unknown = directives
            .entries()
            .iter()
            .filter(|entry| entry.target == Target::Unknown)
            .map(|entry| {
                let name = entry.name.text(context.source);
                let (message, help) = closest_rule_name(name, rules().map(Rule::name)).map_or_else(
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
                Finding::new(message, entry.name).with_help(help)
            });
        malformed.chain(unknown).collect()
    }
}

#[cfg(test)]
mod tests {
    use luck_token::LuaVersion;

    use super::*;
    use crate::diagnostic::LintDiagnostic;
    use crate::fixtures::run_rule;

    fn run(source: &str) -> Vec<LintDiagnostic> {
        run_rule(&InvalidLintFilter, source, LuaVersion::Lua54)
    }

    #[test]
    fn flags_unknown_name() {
        let diags = run("\
-- luck: allow(not_a_real_rule)
local _x = 1");
        assert_eq!(diags.len(), 1, "{diags:?}");
        assert!(diags[0].message.contains("not_a_real_rule"));
    }

    #[test]
    fn flags_parse_error_which_no_directive_can_silence() {
        let diags = run("\
-- #luck: allow(parse_error)
local _x = 1");
        assert_eq!(diags.len(), 1, "{diags:?}");
        assert_eq!(diags[0].message, "unknown lint rule `parse_error`");
    }

    #[test]
    fn flags_a_misspelled_region_word() {
        let source = "\
-- luck: allow(unused_variable) strat
local _x = 1";
        let diags = run(source);
        assert_eq!(diags.len(), 1, "{diags:?}");
        assert_eq!(diags[0].span.text(source), "strat");
    }

    #[test]
    fn flags_a_luck_comment_that_is_no_directive() {
        let diags = run("\
-- luck: alow(unused_variable)
-- luck: format offf
local _x = 1");
        assert_eq!(diags.len(), 2, "{diags:?}");
        assert!(diags[0].message.starts_with("unknown directive `alow(unused_variable)`"));
        assert!(diags[1].message.contains("after `format`"));
    }

    #[test]
    fn flags_an_end_that_closes_no_region() {
        let diags = run("\
local _x = 1
-- luck: deny(unused_variable) end");
        assert_eq!(diags.len(), 1, "{diags:?}");
        assert_eq!(diags[0].message, "no `deny(unused_variable) start` for this `end` to close");
    }

    #[test]
    fn ignores_wildcard() {
        let diags = run("\
-- luck: allow(*)
local _x = 1");
        assert!(diags.is_empty(), "{diags:?}");
    }

    #[test]
    fn ignores_known_name_and_formatter_directives() {
        let diags = run("\
-- luck: allow(unused_variable)
-- luck: format off
local _x = 1
-- luck: format on
-- luck: ignore because aligned
local _y = 2");
        assert!(diags.is_empty(), "{diags:?}");
    }
}
