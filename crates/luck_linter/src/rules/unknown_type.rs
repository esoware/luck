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

    fn run<'ast>(&self, node: &AstNode<'ast>, context: &LintContext<'_, 'ast>) {
        let AstKind::BinaryOp(run) = node.kind() else {
            return;
        };
        if !run.links.first().op.is_comparison() {
            return;
        }
        for pair in operator_pairs(run) {
            if matches!(pair.op, BinOp::Eq | BinOp::Ne)
                && let (Some(lhs), Some(rhs)) = (pair.lhs, pair.rhs)
                && let Some((function, literal)) = type_comparison(lhs, rhs, context.semantic)
            {
                check_literal(function, literal, context);
            }
        }
    }
}

/// The global whose result a comparison checks against a type name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TypeFunction {
    Type,
    Typeof,
}

impl TypeFunction {
    /// The one `call` calls with one argument: `type`, or on Luau `typeof`.
    fn of_call(call: Call<'_>, semantic: &SemanticAnalysis<'_>) -> Option<Self> {
        let scoping = semantic.scoping();
        let function = if is_global_call(call, "type", scoping) {
            Self::Type
        } else if semantic.version().is_luau() && is_global_call(call, "typeof", scoping) {
            Self::Typeof
        } else {
            return None;
        };
        (CallArgs::new(call.args).len() == 1).then_some(function)
    }

    const fn name(self) -> &'static str {
        match self {
            Self::Type => "type",
            Self::Typeof => "typeof",
        }
    }
}

fn check_literal(
    function: TypeFunction,
    literal: &StringLiteral<'_>,
    context: &LintContext<'_, '_>,
) {
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
    let is_roblox_typeof =
        function == TypeFunction::Typeof && context.semantic.target().is_roblox();
    let case_match = primitives.iter().find(|primitive| primitive.eq_ignore_ascii_case(name));
    if is_roblox_typeof && case_match.is_none() {
        return;
    }
    let suggestion = case_match
        .copied()
        .or_else(|| closest(name, primitives.iter().copied(), |distance, _| distance <= 2));
    let mut finding = Finding::new(
        format!("`{name}` is not a value that `{}` can return", function.name()),
        literal.span,
    );
    if let Some(primitive) = suggestion {
        finding = finding.with_help(format!("did you mean `{primitive}`?"));
    }
    context.diagnostic(finding);
}

/// The type function one side of `lhs` and `rhs` calls, and the string
/// literal the other side is.
fn type_comparison<'ast>(
    lhs: &'ast Expression<'ast>,
    rhs: &'ast Expression<'ast>,
    semantic: &SemanticAnalysis<'_>,
) -> Option<(TypeFunction, &'ast StringLiteral<'ast>)> {
    for (call_side, literal_side) in [(lhs, rhs), (rhs, lhs)] {
        let Expression::Suffixed(chain) = call_side else {
            continue;
        };
        let Some(function) =
            chain.last_call().and_then(|call| TypeFunction::of_call(call, semantic))
        else {
            continue;
        };
        let Expression::StringLiteral(literal) = literal_side else {
            continue;
        };
        return Some((function, literal));
    }
    None
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
