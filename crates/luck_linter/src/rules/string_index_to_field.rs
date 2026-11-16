use luck_ast::{ChainHead, Expression, Suffix, SuffixedExpression};
use luck_token::{LuaVersion, Span, TokenKind, short_string_body};

use crate::diagnostic::Finding;
use crate::fix::{Fix, TextEdit};
use crate::node_type::{NodeType, NodeTypeSet};
use crate::rule::{ChainPosition, LintContext, NodeRule, impl_rule};

#[derive(Debug, Clone, Copy)]
pub(crate) struct StringIndexToField;

impl_rule!(
    StringIndexToField,
    "string_index_to_field",
    Style,
    Warning,
    "use of string-key index for an identifier-safe key; prefer dot syntax"
);

impl NodeRule for StringIndexToField {
    fn node_types(&self) -> NodeTypeSet {
        NodeTypeSet::from_types(&[NodeType::Suffixed])
    }
    fn on_chain(
        &self,
        chain: &SuffixedExpression,
        _position: ChainPosition,
        context: &LintContext<'_>,
        findings: &mut Vec<Finding>,
    ) {
        for (position, suffix) in chain.suffixes.iter().enumerate() {
            findings.extend(check_index(chain.head(position), suffix, context.semantic.version));
        }
    }
}

fn check_index(head: ChainHead<'_>, suffix: &Suffix, version: LuaVersion) -> Option<Finding> {
    let Suffix::Index { span: index_span, index: Expression::StringLiteral(literal) } = suffix
    else {
        return None;
    };
    // The body is the raw spelling. The shape check rejects the `\` of any
    // escape, which could make the key differ from that spelling.
    let name = short_string_body(&literal.text)?;
    if !TokenKind::is_identifier_shaped(name) || TokenKind::is_reserved(name, version) {
        return None;
    }

    // Replace everything after the head, brackets included.
    let span = Span::new(head.span().end, index_span.end);
    Some(
        Finding::new(format!("`[\"{name}\"]` is equivalent to `.{name}`"), span)
            .with_help("prefer dot access for identifier-safe keys")
            .with_fix(Fix {
                description: format!("rewrite `[\"{name}\"]` as `.{name}`"),
                edits: vec![TextEdit { span, replacement: format!(".{name}") }],
            }),
    )
}

#[cfg(test)]
mod tests {
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
    fn ignores_escape_sequence_key() {
        let diags = run("local x = t[\"a\\nb\"]");
        assert!(diags.is_empty(), "{diags:?}");
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
