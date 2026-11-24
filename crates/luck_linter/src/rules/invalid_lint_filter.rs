use std::borrow::Cow;

use luck_token::{DirectiveError, DirectiveErrorKind};

use crate::context::LintContext;
use crate::diagnostic::Finding;
use crate::directives::{DirectiveProblem, Target};
use crate::rule::{Rule, declare_lint_rule};
use crate::rules::closest_rule_name;

/// Flags a `luck:` comment that's no directive, and a directive's rule name
/// no rule has, such as a parse error's code, `E008`. Either would otherwise
/// do nothing.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct InvalidLintFilter;

declare_lint_rule!(
    InvalidLintFilter,
    Correctness,
    Warning,
    "malformed `luck:` directive, or a rule name in one that no rule has"
);

impl Rule for InvalidLintFilter {
    fn run_once(&self, context: &LintContext<'_>) {
        let directives = context.directives;
        let malformed = directives.errors().iter().map(|problem| match *problem {
            DirectiveProblem::Malformed(error) => {
                Finding::new(malformed_message(error, context.source), error.span)
            }
            DirectiveProblem::UnopenedEnd { verb, name } => Finding::new(
                format!(
                    "no `{}({}) start` for this `end` to close",
                    verb.name(),
                    String::from_utf8_lossy(name.text(context.source))
                ),
                name,
            ),
        });
        let unknown =
            directives.entries().iter().filter(|entry| entry.target == Target::Unknown).map(
                |entry| {
                    let name = String::from_utf8_lossy(entry.name.text(context.source));
                    let (message, help) = match closest_rule_name(&name) {
                        Some(rule) => {
                            let suggestion = rule.name();
                            (
                                format!("unknown lint rule `{name}`, did you mean `{suggestion}`?"),
                                format!("rename it to `{suggestion}`"),
                            )
                        }
                        None => (
                            format!("unknown lint rule `{name}`"),
                            "no lint rule has this name".to_owned(),
                        ),
                    };
                    Finding::new(message, entry.name).with_help(help)
                },
            );
        for finding in malformed.chain(unknown) {
            context.diagnostic(finding);
        }
    }
}

fn malformed_message(error: DirectiveError, source: &[u8]) -> Cow<'static, str> {
    let text = String::from_utf8_lossy(error.span.text(source));
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
    use crate::fixtures::Tester;
    use crate::rules::RuleId;

    const PASS: &[&str] = &[
        "\
-- luck: allow(*)
local _x = 1",
        "\
-- luck: allow(unused_variable)
-- luck: format off
local _x = 1
-- luck: format on
-- luck: ignore because aligned
local _y = 2",
    ];

    const FAIL: &[&str] = &[
        "\
-- luck: allow(not_a_real_rule)
local _x = 1",
        "\
-- #luck: allow(E008)
local _x = 1",
        "\
-- luck: allow(unused_variable) strat
local _x = 1",
        "\
-- luck: alow(unused_variable)
-- luck: format offf
local _x = 1",
        "\
local _x = 1
-- luck: deny(unused_variable) end",
    ];

    #[test]
    fn test() {
        Tester::new(RuleId::InvalidLintFilter, PASS, FAIL).test_and_snapshot();
    }
}
