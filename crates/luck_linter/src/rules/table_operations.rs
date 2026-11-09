use luck_ast::{Expression, FunctionArgs, FunctionCall, Var};
use luck_token::{BinOp, LuaVersion, Token, UnOp};

use crate::call::dotted_callee;
use crate::diagnostic::Finding;
use crate::node_type::{NodeType, NodeTypeSet};
use crate::number::is_number_literal;
use crate::rule::{CallPosition, LintContext, NodeRule, impl_rule};

#[derive(Debug, Clone, Copy)]
pub(crate) struct TableOperations;

impl_rule!(
    TableOperations,
    "table_operations",
    Suspicious,
    Warning,
    "misuse of `table.insert`, `table.remove`, `table.move`, or `table.create`"
);

impl NodeRule for TableOperations {
    fn node_types(&self) -> NodeTypeSet {
        NodeTypeSet::from_types(&[NodeType::FunctionCallStmt, NodeType::FunctionCallExpr])
    }
    fn on_call(
        &self,
        call: &FunctionCall,
        _position: CallPosition,
        context: &LintContext<'_>,
        findings: &mut Vec<Finding>,
    ) {
        check_call(call, context, findings);
    }
}

fn check_call(call: &FunctionCall, context: &LintContext<'_>, findings: &mut Vec<Finding>) {
    let Some(function_name) = table_stdlib_function(call, context) else {
        return;
    };
    let FunctionArgs::Parenthesized { args, .. } = &call.args else {
        return;
    };
    let version = context.semantic.version;
    let finding = |message: &str| Finding::new(message, call.span);
    match (function_name, args.items.as_slice()) {
        ("insert", [table, index, _]) => {
            if is_number_literal(index, 0.0, version) {
                findings.push(
                    finding("`table.insert` with index 0, but Lua arrays are 1-based")
                        .with_help("did you mean index 1?"),
                );
                return;
            }
            if length_operand(index).is_some_and(|target| same_target(target, table)) {
                findings.push(
                    finding("`table.insert(t, #t, v)` inserts the value before the last element")
                        .with_help("to append, use `table.insert(t, v)`"),
                );
                return;
            }
            if length_offset_operand(index, BinOp::Add, version)
                .is_some_and(|target| same_target(target, table))
            {
                findings.push(
                    finding(
                        "`table.insert(t, #t + 1, v)` appends, so the index argument is redundant",
                    )
                    .with_help("use `table.insert(t, v)` instead"),
                );
            }
        }
        ("remove", [table, index]) => {
            if is_number_literal(index, 0.0, version) {
                findings.push(
                    finding("`table.remove` with index 0, but Lua arrays are 1-based")
                        .with_help("did you mean index 1?"),
                );
                return;
            }
            if length_offset_operand(index, BinOp::Sub, version)
                .is_some_and(|target| same_target(target, table))
            {
                findings.push(
                    finding("`table.remove(t, #t - 1)` removes the value before the last element")
                        .with_help("`table.remove(t)` removes the last element"),
                );
            }
        }
        ("move", move_args) => {
            if move_args.get(1).is_some_and(|start| is_number_literal(start, 0.0, version)) {
                findings
                    .push(finding("`table.move` with start index 0, but Lua arrays are 1-based"));
            }
            if move_args
                .get(3)
                .is_some_and(|destination| is_number_literal(destination, 0.0, version))
            {
                findings.push(finding(
                    "`table.move` with destination index 0, but Lua arrays are 1-based",
                ));
            }
        }
        ("create", [_, Expression::TableConstructor(_)]) => {
            findings.push(
                finding("`table.create(n, {...})` fills every slot with the same table object")
                    .with_help("use a `for` loop to give each element its own table"),
            );
        }
        _ => {}
    }
}

/// The `field` of a `table.field(...)` call, if `table` is the stdlib global
/// and this version's stdlib has that function.
fn table_stdlib_function<'call>(
    call: &'call FunctionCall,
    context: &LintContext<'_>,
) -> Option<&'call str> {
    let (root, field) = dotted_callee(call)?;
    if root.identifier() != Some("table") || context.semantic.resolves_to_local("table", root.span)
    {
        return None;
    }
    context.semantic.stdlib().lookup(&["table", field])?;
    Some(field)
}

fn length_operand(expr: &Expression) -> Option<&Expression> {
    let Expression::UnaryOp(unary) = expr else {
        return None;
    };
    (unary.op == UnOp::Len).then_some(&unary.operand)
}

fn length_offset_operand(expr: &Expression, op: BinOp, version: LuaVersion) -> Option<&Expression> {
    let Expression::BinaryOp(binary_op) = expr else {
        return None;
    };
    if binary_op.op != op || !is_number_literal(&binary_op.right, 1.0, version) {
        return None;
    }
    length_operand(&binary_op.left)
}

/// Anything else, such as an index or a call, never matches.
fn same_target(left: &Expression, right: &Expression) -> bool {
    let (mut left_expr, mut right_expr) = (left, right);
    loop {
        let (Expression::Var(left_var), Expression::Var(right_var)) = (left_expr, right_expr)
        else {
            return false;
        };
        match (left_var, right_var) {
            (Var::Name(left_token), Var::Name(right_token)) => {
                return same_identifier(left_token, right_token);
            }
            (Var::FieldAccess(left_field), Var::FieldAccess(right_field)) => {
                if !same_identifier(&left_field.name, &right_field.name) {
                    return false;
                }
                left_expr = &left_field.prefix;
                right_expr = &right_field.prefix;
            }
            _ => return false,
        }
    }
}

fn same_identifier(left: &Token, right: &Token) -> bool {
    left.identifier().is_some_and(|name| right.identifier() == Some(name))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diagnostic::LintDiagnostic;
    use crate::fixtures::run_rule;

    fn run(source: &str, version: LuaVersion) -> Vec<LintDiagnostic> {
        run_rule(&TableOperations, source, version)
    }

    #[test]
    fn flags_insert_index_zero() {
        let diags = run("table.insert(t, 0, v)", LuaVersion::Lua54);
        assert_eq!(diags.len(), 1, "{diags:?}");
        assert!(diags[0].message.contains("1-based"), "{diags:?}");
    }

    #[test]
    fn flags_hex_index_zero() {
        for source in [
            "table.insert(t, 0x0, v)",
            "table.remove(t, 0x0)",
            "table.move(t, 0x0, #t, 1)",
            "table.move(t, 1, #t, 0x0, u)",
        ] {
            let diags = run(source, LuaVersion::Lua54);
            assert_eq!(diags.len(), 1, "{source}: {diags:?}");
        }
    }

    #[test]
    fn flags_hex_length_offset() {
        for source in ["table.insert(t, #t + 0x1, v)", "table.remove(t, #t - 0x1)"] {
            let diags = run(source, LuaVersion::Lua54);
            assert_eq!(diags.len(), 1, "{source}: {diags:?}");
        }
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
    fn flags_call_in_expr_position() {
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
