use luck_ast::Expression;
use luck_ast::expr::{FunctionArgs, FunctionCall, Var};
use luck_ast::stmt::Statement;
use luck_token::TokenKind;
use luck_token::{BinOp, UnOp};

use crate::diagnostic::{Category, LintDiagnostic, Severity};
use crate::rule::{LintContext, NodeRule, Rule};
use luck_ast::node::{AstTypesBitset, NodeType};

pub struct TableOperations;

impl Rule for TableOperations {
    fn name(&self) -> &'static str {
        "table_operations"
    }

    fn category(&self) -> Category {
        Category::Suspicious
    }

    fn default_severity(&self) -> Severity {
        Severity::Warning
    }

    fn description(&self) -> &'static str {
        "Misuse of table.insert, table.remove, table.move, or table.create."
    }

    fn check(&self, ctx: &LintContext) -> Vec<LintDiagnostic> {
        crate::bus::run_single(self, ctx)
    }
}

impl NodeRule for TableOperations {
    fn node_types(&self) -> Option<&'static AstTypesBitset> {
        static TYPES: AstTypesBitset =
            AstTypesBitset::from_types(&[NodeType::FunctionCallStmt, NodeType::FunctionCallExpr]);
        Some(&TYPES)
    }
    fn on_statement(&self, stmt: &Statement, ctx: &LintContext, out: &mut Vec<LintDiagnostic>) {
        if let Statement::FunctionCall(call_stmt) = stmt {
            check_call(&call_stmt.call, ctx, out);
        }
    }

    fn on_expression(&self, expr: &Expression, ctx: &LintContext, out: &mut Vec<LintDiagnostic>) {
        if let Expression::FunctionCall(call) = expr {
            check_call(call, ctx, out);
        }
    }
}

fn check_call(call: &FunctionCall, ctx: &LintContext, out: &mut Vec<LintDiagnostic>) {
    let Some(func) = table_stdlib_function(call, ctx) else {
        return;
    };
    let FunctionArgs::Parenthesized { args, .. } = &call.args else {
        return;
    };
    let args: Vec<&Expression> = args.iter().collect();
    let diag = |message: &str| LintDiagnostic::new("table_operations", message, call.span);
    match func {
        "insert" if args.len() == 3 => {
            if is_number_literal(args[1], 0.0) {
                out.push(
                    diag("table.insert with index 0; Lua arrays are 1-based")
                        .with_help("did you mean index 1?".to_string()),
                );
            } else if length_operand(args[1]).is_some_and(|target| same_target(target, args[0])) {
                out.push(
                    diag("table.insert(t, #t, v) inserts the value before the last element")
                        .with_help("to append, use table.insert(t, v) or index #t + 1".to_string()),
                );
            } else if length_plus_one_operand(args[1])
                .is_some_and(|target| same_target(target, args[0]))
            {
                out.push(
                    diag("table.insert(t, #t + 1, v) appends; the index argument is redundant")
                        .with_help("use table.insert(t, v) instead".to_string()),
                );
            }
        }
        "remove" if args.len() == 2 => {
            if is_number_literal(args[1], 0.0) {
                out.push(
                    diag("table.remove with index 0; Lua arrays are 1-based")
                        .with_help("did you mean index 1?".to_string()),
                );
            } else if length_minus_one_operand(args[1])
                .is_some_and(|target| same_target(target, args[0]))
            {
                out.push(
                    diag("table.remove(t, #t - 1) removes the value before the last element")
                        .with_help("table.remove(t) removes the last element".to_string()),
                );
            }
        }
        "move" => {
            if args.len() >= 2 && is_number_literal(args[1], 0.0) {
                out.push(diag(
                    "table.move with start index 0; Lua arrays are 1-based",
                ));
            }
            if args.len() >= 4 && is_number_literal(args[3], 0.0) {
                out.push(diag(
                    "table.move with destination index 0; Lua arrays are 1-based",
                ));
            }
        }
        "create" if args.len() == 2 => {
            if matches!(args[1], Expression::TableConstructor(_)) {
                out.push(
                    diag("table.create(n, {...}) fills every slot with the same table object")
                        .with_help(
                            "use a for loop to create a distinct table per element".to_string(),
                        ),
                );
            }
        }
        _ => {}
    }
}

