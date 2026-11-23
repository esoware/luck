use alloc::borrow::Cow;

use luck_token::{DirectiveError, DirectiveErrorKind};

use crate::diagnostic::Finding;
use crate::directives::{DirectiveProblem, Target};
use crate::rule::{LintContext, Rule, WholeRule, impl_rule};
use crate::rules::{closest_rule_name, rules};

/// Flags a `luck:` comment that's no directive, and a directive's rule name
/// no rule has, such as a parse error's code, `E008`. Either would otherwise
/// do nothing.
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
        let malformed = directives.errors().iter().map(|problem| match *problem {
            DirectiveProblem::Malformed(error) => {
                Finding::new(malformed_message(error, context.source), error.span)
            }
            DirectiveProblem::UnopenedEnd { verb, name } => Finding::new(
                format!(
                    "no `{}({}) start` for this `end` to close",
                    verb.name(),
                    name.text(context.source)
                ),
                name,
            ),
        });
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

fn malformed_message(error: DirectiveError, source: &str) -> Cow<'static, str> {
    let text = error.span.text(source);
    match error.kind {
        DirectiveErrorKind::Empty => "expected a directive after `luck:`".into(),
        DirectiveErrorKind::NotLintVerb => "`#luck:` takes only `allow`, `deny`, or `warn`".into(),
        DirectiveErrorKind::UnknownDirective => format!(
            "unknown directive `{text}`, expected `allow`, `deny`, `warn`, `format`, or `ignore`"
        )
        .into(),
        DirectiveErrorKind::UnknownFormatSetting => {
            "expected `off`, `on`, or `ignore` after `format`".into()
        }
        DirectiveErrorKind::MissingList(verb) => {
            format!("expected `(` after `{}`", verb.name()).into()
        }
        DirectiveErrorKind::UnclosedList => "unclosed `(`".into(),
        DirectiveErrorKind::EmptyList(verb) => format!("`{}` names no rule", verb.name()).into(),
        DirectiveErrorKind::RegionOnFileWide => {
            "a `#luck:` directive covers the whole file, so it takes no `start` or `end`".into()
        }
        DirectiveErrorKind::UnknownExtent(verb) => {
            format!("expected `start` or `end` after `{}(...)`, found `{text}`", verb.name()).into()
        }
    }
}

#[cfg(test)]
mod tests {
    use luck_token::LuaVersion;

    use super::*;
    use crate::diagnostic::Message;
    use crate::fixtures::run_rule;

    fn run(source: &str) -> Vec<Message> {
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
    fn flags_the_parse_error_code_which_no_directive_can_silence() {
        let diags = run("\
-- #luck: allow(E008)
local _x = 1");
        assert_eq!(diags.len(), 1, "{diags:?}");
        assert_eq!(diags[0].message, "unknown lint rule `E008`");
    }

    #[test]
    fn flags_a_misspelled_region_word() {
        let source = "\
-- luck: allow(unused_variable) strat
local _x = 1";
        let diags = run(source);
        assert_eq!(diags.len(), 1, "{diags:?}");
        assert_eq!(diags[0].span().text(source), "strat");
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
