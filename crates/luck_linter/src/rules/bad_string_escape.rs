use crate::context::LintContext;
use crate::diagnostic::Finding;
use crate::rule::{Rule, declare_lint_rule};

/// Flags backslash escapes in strings that Luau doesn't define, which it
/// reads as the bare character. The lexer notes each where it reads it.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct BadStringEscape;

const HELP: &str = "valid escapes: `\\a` `\\b` `\\f` `\\n` `\\r` `\\t` `\\v` `\\\\` `\\\"` `\\'` \
                    `\\ddd` `\\xXX` `\\z` `\\u{XXX}`";

declare_lint_rule!(
    BadStringEscape,
    Correctness,
    Warning,
    "invalid backslash escape in string literal"
);

impl Rule for BadStringEscape {
    fn run_once(&self, context: &LintContext<'_, '_>) {
        for &span in context.lenient_escapes {
            let escaped = &span.text(context.source)[1..];
            // The escape takes one byte, and its span the whole character.
            let display = match str::from_utf8(escaped).ok().and_then(|text| text.chars().next()) {
                Some(character) if character.is_ascii_control() || character == ' ' => {
                    format!("\\x{:02x}", u32::from(character))
                }
                Some(character) => format!("\\{character}"),
                None => format!("\\x{:02x}", escaped[0]),
            };
            context.diagnostic(
                Finding::new(format!("invalid string escape `{display}`"), span).with_help(HELP),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::fixtures::Tester;
    use crate::rules::RuleId;

    const PASS: &[&str] = &[
        r#"local x = "\n""#,
        r#"local x = "\z""#,
        r#"local x = "\u{1F600}""#,
        // A long string holds no escapes.
        r"local x = [[\q]]",
        r#"local x = "\065""#,
        "\
-- a \\q comment
local x = 1
",
        r"local a = `it's \{x}`",
        r#"local b = `say "\{hi}" {x}`"#,
        r#"local x = "\x41\z  \u{41}""#,
        r"local x = `\{\}\`\n\x41\z  \u{41}\065`",
    ];

    const FAIL: &[&str] = &[
        r#"local x = "\q""#,
        "\
-- \"\\q
local x = \"\\q\" --[[ '\\q ]] local y = '\\w' -- \\e
",
        r"local x = `a\q{1}b\w`",
        r"local x = `a\qb\{c}\w`",
        r#"print "\q""#,
        // The span and the message take the whole character, not its first byte.
        "local x = \"\\\u{e9}\"",
    ];

    #[test]
    fn test() {
        Tester::new(RuleId::BadStringEscape, PASS, FAIL).test_and_snapshot();
    }
}
