use luck_ast::{AstKind, AstType, Call, Expression, StringLiteral};
use luck_semantic::{AstNode, AstTypesBitset};
use luck_stdlib::SemanticAnalysis;
use luck_token::{BinOp, LuaVersion};

use crate::binary::operator_pairs;
use crate::call::{CallArgs, is_global_call};
use crate::context::LintContext;
use crate::diagnostic::Finding;
use crate::rule::{Rule, declare_lint_rule};
use crate::suggest::closest;

#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct UnknownType;

declare_lint_rule!(
    UnknownType,
    Correctness,
    Warning,
    "`type()` or `typeof()` result compared against an unknown type name"
);

impl Rule for UnknownType {
    const NODE_TYPES: AstTypesBitset = AstTypesBitset::from_types(&[AstType::BinaryOp]);

    fn run<'ast>(&self, node: &AstNode<'ast>, context: &LintContext<'ast>) {
        let AstKind::BinaryOp(run) = node.kind() else {
            return;
        };
        if !run.links.first().op.is_comparison() {
            return;
        }
        for pair in operator_pairs(run) {
            if matches!(pair.op, BinOp::Eq | BinOp::Ne)
                && let (Some(lhs), Some(rhs)) = (pair.lhs, pair.rhs)
                && let Some((callee, literal)) = classify_comparison(lhs, rhs, context.semantic)
            {
                check_literal(callee, literal, context);
            }
        }
    }
}

fn check_literal(callee: &str, literal: &StringLiteral<'_>, context: &LintContext<'_>) {
    let Some(name) = literal.as_str() else {
        return;
    };
    let primitives = primitive_type_names(context.semantic.version());
    if primitives.contains(&name) {
        return;
    }
    // Roblox `typeof` also returns datatype names (`"Instance"`,
    // `"Vector3"`) that aren't in this list, so only case typos of a
    // primitive are flagged there.
    let is_roblox_typeof = callee == "typeof" && context.semantic.target().is_roblox();
    let case_match = primitives.iter().find(|primitive| primitive.eq_ignore_ascii_case(name));
    if is_roblox_typeof && case_match.is_none() {
        return;
    }
    let suggestion = case_match
        .copied()
        .or_else(|| closest(name, primitives.iter().copied(), |distance, _| distance <= 2));
    let mut finding =
        Finding::new(format!("`{name}` is not a value that `{callee}` can return"), literal.span);
    if let Some(primitive) = suggestion {
        finding = finding.with_help(format!("did you mean `{primitive}`?"));
    }
    context.diagnostic(finding);
}

fn classify_comparison<'ast>(
    lhs: &'ast Expression<'ast>,
    rhs: &'ast Expression<'ast>,
    semantic: &SemanticAnalysis<'_>,
) -> Option<(&'static str, &'ast StringLiteral<'ast>)> {
    for (call_side, literal_side) in [(lhs, rhs), (rhs, lhs)] {
        let Expression::Suffixed(chain) = call_side else {
            continue;
        };
        let Some(callee) = chain.last_call().and_then(|call| type_function_callee(call, semantic))
        else {
            continue;
        };
        let Expression::StringLiteral(literal) = literal_side else {
            continue;
        };
        return Some((callee, literal));
    }
    None
}

/// `type`, or `typeof` on Luau, when `call` is a one-argument call to that
/// global.
fn type_function_callee(call: Call<'_>, semantic: &SemanticAnalysis<'_>) -> Option<&'static str> {
    let callee = if is_global_call(call, "type", semantic) {
        "type"
    } else if semantic.version().is_luau() && is_global_call(call, "typeof", semantic) {
        "typeof"
    } else {
        return None;
    };
    (CallArgs::new(call.args).len() == 1).then_some(callee)
}

const fn primitive_type_names(version: LuaVersion) -> &'static [&'static str] {
    if version.is_luau() {
        &[
            "nil", "boolean", "number", "string", "table", "function", "thread", "userdata",
            "buffer", "vector", "integer",
        ]
    } else {
        &["nil", "boolean", "number", "string", "table", "function", "thread", "userdata"]
    }
}

#[cfg(test)]
mod tests {
    use luck_token::LuaTarget;

    use crate::fixtures::Tester;
    use crate::rules::RuleId;

    const PASS: &[(&str, LuaTarget)] = &[
        (r#"local ok = type(x) == "nil""#, LuaTarget::Lua54),
        (r#"local ok = type(x) == "boolean""#, LuaTarget::Lua54),
        (r#"local ok = type(x) == "number""#, LuaTarget::Lua54),
        (r#"local ok = type(x) == "string""#, LuaTarget::Lua54),
        (r#"local ok = type(x) == "table""#, LuaTarget::Lua54),
        (r#"local ok = type(x) == "function""#, LuaTarget::Lua54),
        (r#"local ok = type(x) == "buffer""#, LuaTarget::Luau),
        // `typeof` isn't a Lua 5.4 builtin, so there's nothing to check.
        (r#"local ok = typeof(x) == "strng""#, LuaTarget::Lua54),
        (
            "\
local type = function() end
local ok = type(x) == \"strng\"",
            LuaTarget::Lua54,
        ),
        (r#"local ok = typeof(x) == "Instance""#, LuaTarget::LuauRoblox),
        (r#"local ok = name == "strng""#, LuaTarget::Lua54),
        ("local ok = type(x) == expected", LuaTarget::Lua54),
    ];

    const FAIL: &[(&str, LuaTarget)] = &[
        (r#"local ok = type(x) == "strng""#, LuaTarget::Lua54),
        // Within 2 of `number`, which comes first, and within 1 of `buffer`.
        (r#"local ok = type(x) == "nuffer""#, LuaTarget::Luau),
        (r#"local ok = "nll" == type(x)"#, LuaTarget::Lua54),
        (r#"local ok = type(x) ~= "Number""#, LuaTarget::Lua54),
        // `buffer` is a Luau-only primitive, which PUC Lua's `type` never returns.
        (r#"local ok = type(x) == "buffer""#, LuaTarget::Lua54),
        (r#"local ok = type"x" == "strng""#, LuaTarget::Lua54),
        (r#"local ok = type{} == "strng""#, LuaTarget::Lua54),
        (r#"local ok = typeof(x) == "tabel""#, LuaTarget::Luau),
        (r#"local ok = typeof(x) == "Number""#, LuaTarget::LuauRoblox),
    ];

    #[test]
    fn test() {
        Tester::new(RuleId::UnknownType, PASS, FAIL).test_and_snapshot();
    }
}
