use luck_ast::{BinaryOp, Expression, FunctionArgs, FunctionCall, Literal};
use luck_semantic::SemanticAnalysis;
use luck_token::{BinOp, LuaVersion, short_string_body};

use crate::call::is_global_call;
use crate::diagnostic::Finding;
use crate::node_type::{NodeType, NodeTypeSet};
use crate::rule::{LintContext, NodeRule, impl_rule};
use crate::suggest::closest;

#[derive(Debug, Clone, Copy)]
pub(crate) struct UnknownType;

impl_rule!(
    UnknownType,
    "unknown_type",
    Correctness,
    Warning,
    "`type()` or `typeof()` result compared against an unknown type name"
);

impl NodeRule for UnknownType {
    fn node_types(&self) -> NodeTypeSet {
        NodeTypeSet::from_types(&[NodeType::BinaryOp])
    }
    fn on_expr(&self, expr: &Expression, context: &LintContext<'_>, findings: &mut Vec<Finding>) {
        let Expression::BinaryOp(binary_op) = expr else {
            return;
        };
        if !matches!(binary_op.op, BinOp::Eq | BinOp::Ne) {
            return;
        }
        let Some((callee, literal)) = classify_comparison(binary_op, context.semantic) else {
            return;
        };
        // An escape could spell a valid name, so only plain text is checked.
        let Some(name) = short_string_body(&literal.text).filter(|body| !body.contains('\\'))
        else {
            return;
        };
        let primitives = primitive_type_names(context.semantic.version);
        if primitives.contains(&name) {
            return;
        }
        // Roblox `typeof` also returns datatype names (`"Instance"`,
        // `"Vector3"`) that aren't in this list, so only case typos of a
        // primitive are flagged there.
        let is_roblox_typeof = callee == "typeof" && context.semantic.environment.is_roblox();
        let case_match = primitives.iter().find(|primitive| primitive.eq_ignore_ascii_case(name));
        if is_roblox_typeof && case_match.is_none() {
            return;
        }
        let suggestion = case_match
            .copied()
            .or_else(|| closest(name, primitives.iter().copied(), |distance, _| distance <= 2));
        let mut finding = Finding::new(
            format!("`{name}` is not a value that `{callee}` can return"),
            literal.span,
        );
        if let Some(primitive) = suggestion {
            finding = finding.with_help(format!("did you mean `{primitive}`?"));
        }
        findings.push(finding);
    }
}

fn classify_comparison<'ast>(
    binary_op: &'ast BinaryOp,
    semantic: &SemanticAnalysis,
) -> Option<(&'static str, &'ast Literal)> {
    let sides = [(&binary_op.lhs, &binary_op.rhs), (&binary_op.rhs, &binary_op.lhs)];
    for (call_side, literal_side) in sides {
        let Expression::FunctionCall(call) = call_side else {
            continue;
        };
        let Some(callee) = type_function_callee(call, semantic) else {
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
fn type_function_callee(call: &FunctionCall, semantic: &SemanticAnalysis) -> Option<&'static str> {
    let callee = if is_global_call(call, "type", semantic) {
        "type"
    } else if semantic.version.is_luau() && is_global_call(call, "typeof", semantic) {
        "typeof"
    } else {
        return None;
    };
    let FunctionArgs::Parenthesized { args, .. } = &call.args else {
        return None;
    };
    (args.len() == 1).then_some(callee)
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
    use luck_token::LuaVersion;

    use super::UnknownType;
    use crate::diagnostic::LintDiagnostic;
    use crate::fixtures::{run_rule, run_rule_roblox};

    fn run(source: &str, version: LuaVersion) -> Vec<LintDiagnostic> {
        run_rule(&UnknownType, source, version)
    }

    #[test]
    fn flags_misspelled_type_name() {
        let diags = run("local ok = type(x) == \"strng\"", LuaVersion::Lua54);
        assert_eq!(diags.len(), 1, "{diags:?}");
        assert!(diags[0].help.as_deref() == Some("did you mean `string`?"), "{diags:?}");
    }

    #[test]
    fn flags_typo_with_the_nearest_primitive() {
        // Within 2 of `number`, which comes first, and within 1 of `buffer`.
        let diags = run("local ok = type(x) == \"nuffer\"", LuaVersion::Luau);
        assert_eq!(diags.len(), 1, "{diags:?}");
        assert!(diags[0].help.as_deref() == Some("did you mean `buffer`?"), "{diags:?}");
    }

    #[test]
    fn flags_reversed_operands() {
        let diags = run("local ok = \"nll\" == type(x)", LuaVersion::Lua54);
        assert_eq!(diags.len(), 1, "{diags:?}");
    }

    #[test]
    fn flags_not_equal_comparison() {
        let diags = run("local ok = type(x) ~= \"Number\"", LuaVersion::Lua54);
        assert_eq!(diags.len(), 1, "{diags:?}");
        assert!(diags[0].help.as_deref() == Some("did you mean `number`?"), "{diags:?}");
    }

    #[test]
    fn flags_buffer_outside_luau() {
        // `buffer` is a Luau-only primitive; PUC type() never returns it.
        let diags = run("local ok = type(x) == \"buffer\"", LuaVersion::Lua54);
        assert_eq!(diags.len(), 1, "{diags:?}");
    }

    #[test]
    fn flags_typeof_typo_in_luau() {
        let diags = run("local ok = typeof(x) == \"tabel\"", LuaVersion::Luau);
        assert_eq!(diags.len(), 1, "{diags:?}");
    }

    #[test]
    fn ignores_valid_primitives() {
        for name in ["nil", "boolean", "number", "string", "table", "function"] {
            let diags = run(&format!("local ok = type(x) == \"{name}\""), LuaVersion::Lua54);
            assert!(diags.is_empty(), "{name}: {diags:?}");
        }
    }

    #[test]
    fn ignores_buffer_in_luau() {
        let diags = run("local ok = type(x) == \"buffer\"", LuaVersion::Luau);
        assert!(diags.is_empty(), "{diags:?}");
    }

    #[test]
    fn ignores_typeof_outside_luau() {
        // `typeof` is not a Lua 5.4 builtin; nothing to validate.
        let diags = run("local ok = typeof(x) == \"strng\"", LuaVersion::Lua54);
        assert!(diags.is_empty(), "{diags:?}");
    }

    #[test]
    fn ignores_shadowed_type() {
        let diags =
            run("local type = function() end\nlocal ok = type(x) == \"strng\"", LuaVersion::Lua54);
        assert!(diags.is_empty(), "{diags:?}");
    }

    #[test]
    fn ignores_roblox_class_names_in_typeof() {
        let diags = run_rule_roblox(&UnknownType, "local ok = typeof(x) == \"Instance\"");
        assert!(diags.is_empty(), "{diags:?}");
    }

    #[test]
    fn flags_roblox_typeof_case_typo() {
        let diags = run_rule_roblox(&UnknownType, "local ok = typeof(x) == \"Number\"");
        assert_eq!(diags.len(), 1, "{diags:?}");
    }

    #[test]
    fn ignores_non_type_comparison() {
        let diags = run("local ok = name == \"strng\"", LuaVersion::Lua54);
        assert!(diags.is_empty(), "{diags:?}");
    }

    #[test]
    fn ignores_dynamic_comparison() {
        let diags = run("local ok = type(x) == expected", LuaVersion::Lua54);
        assert!(diags.is_empty(), "{diags:?}");
    }
}
