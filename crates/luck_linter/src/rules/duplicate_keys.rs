use alloc::borrow::Cow;
use core::{fmt, str};

use luck_ast::{Expression, Field, Literal, TableConstructor};
use luck_token::{LuaVersion, Span, decode_string};
use rustc_hash::FxHashMap;

use crate::diagnostic::{Finding, line_column};
use crate::node_type::{NodeType, NodeTypeSet};
use crate::number::signed_number_value;
use crate::rule::{LintContext, NodeRule, impl_rule};

#[derive(Debug, Clone, Copy)]
pub(crate) struct DuplicateKeys;

impl_rule!(
    DuplicateKeys,
    "duplicate_keys",
    Correctness,
    Error,
    "table constructor has duplicate keys"
);

impl NodeRule for DuplicateKeys {
    fn node_types(&self) -> NodeTypeSet {
        NodeTypeSet::from_types(&[NodeType::TableConstructor])
    }
    fn on_table(
        &self,
        table: &TableConstructor,
        context: &LintContext<'_>,
        findings: &mut Vec<Finding>,
    ) {
        let version = context.semantic.version;
        let mut first_spans: FxHashMap<Key<'_>, Span> = FxHashMap::default();
        for field in table.fields.iter() {
            let keyed = match field {
                Field::Named { name, .. } => name
                    .identifier()
                    .map(|identifier| (Key::String(Cow::Borrowed(identifier)), name.span)),
                // Compare decoded values, since `["\97"]` and `["a"]` are the
                // same key.
                Field::Bracketed { key: Expression::StringLiteral(literal), .. } => {
                    decoded_key(literal, version).map(|key| (Key::String(key), literal.span))
                }
                Field::Bracketed { key: key_expr, .. } => {
                    number_key(key_expr, version).map(|key| (key, key_expr.span()))
                }
                Field::Positional { .. } => None,
            };
            let Some((key, span)) = keyed else {
                continue;
            };
            if let Some(first_span) = first_spans.get(&key) {
                let (line, column) = line_column(context.source, first_span.start, version);
                findings.push(
                    Finding::new(format!("duplicate key `{key}` in table constructor"), span)
                        .with_help(format!("previous definition at {line}:{column}")),
                );
            } else {
                first_spans.insert(key, span);
            }
        }
    }
}

/// A table key this rule can compare.
#[derive(Debug, PartialEq, Eq, Hash)]
enum Key<'ast> {
    String(Cow<'ast, str>),
    /// A number literal's value, as bits with `-0.0` folded into `0.0`.
    Number(u64),
}

impl fmt::Display for Key<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::String(value) => write!(f, "{value}"),
            Self::Number(bits) => write!(f, "{}", f64::from_bits(*bits)),
        }
    }
}

/// A string literal's decoded value, borrowed when decoding changes nothing,
/// or `None` when it doesn't decode to UTF-8.
fn decoded_key(literal: &Literal, version: LuaVersion) -> Option<Cow<'_, str>> {
    match decode_string(&literal.text, version)? {
        Cow::Borrowed(bytes) => str::from_utf8(bytes).ok().map(Cow::Borrowed),
        Cow::Owned(bytes) => String::from_utf8(bytes).ok().map(Cow::Owned),
    }
}

/// The key a number literal sets, so `1` and `1.0` match. A value of 2^53 or
/// more isn't compared, since a double can't tell it from its neighbors.
fn number_key(expr: &Expression, version: LuaVersion) -> Option<Key<'static>> {
    let value = signed_number_value(expr, version)?;
    (value.abs() < EXACT_DOUBLE_LIMIT)
        .then(|| Key::Number(if value == 0.0f64 { 0 } else { value.to_bits() }))
}

/// 2^53, the first double that can't hold an integer's value exactly.
const EXACT_DOUBLE_LIMIT: f64 = 9_007_199_254_740_992.0;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diagnostic::LintDiagnostic;
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

    #[test]
    fn flags_duplicate_keys_in_table_call_argument() {
        let diags = run("foo{ a = 1, a = 2 }");
        assert_eq!(diags.len(), 1, "a `f{{...}}` table is a table constructor: {diags:?}");
    }

    #[test]
    fn flags_duplicate_numeric_keys() {
        let diags = run("local t = { [1] = \"a\", [1] = \"b\" }");
        assert_eq!(diags.len(), 1, "both fields set key 1: {diags:?}");
    }

    #[test]
    fn flags_numeric_keys_spelled_differently() {
        let diags = run("local t = { [1] = \"a\", [1.0] = \"b\", [-2] = \"c\", [-2.0] = \"d\" }");
        assert_eq!(diags.len(), 2, "{diags:?}");
        assert!(diags[0].message.contains("`1`"), "{diags:?}");
    }

    #[test]
    fn ignores_integers_a_double_cannot_tell_apart() {
        let diags = run("local t = { [9007199254740993] = 1, [9007199254740992] = 2 }");
        assert!(diags.is_empty(), "{diags:?}");
    }
}
