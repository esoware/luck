use crate::context::LintContext;
use crate::diagnostic::Finding;
use crate::rule::{Rule, declare_lint_rule};

/// Flags backslash escapes in strings that the dialect doesn't define, which
/// Lua 5.1 and Luau read as the bare character.
///
/// The lexer notes each where it reads it. Every other version rejects these
/// in the lexer, so a file on those targets that reaches the rules has none
/// to flag.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct BadStringEscape;

declare_lint_rule!(
    BadStringEscape,
    Correctness,
    Warning,
    "invalid backslash escape in string literal"
);

impl Rule for BadStringEscape {
    fn run_once(&self, context: &LintContext<'_, '_>) {
        if context.lenient_escapes.is_empty() {
            return;
        }
        let extended_escapes = if context.semantic.version().has_hex_escape() {
            " `\\xXX` `\\z` `\\u{XXX}`"
        } else {
            ""
        };
        let help = format!(
            "valid escapes: `\\a` `\\b` `\\f` `\\n` `\\r` `\\t` `\\v` `\\\\` `\\\"` `\\'` \
             `\\ddd`{extended_escapes}"
        );
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
                Finding::new(format!("invalid string escape `{display}`"), span)
                    .with_help(help.clone()),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use luck_token::LuaTarget;

    use crate::fixtures::Tester;
    use crate::rules::RuleId;

    const PASS: &[(&str, LuaTarget)] = &[
        (r#"local x = "\n""#, LuaTarget::Lua51),
        (r#"local x = "\z""#, LuaTarget::Lua52),
        (r#"local x = "\u{1F600}""#, LuaTarget::Lua53),
        // A long string holds no escapes.
        (r"local x = [[\q]]", LuaTarget::Lua51),
        (r#"local x = "\065""#, LuaTarget::Lua51),
        (
            "\
-- a \\q comment
local x = 1
",
            LuaTarget::Lua51,
        ),
        (r"local a = `it's \{x}`", LuaTarget::Luau),
        (r#"local b = `say "\{hi}" {x}`"#, LuaTarget::Luau),
        (r#"local x = "\x41\z  \u{41}""#, LuaTarget::Luau),
        (r"local x = `\{\}\`\n\x41\z  \u{41}\065`", LuaTarget::Luau),
    ];

    const FAIL: &[(&str, LuaTarget)] = &[
        (r#"local x = "\q""#, LuaTarget::Lua51),
        (r#"local x = "\z""#, LuaTarget::Lua51),
        (r#"local x = "\u{1F600}""#, LuaTarget::Lua51),
        (r#"local x = "\x41""#, LuaTarget::Lua51),
        (
            "\
-- \"\\q
local x = \"\\q\" --[[ '\\q ]] local y = '\\w' -- \\e
",
            LuaTarget::Lua51,
        ),
        (r#"local x = "\q""#, LuaTarget::Luau),
        (r"local x = `a\q{1}b\w`", LuaTarget::Luau),
        (r"local x = `a\qb\{c}\w`", LuaTarget::Luau),
        (r#"print "\q""#, LuaTarget::Lua51),
        // The span and the message take the whole character, not its first byte.
        ("local x = \"\\\u{e9}\"", LuaTarget::Lua51),
    ];

    #[test]
    fn test() {
        Tester::new(RuleId::BadStringEscape, PASS, FAIL).test_and_snapshot();
    }
}