/// If `call` is `table.<field>(...)` where `table` is the real stdlib
/// global (not a shadowing local) and `<field>` exists in the current
/// version's stdlib, return the field name.
fn table_stdlib_function<'a>(call: &'a FunctionCall, ctx: &LintContext) -> Option<&'a str> {
    if call.method.is_some() {
        return None;
    }
    let Expression::Var(var) = &call.callee else {
        return None;
    };
    let Var::FieldAccess(fa) = var.as_ref() else {
        return None;
    };
    let Expression::Var(prefix_var) = &fa.prefix else {
        return None;
    };
    let Var::Name(prefix_token) = prefix_var.as_ref() else {
        return None;
    };
    let TokenKind::Identifier(prefix_name) = &prefix_token.kind else {
        return None;
    };
    if prefix_name.as_str() != "table"
        || ctx
            .semantic
            .resolves_to_local(prefix_name.as_str(), prefix_token.span)
    {
        return None;
    }
    let TokenKind::Identifier(field) = &fa.name.kind else {
        return None;
    };
    ctx.semantic.lookup_stdlib_str(&["table", field.as_str()])?;
    Some(field.as_str())
}

fn is_number_literal(expr: &Expression, want: f64) -> bool {
    let Expression::Number(literal) = expr else {
        return false;
    };
    literal.text.parse::<f64>() == Ok(want)
}

/// Recognize `#operand` and return the operand.
fn length_operand(expr: &Expression) -> Option<&Expression> {
    let Expression::UnaryOp(unop) = expr else {
        return None;
    };
    (unop.op == UnOp::Len).then_some(&unop.operand)
}

/// Recognize `#operand + 1` and return the operand.
fn length_plus_one_operand(expr: &Expression) -> Option<&Expression> {
    let Expression::BinaryOp(binop) = expr else {
        return None;
    };
    if !matches!(binop.op, BinOp::Add) || !is_number_literal(&binop.right, 1.0) {
        return None;
    }
    length_operand(&binop.left)
}

/// Recognize `#operand - 1` and return the operand.
fn length_minus_one_operand(expr: &Expression) -> Option<&Expression> {
    let Expression::BinaryOp(binop) = expr else {
        return None;
    };
    if !matches!(binop.op, BinOp::Sub) || !is_number_literal(&binop.right, 1.0) {
        return None;
    }
    length_operand(&binop.left)
}

