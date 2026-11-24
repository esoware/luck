use core::fmt;

use luck_ast::{AstKind, AstType, Expression, Field, GetSpan as _};
use luck_semantic::{AstNode, AstTypesBitset};
use luck_token::Span;
use luck_value::{ConstantEvaluation as _, ConstantValue, ValueContext};
use rustc_hash::FxHashMap;

use crate::context::LintContext;
use crate::diagnostic::Finding;
use crate::rule::{Rule, declare_lint_rule};

#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct DuplicateKeys;

declare_lint_rule!(DuplicateKeys, Correctness, Error, "table constructor has duplicate keys");

impl Rule for DuplicateKeys {
    const NODE_TYPES: AstTypesBitset = AstTypesBitset::from_types(&[AstType::TableConstructor]);

    fn run<'ast>(&self, node: &AstNode<'ast>, context: &LintContext<'ast>) {
        let AstKind::TableConstructor(table) = node.kind() else {
            return;
        };
        let mut first_spans: FxHashMap<Key<'_>, Span> = FxHashMap::default();
        for field in &table.fields {
            let keyed = match field {
                Field::Named { name, .. } => Some((Key::String(name.text.as_bytes()), name.span)),
                Field::Bracketed { key, .. } => table_key(key, context).map(|k| (k, key.span())),
                Field::Positional(_) => None,
            };
            let Some((key, span)) = keyed else {
                continue;
            };
            if let Some(&first_span) = first_spans.get(&key) {
                context.diagnostic(
                    Finding::new(format!("duplicate key `{key}` in table constructor"), span)
                        .with_label(first_span, "first defined here"),
                );
            } else {
                first_spans.insert(key, span);
            }
        }
    }
}

#[derive(Debug, PartialEq, Eq, Hash)]
enum Key<'ast> {
    String(&'ast [u8]),
    /// A key with an integer's value, which a float with one is too, as
    /// Lua 5.3+ converts it.
    Integer(i64),
    /// Any other float's bits.
    Float(u64),
}

impl fmt::Display for Key<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::String(value) => write!(f, "{}", value.escape_ascii()),
            Self::Integer(value) => write!(f, "{value}"),
            Self::Float(bits) => write!(f, "{}", f64::from_bits(*bits)),
        }
    }
}

/// The string or number key `expr` evaluates to. Values compare, so
/// `["\97"]` and `["a"]` set one key, and so do `[1]`, `[1.0]`, and `[0x1]`,
/// and `[0]` and `[-0]`. NaN is no key: setting it raises.
fn table_key<'ast>(expr: &Expression<'ast>, context: &dyn ValueContext) -> Option<Key<'ast>> {
    match expr.evaluate_value(context)? {
        ConstantValue::String(bytes) => Some(Key::String(bytes)),
        ConstantValue::Number(number) => {
            let float = number.to_f64();
            if float.is_nan() {
                return None;
            }
            Some(number.to_integer().map_or_else(|| Key::Float(float.to_bits()), Key::Integer))
        }
        ConstantValue::Nil | ConstantValue::Boolean(_) | ConstantValue::Integer(_) => None,
    }
}

#[cfg(test)]
mod tests {
    use luck_token::LuaVersion;

    use crate::diagnostic::Message;
    use crate::fixtures::run_rule;
    use crate::rules::RuleId;

    fn run(source: &str) -> Vec<Message> {
        run_rule(RuleId::DuplicateKeys, source, LuaVersion::Lua54)
    }

    #[test]
    fn flags_repeated_name_key() {
        let diags = run("local t = { a = 1, b = 2, a = 3 }");
        assert_eq!(diags.len(), 1, "{diags:?}");
        assert_eq!(diags[0].span().start, 26);
        assert_eq!(diags[0].labels.first().map(|label| label.span.start), Some(12));
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
            diags.iter().all(|diag| diag.labels.first().map(|label| label.span.start) == Some(12)),
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
    fn flags_keys_that_evaluate_alike() {
        let diags =
            run("local t = { [2] = 1, [1 + 1] = 2, [0] = 3, [-0.0] = 4, [0/0] = 5, [0/0] = 6 }");
        assert_eq!(diags.len(), 2, "NaN is no key: {diags:?}");
    }

    #[test]
    fn ignores_integers_lua53_tells_apart() {
        let diags = run("local t = { [9007199254740993] = 1, [9007199254740992] = 2 }");
        assert!(diags.is_empty(), "{diags:?}");
    }

    #[test]
    fn flags_integers_that_read_as_one_double() {
        let source = "local t = { [9007199254740993] = 1, [9007199254740992] = 2 }";
        let diags = run_rule(RuleId::DuplicateKeys, source, LuaVersion::Lua51);
        assert_eq!(diags.len(), 1, "{diags:?}");
    }

    #[test]
    fn flags_keys_spelled_in_hex_or_long_brackets() {
        let diags = run("local t = { [0x10] = 1, [16.0] = 2, x = 3, [ [[x]] ] = 4 }");
        assert_eq!(diags.len(), 2, "{diags:?}");
    }
}
