use luck_ast::{ChainHead, Expression, GetSpan as _, Suffix, SuffixedExpression};
use luck_token::{Span, TokenKind};

use crate::diagnostic::Finding;
use crate::fix::FixKind;
use crate::node_type::{NodeType, NodeTypeSet};
use crate::rule::{ChainPosition, LintContext, NodeRule, impl_rule};

#[derive(Debug, Clone, Copy)]
pub(crate) struct StringIndexToField;

impl_rule!(
    StringIndexToField,
    "string_index_to_field",
    Style,
    Warning,
    "use of string-key index for an identifier-safe key; prefer dot syntax",
    Fix
);

impl NodeRule for StringIndexToField {
    fn node_types(&self) -> NodeTypeSet {
        NodeTypeSet::from_types(&[NodeType::Suffixed])
    }
    fn on_chain(
        &self,
        chain: &SuffixedExpression<'_>,
        _position: ChainPosition,
        context: &LintContext<'_>,
        findings: &mut Vec<Finding>,
    ) {
        for (position, suffix) in chain.suffixes.iter().enumerate() {
            findings.extend(check_index(chain.head(position), suffix, context));
        }
    }
}

fn check_index(
    head: ChainHead<'_>,
    suffix: &Suffix<'_>,
    context: &LintContext<'_>,
) -> Option<Finding> {
    let Suffix::Index { span: index_span, index: Expression::StringLiteral(literal) } = suffix
    else {
        return None;
    };
    // The key is its value, however it's spelled.
    let name = literal.as_str()?;
    if !TokenKind::is_identifier_shaped(name)
        || TokenKind::is_reserved(name, context.semantic.version())
    {
        return None;
    }

    // Replace everything after the head, brackets included.
    let span = Span::new(head.span().end, index_span.end);
    let key = literal.span.text(context.source);
    Some(
        Finding::new(format!("`[{key}]` is equivalent to `.{name}`"), span)
            .with_help("prefer dot access for identifier-safe keys")
            .with_fix(context, FixKind::Fix, |fixer| {
                fixer.replace(format!("rewrite `[{key}]` as `.{name}`"), span, format!(".{name}"))
            }),
    )
}

#[cfg(test)]
mod tests {
    use luck_token::LuaVersion;

    use super::*;
    use crate::diagnostic::LintDiagnostic;
    use crate::fixtures::{fix_and_reparse, run_rule};

    fn run(source: &str) -> Vec<LintDiagnostic> {
        run_rule(&StringIndexToField, source, LuaVersion::Lua54)
    }

    #[test]
    fn flags_identifier_safe_key() {
        let source = "local x = t[\"foo\"]";
        let diags = run(source);
        assert_eq!(diags.len(), 1, "{diags:?}");
        let fixed = fix_and_reparse(source, &diags, LuaVersion::Lua54);
        assert_eq!(fixed, "local x = t.foo");
    }

    #[test]
    fn flags_key_quoted_as_written() {
        let diags = run("local x = t['foo']");
        assert_eq!(diags.len(), 1, "{diags:?}");
        assert_eq!(diags[0].message, "`['foo']` is equivalent to `.foo`");
    }

    #[test]
    fn flags_without_fix_for_comment_inside_brackets() {
        let diags = run("local x = t[--[[why]] \"foo\"]");
        assert_eq!(diags.len(), 1, "{diags:?}");
        assert!(diags[0].fix.is_none(), "the fix would delete the comment: {diags:?}");
    }

    #[test]
    fn ignores_keyword_key() {
        let diags = run("local x = t[\"end\"]");
        assert!(diags.is_empty(), "got: {diags:?}");
    }

    #[test]
    fn ignores_luau_continue_key() {
        let diags = run_rule(&StringIndexToField, "local x = t[\"continue\"]", LuaVersion::Luau);
        assert!(diags.is_empty(), "{diags:?}");
    }

    #[test]
    fn ignores_non_identifier_key() {
        let diags = run("local x = t[\"with space\"]");
        assert!(diags.is_empty(), "{diags:?}");
    }

    #[test]
    fn ignores_numeric_start() {
        let diags = run("local x = t[\"1bad\"]");
        assert!(diags.is_empty(), "{diags:?}");
    }

    #[test]
    fn flags_each_nested_index() {
        let source = "local x = t[\"a\"][\"b\"]";
        let diags = run(source);
        assert_eq!(diags.len(), 2, "{diags:?}");
        let fixed = fix_and_reparse(source, &diags, LuaVersion::Lua54);
        assert_eq!(fixed, "local x = t.a.b");
    }

    #[test]
    fn flags_assignment_targets() {
        let source = "t[\"a\"][\"b\"], u[\"c\"] = 1, 2";
        let diags = run(source);
        assert_eq!(diags.len(), 3, "{diags:?}");
        let fixed = fix_and_reparse(source, &diags, LuaVersion::Lua54);
        assert_eq!(fixed, "t.a.b, u.c = 1, 2");
    }

    #[test]
    fn flags_compound_assignment_target() {
        let source = "t[\"count\"] += 1";
        let diags = run_rule(&StringIndexToField, source, LuaVersion::Luau);
        assert_eq!(diags.len(), 1, "{diags:?}");
        let fixed = fix_and_reparse(source, &diags, LuaVersion::Luau);
        assert_eq!(fixed, "t.count += 1");
    }

    #[test]
    fn ignores_a_key_whose_value_is_no_name() {
        let diags = run("local x = t[\"a\\nb\"]");
        assert!(diags.is_empty(), "{diags:?}");
    }

    #[test]
    fn flags_a_key_spelled_with_escapes_or_long_brackets() {
        let source = "local x = t[\"\\102oo\"] + t[ [[bar]] ]";
        let diags = run(source);
        assert_eq!(diags.len(), 2, "{diags:?}");
        assert_eq!(diags[0].message, "`[\"\\102oo\"]` is equivalent to `.foo`");
        let fixed = fix_and_reparse(source, &diags, LuaVersion::Lua54);
        assert_eq!(fixed, "local x = t.foo + t.bar");
    }

    #[test]
    fn flags_underscore_prefixed_key() {
        // A leading `_` is valid in an identifier.
        let source = "local x = t[\"_priv\"]";
        let diags = run(source);
        assert_eq!(diags.len(), 1, "{diags:?}");
        let fixed = fix_and_reparse(source, &diags, LuaVersion::Lua54);
        assert_eq!(fixed, "local x = t._priv");
    }
}
