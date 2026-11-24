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
    use luck_token::LuaTarget;

    use crate::fixtures::Tester;
    use crate::rules::RuleId;

    const PASS: &[(&str, LuaTarget)] = &[
        ("local t = { 1, 1, a = 1, b = 1, [1] = 2, [x] = 3, [x] = 4 }", LuaTarget::Lua54),
        ("local t = { [9007199254740993] = 1, [9007199254740992] = 2 }", LuaTarget::Lua54),
    ];

    const FAIL: &[(&str, LuaTarget)] = &[
        ("local t = { a = 1, b = 2, a = 3 }", LuaTarget::Lua54),
        (r#"local t = { a = 1, ["a"] = 2 }"#, LuaTarget::Lua54),
        (r#"local t = { a = 1, ["\97"] = 2 }"#, LuaTarget::Lua54),
        ("local t = { a = 1, a = 2, a = 3 }", LuaTarget::Lua54),
        // A `f{}` table is a table constructor.
        ("foo{ a = 1, a = 2 }", LuaTarget::Lua54),
        // Both fields set key 1.
        (r#"local t = { [1] = "a", [1] = "b" }"#, LuaTarget::Lua54),
        (r#"local t = { [1] = "a", [1.0] = "b", [-2] = "c", [-2.0] = "d" }"#, LuaTarget::Lua54),
        // NaN is no key.
        (
            "local t = { [2] = 1, [1 + 1] = 2, [0] = 3, [-0.0] = 4, [0/0] = 5, [0/0] = 6 }",
            LuaTarget::Lua54,
        ),
        ("local t = { [9007199254740993] = 1, [9007199254740992] = 2 }", LuaTarget::Lua51),
        ("local t = { [0x10] = 1, [16.0] = 2, x = 3, [ [[x]] ] = 4 }", LuaTarget::Lua54),
        ("local t = { a = 1, a = 2 }", LuaTarget::Lua54),
    ];

    #[test]
    fn test() {
        Tester::new(RuleId::DuplicateKeys, PASS, FAIL).test_and_snapshot();
    }
}
