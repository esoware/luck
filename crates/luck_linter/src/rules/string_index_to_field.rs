use luck_ast::{AstKind, AstType, Expression, Suffix, is_valid_identifier_in};
use luck_semantic::{AstNode, AstTypesBitset};

use crate::context::LintContext;
use crate::diagnostic::Finding;
use crate::fix::FixKind;
use crate::rule::{Rule, declare_lint_rule};

#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct StringIndexToField;

declare_lint_rule!(
    StringIndexToField,
    Style,
    Warning,
    "use of string-key index for an identifier-safe key; prefer dot syntax",
    fix = Fix
);

impl Rule for StringIndexToField {
    const NODE_TYPES: AstTypesBitset = AstTypesBitset::from_types(&[AstType::SuffixedExpression]);

    fn run<'ast>(&self, node: &AstNode<'ast>, context: &LintContext<'ast>) {
        let AstKind::SuffixedExpression(chain) = node.kind() else {
            return;
        };
        for suffix in &chain.suffixes {
            if let Some(finding) = check_index(suffix, context) {
                context.diagnostic(finding);
            }
        }
    }
}

/// The fix replaces just the brackets, since parentheses the tree leaves
/// out can close between the head and them.
fn check_index(suffix: &Suffix<'_>, context: &LintContext<'_>) -> Option<Finding> {
    let Suffix::Index { span: brackets, index: Expression::StringLiteral(literal) } = suffix else {
        return None;
    };
    // The key is its value, however it's spelled.
    let name = literal.as_str()?;
    if !is_valid_identifier_in(name, context.semantic.version()) {
        return None;
    }

    let key = String::from_utf8_lossy(literal.span.text(context.source));
    Some(
        Finding::new(format!("`[{key}]` is equivalent to `.{name}`"), *brackets)
            .with_help("prefer dot access for identifier-safe keys")
            .with_fix(context, FixKind::Fix, |fixer| {
                fixer.replace(
                    format!("rewrite `[{key}]` as `.{name}`"),
                    *brackets,
                    format!(".{name}").into_bytes(),
                )
            }),
    )
}

#[cfg(test)]
mod tests {
    use luck_token::LuaTarget;

    use crate::fixtures::Tester;
    use crate::rules::RuleId;

    const PASS: &[(&str, LuaTarget)] = &[
        (r#"local x = t["end"]"#, LuaTarget::Lua54),
        // Lua 5.5 builds without `LUA_COMPAT_GLOBAL` reserve `global`.
        (r#"local x = t["global"]"#, LuaTarget::Lua55),
        (r#"local x = t["with space"]"#, LuaTarget::Lua54),
        (r#"local x = t["1bad"]"#, LuaTarget::Lua54),
        (r#"local x = t["a\nb"]"#, LuaTarget::Lua54),
    ];

    const FAIL: &[(&str, LuaTarget)] = &[
        (r#"local x = t["foo"]"#, LuaTarget::Lua54),
        ("local x = t['foo']", LuaTarget::Lua54),
        // No fix, since it would delete the comment.
        (r#"local x = t[--[[why]] "foo"]"#, LuaTarget::Lua54),
        // Luau reads `continue` as a name wherever a name goes, a field included.
        (r#"local x = t["continue"]"#, LuaTarget::Luau),
        ("local x = (t)['a'] .. (t.u)['b']", LuaTarget::Lua54),
        (r#"local x = t["a"]["b"]"#, LuaTarget::Lua54),
        (r#"t["a"]["b"], u["c"] = 1, 2"#, LuaTarget::Lua54),
        (r#"t["count"] += 1"#, LuaTarget::Luau),
        (r#"local x = t["\102oo"] + t[ [[bar]] ]"#, LuaTarget::Lua54),
        // A leading `_` is valid in an identifier.
        (r#"local x = t["_priv"]"#, LuaTarget::Lua54),
    ];

    #[test]
    fn test() {
        Tester::new(RuleId::StringIndexToField, PASS, FAIL).test_and_snapshot();
    }
}