/// Structural identity for the length-operand checks: the same bare
/// identifier or the same dotted path. Anything dynamic (indexing,
/// calls) is not comparable, so it never matches.
fn same_target(a: &Expression, b: &Expression) -> bool {
    let (Expression::Var(a), Expression::Var(b)) = (a, b) else {
        return false;
    };
    match (a.as_ref(), b.as_ref()) {
        (Var::Name(a), Var::Name(b)) => match (&a.kind, &b.kind) {
            (TokenKind::Identifier(a_name), TokenKind::Identifier(b_name)) => a_name == b_name,
            _ => false,
        },
        (Var::FieldAccess(a), Var::FieldAccess(b)) => match (&a.name.kind, &b.name.kind) {
            (TokenKind::Identifier(a_name), TokenKind::Identifier(b_name)) => {
                a_name == b_name && same_target(&a.prefix, &b.prefix)
            }
            _ => false,
        },
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use luck_token::LuaVersion;

    use super::TableOperations;
    use crate::diagnostic::LintDiagnostic;

    fn run(source: &str, version: LuaVersion) -> Vec<LintDiagnostic> {
        crate::test_support::run_rule(&TableOperations, source, version)
    }

    #[test]
    fn flags_insert_index_zero() {
        let diags = run("table.insert(t, 0, v)", LuaVersion::Lua54);
        assert_eq!(diags.len(), 1, "{diags:?}");
        assert!(diags[0].message.contains("1-based"), "{diags:?}");
    }

    #[test]
    fn flags_insert_length_index() {
        let diags = run("table.insert(t, #t, v)", LuaVersion::Lua54);
        assert_eq!(diags.len(), 1, "{diags:?}");
        assert!(diags[0].message.contains("before the last"), "{diags:?}");
    }

    #[test]
    fn flags_insert_length_plus_one() {
        let diags = run("table.insert(t, #t + 1, v)", LuaVersion::Lua54);
        assert_eq!(diags.len(), 1, "{diags:?}");
        assert!(diags[0].message.contains("redundant"), "{diags:?}");
    }

    #[test]
    fn flags_insert_dotted_path_identity() {
        let diags = run("table.insert(a.b, #a.b, v)", LuaVersion::Lua54);
        assert_eq!(diags.len(), 1, "{diags:?}");
    }

    #[test]
    fn flags_remove_index_zero() {
        let diags = run("table.remove(t, 0)", LuaVersion::Lua54);
        assert_eq!(diags.len(), 1, "{diags:?}");
    }

    #[test]
    fn flags_remove_length_minus_one() {
        let diags = run("table.remove(t, #t - 1)", LuaVersion::Lua54);
        assert_eq!(diags.len(), 1, "{diags:?}");
    }

    #[test]
    fn flags_move_start_index_zero() {
        let diags = run("table.move(t, 0, #t, 1)", LuaVersion::Lua54);
        assert_eq!(diags.len(), 1, "{diags:?}");
    }

    #[test]
    fn flags_move_destination_index_zero() {
        let diags = run("table.move(t, 1, #t, 0, u)", LuaVersion::Lua54);
        assert_eq!(diags.len(), 1, "{diags:?}");
    }

    #[test]
    fn flags_create_with_table_literal() {
        let diags = run("local x = table.create(3, {})", LuaVersion::Luau);
        assert_eq!(diags.len(), 1, "{diags:?}");
        assert!(diags[0].message.contains("same table"), "{diags:?}");
    }

    #[test]
    fn flags_call_in_expression_position() {
        let diags = run("local last = table.remove(t, 0)", LuaVersion::Lua54);
        assert_eq!(diags.len(), 1, "{diags:?}");
    }

    #[test]
    fn ignores_plain_append() {
        let diags = run("table.insert(t, v)", LuaVersion::Lua54);
        assert!(diags.is_empty(), "{diags:?}");
    }

    #[test]
    fn ignores_insert_index_one() {
        let diags = run("table.insert(t, 1, v)", LuaVersion::Lua54);
        assert!(diags.is_empty(), "{diags:?}");
    }

    #[test]
    fn ignores_length_of_different_table() {
        let diags = run("table.insert(t, #u, v)", LuaVersion::Lua54);
        assert!(diags.is_empty(), "{diags:?}");
    }

    #[test]
    fn ignores_length_of_different_path() {
        let diags = run("table.insert(a.b, #a.c, v)", LuaVersion::Lua54);
        assert!(diags.is_empty(), "{diags:?}");
    }

    #[test]
    fn ignores_shadowed_table_local() {
        let diags = run("local table = {}\ntable.insert(t, 0, v)", LuaVersion::Lua54);
        assert!(diags.is_empty(), "{diags:?}");
    }

    #[test]
    fn ignores_valid_move() {
        let diags = run("table.move(t, 1, #t, 1, u)", LuaVersion::Lua54);
        assert!(diags.is_empty(), "{diags:?}");
    }

    #[test]
    fn ignores_create_with_scalar_fill() {
        let diags = run("local x = table.create(3, 0)", LuaVersion::Luau);
        assert!(diags.is_empty(), "{diags:?}");
    }
}
