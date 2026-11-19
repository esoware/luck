use core::fmt;
use core::ops::Neg as _;

use luck_ast::{Expression, Field, GetSpan as _, TableConstructor};
use luck_token::{LuaNumber, Span, UnOp};
use rustc_hash::FxHashMap;

use crate::diagnostic::Finding;
use crate::node_type::{NodeType, NodeTypeSet};
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
        table: &TableConstructor<'_>,
        context: &LintContext<'_>,
        findings: &mut Vec<Finding>,
    ) {
        let mut first_spans: FxHashMap<Key<'_>, Span> = FxHashMap::default();
        for field in &table.fields {
            let keyed = match field {
                Field::Named { name, .. } => Some((Key::String(name.text.as_bytes()), name.span)),
                // Values compare, so `["\97"]` and `["a"]` are the same key.
                Field::Bracketed { key: Expression::StringLiteral(literal), .. } => {
                    Some((Key::String(literal.value.as_bytes()), literal.span))
                }
                Field::Bracketed { key: key_expr, .. } => {
                    number_key(key_expr).map(|k| (k, key_expr.span()))
                }
                Field::Positional(_) => None,
            };
            let Some((key, span)) = keyed else {
                continue;
            };
            if let Some(first_span) = first_spans.get(&key) {
                let (line, column) = context.line_index().line_column(first_span.start);
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

/// The key a number literal sets, possibly negated, so `1`, `1.0`, and
/// `0x1` match, and so do `0` and `-0`.
fn number_key(expr: &Expression<'_>) -> Option<Key<'static>> {
    let value = signed_number(expr)?;
    let float = match value {
        LuaNumber::Int(integer) => return Some(Key::Integer(integer)),
        LuaNumber::Float(float) => float,
    };
    if float.fract() == 0.0f64
        && let Some(integer) = integral_float(float)
    {
        return Some(Key::Integer(integer));
    }
    Some(Key::Float(float.to_bits()))
}

/// A number literal's value under any number of unary minuses, negated as
/// Lua negates it.
fn signed_number(expr: &Expression<'_>) -> Option<LuaNumber> {
    match expr {
        Expression::Number(literal) => Some(literal.value),
        Expression::UnaryOp(unary) if unary.op == UnOp::Neg => {
            Some(match signed_number(&unary.operand)? {
                LuaNumber::Int(integer) => LuaNumber::Int(integer.wrapping_neg()),
                LuaNumber::Float(float) => LuaNumber::Float(float.neg()),
            })
        }
        Expression::Nil(_)
        | Expression::False(_)
        | Expression::True(_)
        | Expression::Integer(_)
        | Expression::StringLiteral(_)
        | Expression::VarArg(_)
        | Expression::FunctionDef(_)
        | Expression::Name(_)
        | Expression::Suffixed(_)
        | Expression::Parenthesized(_)
        | Expression::TableConstructor(_)
        | Expression::BinaryOp(_)
        | Expression::UnaryOp(_)
        | Expression::IfExpression(_)
        | Expression::InterpolatedString(_)
        | Expression::TypeCast(_)
        | Expression::Error(_) => None,
    }
}

/// `float`, which has no fraction, as the integer it equals, when one does:
/// one in `[-2^63, 2^63)`.
#[expect(
    clippy::as_conversions,
    clippy::cast_possible_truncation,
    reason = "every float in the range checked is an integer's exact value"
)]
fn integral_float(float: f64) -> Option<i64> {
    // -2^63 and 2^63.
    let range = f64::from_bits(0xC3E0_0000_0000_0000)..f64::from_bits(0x43E0_0000_0000_0000);
    range.contains(&float).then_some(float as i64)
}

#[cfg(test)]
mod tests {
    use luck_token::LuaVersion;

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
    fn ignores_integers_lua53_tells_apart() {
        let diags = run("local t = { [9007199254740993] = 1, [9007199254740992] = 2 }");
        assert!(diags.is_empty(), "{diags:?}");
    }

    #[test]
    fn flags_integers_that_read_as_one_double() {
        let source = "local t = { [9007199254740993] = 1, [9007199254740992] = 2 }";
        let diags = run_rule(&DuplicateKeys, source, LuaVersion::Lua51);
        assert_eq!(diags.len(), 1, "{diags:?}");
    }

    #[test]
    fn flags_keys_spelled_in_hex_or_long_brackets() {
        let diags = run("local t = { [0x10] = 1, [16.0] = 2, x = 3, [ [[x]] ] = 4 }");
        assert_eq!(diags.len(), 2, "{diags:?}");
    }
}
