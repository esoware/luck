use alloc::borrow::Cow;

use luck_ast::{Expression, Field, NodeType, NodeTypeSet};
use luck_token::{Span, decode_string};
use rustc_hash::FxHashMap;

use crate::diagnostic::{Category, LintDiagnostic, Severity, line_column};
use crate::rule::{LintContext, NodeRule, Rule};

#[derive(Debug, Clone, Copy)]
pub(crate) struct DuplicateKeys;

impl Rule for DuplicateKeys {
    fn name(&self) -> &'static str {
        "duplicate_keys"
    }
    fn category(&self) -> Category {
        Category::Correctness
    }
    fn default_severity(&self) -> Severity {
        Severity::Error
    }
    fn description(&self) -> &'static str {
        "table constructor has duplicate keys"
    }
}

impl NodeRule for DuplicateKeys {
    fn node_types(&self) -> NodeTypeSet {
        NodeTypeSet::from_types(&[NodeType::TableConstructor])
    }
    fn on_expression(
        &self,
        expr: &Expression,
        context: &LintContext<'_>,
        diags: &mut Vec<LintDiagnostic>,
    ) {
        let Expression::TableConstructor(table) = expr else {
            return;
        };
        let mut first_spans: FxHashMap<Cow<'_, str>, Span> = FxHashMap::default();
        for field in table.fields.iter() {
            let string_key = match field {
                Field::Named { name, .. } => {
                    name.identifier().map(|identifier| (Cow::Borrowed(identifier), name.span))
                }
                // Compare decoded values, since `["\97"]` and `["a"]` are the
                // same key.
                Field::Bracketed { key: Expression::StringLiteral(literal), .. } => {
                    decode_string(&literal.text, context.semantic.version)
                        .and_then(|bytes| String::from_utf8(bytes.into_owned()).ok())
                        .map(|value| (Cow::Owned(value), literal.span))
                }
                Field::Bracketed { .. } | Field::Positional { .. } => None,
            };
            let Some((key, span)) = string_key else {
                continue;
            };
            if let Some(first_span) = first_spans.get(key.as_ref()) {
                let (line, column) = line_column(context.source, first_span.start);
                diags.push(
                    LintDiagnostic::new(
                        "duplicate_keys",
                        format!("duplicate key `{key}` in table constructor"),
                        span,
                    )
                    .with_help(format!("previous definition at {line}:{column}")),
                );
            } else {
                first_spans.insert(key, span);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use luck_token::LuaVersion;

    use super::*;
    use crate::fixtures::run_rule;

    fn run(source: &str) -> Vec<LintDiagnostic> {
        run_rule(&DuplicateKeys, source, LuaVersion::Lua54)
    }

    #[test]
    fn flags_repeated_name_key() {
        let diags = run("local t = { a = 1, b = 2, a = 3 }");
        assert_eq!(diags.len(), 1, "{diags:?}");
        assert_eq!(diags[0].help.as_deref(), Some("previous definition at 1:13"));
    }

    #[test]
    fn flags_bracketed_string_matching_name_key() {
        let diags = run("local t = { a = 1, [\"a\"] = 2 }");
        assert_eq!(diags.len(), 1, "{diags:?}");
    }

    #[test]
    fn flags_escaped_key_by_decoded_value() {
        let diags = run("local t = { a = 1, [\"\\97\"] = 2 }");
        assert_eq!(diags.len(), 1, "{diags:?}");
        assert!(diags[0].message.contains("`a`"), "{diags:?}");
    }

    #[test]
    fn flags_every_repeat_against_the_first() {
        let diags = run("local t = { a = 1, a = 2, a = 3 }");
        assert_eq!(diags.len(), 2, "{diags:?}");
        assert!(
            diags.iter().all(|diag| diag.help.as_deref() == Some("previous definition at 1:13")),
            "{diags:?}"
        );
    }

    #[test]
    fn ignores_distinct_and_positional_keys() {
        let diags = run("local t = { 1, 1, a = 1, b = 1, [1] = 2, [x] = 3, [x] = 4 }");
        assert!(diags.is_empty(), "{diags:?}");
    }
}
