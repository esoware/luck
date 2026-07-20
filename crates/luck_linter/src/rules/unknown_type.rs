use luck_ast::Expression;
use luck_ast::expr::{BinaryOp, FunctionArgs, FunctionCall, Var};
use luck_token::BinOp;
use luck_token::{Span, StdlibEnvironment, TokenKind};

use crate::diagnostic::{Category, LintDiagnostic, Severity};
use crate::rule::{LintContext, NodeRule, Rule};
use luck_ast::node::{AstTypesBitset, NodeType};

pub struct UnknownType;

impl Rule for UnknownType {
    fn name(&self) -> &'static str {
        "unknown_type"
    }

    fn category(&self) -> Category {
        Category::Correctness
    }

    fn default_severity(&self) -> Severity {
        Severity::Warning
    }

    fn description(&self) -> &'static str {
        "type() or typeof() result compared against an unknown type name."
    }

    fn check(&self, ctx: &LintContext) -> Vec<LintDiagnostic> {
        crate::bus::run_single(self, ctx)
    }
}

impl NodeRule for UnknownType {
    fn node_types(&self) -> Option<&'static AstTypesBitset> {
        static TYPES: AstTypesBitset = AstTypesBitset::from_types(&[NodeType::BinaryOp]);
        Some(&TYPES)
    }
    fn on_expression(&self, expr: &Expression, ctx: &LintContext, out: &mut Vec<LintDiagnostic>) {
        let Expression::BinaryOp(binop) = expr else {
            return;
        };
        if !matches!(binop.op, BinOp::Eq | BinOp::Ne) {
            return;
        }
        let Some((callee, literal_span)) = classify_comparison(binop, ctx) else {
            return;
        };
        let Some(name) = literal_content(ctx.source, literal_span) else {
            return;
        };
        let primitives = primitive_type_names(ctx);
        if primitives.contains(&name) {
            return;
        }
        // Roblox typeof() yields class names (`"Instance"`, `"Vector3"`),
        // which no static list can enumerate; only case typos of true
        // primitives are safe to flag there.
        let roblox_typeof =
            callee == "typeof" && ctx.semantic.environment == StdlibEnvironment::Roblox;
        let case_match = primitives
            .iter()
            .find(|primitive| primitive.eq_ignore_ascii_case(name));
        if roblox_typeof && case_match.is_none() {
            return;
        }
        let suggestion = case_match.copied().or_else(|| {
            primitives
                .iter()
                .copied()
                .find(|primitive| edit_distance(primitive, name) <= 2)
        });
        let mut diag = LintDiagnostic::new(
            "unknown_type",
            format!("'{name}' is not a value that `{callee}` can return"),
            literal_span,
        );
        if let Some(suggestion) = suggestion {
            diag = diag.with_help(format!("did you mean '{suggestion}'?"));
        }
        out.push(diag);
    }
}

/// Match `type(x) == "literal"` in either operand order. Returns the
/// callee name (`type`/`typeof`) and the string literal's span.
fn classify_comparison<'a>(binop: &'a BinaryOp, ctx: &LintContext) -> Option<(&'a str, Span)> {
    let sides = [(&binop.left, &binop.right), (&binop.right, &binop.left)];
    for (call_side, literal_side) in sides {
        let Expression::FunctionCall(call) = call_side else {
            continue;
        };
        let Some(callee) = type_function_callee(call, ctx) else {
            continue;
        };
        let Expression::StringLiteral(token) = literal_side else {
            continue;
        };
        return Some((callee, token.span));
    }
    None
}

/// The call must be a bare `type(x)` or, in Luau, `typeof(x)` with the
/// name resolving to the real global.
fn type_function_callee<'a>(call: &'a FunctionCall, ctx: &LintContext) -> Option<&'a str> {
    if call.method.is_some() {
        return None;
    }
    let Expression::Var(var) = &call.callee else {
        return None;
    };
    let Var::Name(token) = var else {
        return None;
    };
    let TokenKind::Identifier(name) = &token.kind else {
        return None;
    };
    let is_type_fn = match name.as_str() {
        "type" => true,
        "typeof" => ctx.semantic.version.is_luau(),
        _ => false,
    };
    if !is_type_fn || ctx.semantic.resolves_to_local(name.as_str(), token.span) {
        return None;
    }
    let FunctionArgs::Parenthesized { args, .. } = &call.args else {
        return None;
    };
    if args.iter().count() != 1 {
        return None;
    }
    Some(name.as_str())
}

fn primitive_type_names(ctx: &LintContext) -> &'static [&'static str] {
    if ctx.semantic.version.is_luau() {
        &[
            "nil", "boolean", "number", "string", "table", "function", "thread", "userdata",
            "buffer", "vector",
        ]
    } else {
        &[
            "nil", "boolean", "number", "string", "table", "function", "thread", "userdata",
        ]
    }
}

/// Quoted literal content without unescaping; long-bracket strings are
/// skipped (their content can't hold a type name typo worth chasing).
fn literal_content(source: &str, span: Span) -> Option<&str> {
    let raw = &source[span.start as usize..span.end as usize];
    let bytes = raw.as_bytes();
    if bytes.len() < 2 || !matches!(bytes[0], b'"' | b'\'') {
        return None;
    }
    let inner = &raw[1..raw.len() - 1];
    inner.bytes().all(|b| b != b'\\').then_some(inner)
}

fn edit_distance(a: &str, b: &str) -> usize {
    let a: Vec<u8> = a.bytes().collect();
    let b: Vec<u8> = b.bytes().collect();
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    let mut current = vec![0; b.len() + 1];
    for (i, &a_byte) in a.iter().enumerate() {
        current[0] = i + 1;
        for (j, &b_byte) in b.iter().enumerate() {
            let substitution = prev[j] + usize::from(a_byte != b_byte);
            current[j + 1] = substitution.min(prev[j + 1] + 1).min(current[j] + 1);
        }
        std::mem::swap(&mut prev, &mut current);
    }
    prev[b.len()]
}

#[cfg(test)]
mod tests {
    use luck_token::LuaVersion;

    use super::UnknownType;
    use crate::diagnostic::LintDiagnostic;

    fn run(source: &str, version: LuaVersion) -> Vec<LintDiagnostic> {
        crate::test_support::run_rule(&UnknownType, source, version)
    }

    #[test]
    fn flags_misspelled_type_name() {
        let diags = run("local ok = type(x) == \"strng\"", LuaVersion::Lua54);
        assert_eq!(diags.len(), 1, "{diags:?}");
        assert!(
            diags[0].help.as_deref() == Some("did you mean 'string'?"),
            "{diags:?}"
        );
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
        assert!(
            diags[0].help.as_deref() == Some("did you mean 'number'?"),
            "{diags:?}"
        );
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
            let diags = run(
                &format!("local ok = type(x) == \"{name}\""),
                LuaVersion::Lua54,
            );
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
        let diags = run(
            "local type = function() end\nlocal ok = type(x) == \"strng\"",
            LuaVersion::Lua54,
        );
        assert!(diags.is_empty(), "{diags:?}");
    }

    #[test]
    fn ignores_roblox_class_names_in_typeof() {
        let diags = crate::test_support::run_rule_roblox(
            &UnknownType,
            "local ok = typeof(x) == \"Instance\"",
        );
        assert!(diags.is_empty(), "{diags:?}");
    }

    #[test]
    fn flags_roblox_typeof_case_typo() {
        let diags = crate::test_support::run_rule_roblox(
            &UnknownType,
            "local ok = typeof(x) == \"Number\"",
        );
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
