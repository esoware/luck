use luck_ast::{Expression, Field, FunctionArgs, LastStatement, Statement, Var};
use luck_parser::ParseResult;
use luck_token::LuaVersion;

use crate::common::{assert_no_errors, local_value};

fn parse_lua51(source: &str) -> ParseResult {
    luck_parser::parse(source, LuaVersion::Lua51)
}

#[test]
fn do_end() {
    let result = parse_lua51("do end");
    assert_no_errors(&result);
    assert_eq!(result.block.stmts.len(), 1);
    assert!(matches!(&result.block.stmts[0], Statement::DoBlock(_)));
}

#[test]
fn nested_do_blocks() {
    let result = parse_lua51("do do do end end end");
    assert_no_errors(&result);
    assert_eq!(result.block.stmts.len(), 1);
    if let Statement::DoBlock(outer) = &result.block.stmts[0] {
        assert_eq!(outer.block.stmts.len(), 1);
        if let Statement::DoBlock(middle) = &outer.block.stmts[0] {
            assert_eq!(middle.block.stmts.len(), 1);
            assert!(matches!(&middle.block.stmts[0], Statement::DoBlock(_)));
        } else {
            panic!("expected nested DoBlock");
        }
    } else {
        panic!("expected DoBlock");
    }
}

#[test]
fn while_loop() {
    let result = parse_lua51("while true do end");
    assert_no_errors(&result);
    assert_eq!(result.block.stmts.len(), 1);
    if let Statement::WhileLoop(while_loop) = &result.block.stmts[0] {
        assert!(matches!(&while_loop.condition, Expression::True(_)));
    } else {
        panic!("expected WhileLoop");
    }
}

#[test]
fn repeat_until() {
    let result = parse_lua51("repeat until false");
    assert_no_errors(&result);
    assert_eq!(result.block.stmts.len(), 1);
    if let Statement::RepeatLoop(repeat_loop) = &result.block.stmts[0] {
        assert!(matches!(&repeat_loop.condition, Expression::False(_)));
    } else {
        panic!("expected RepeatLoop");
    }
}

#[test]
fn if_then_end() {
    let result = parse_lua51("if true then end");
    assert_no_errors(&result);
    assert_eq!(result.block.stmts.len(), 1);
    assert!(matches!(&result.block.stmts[0], Statement::IfStatement(_)));
}

#[test]
fn if_elseif_else() {
    let result = parse_lua51("if x then a() elseif y then b() else c() end");
    assert_no_errors(&result);
    if let Statement::IfStatement(if_stmt) = &result.block.stmts[0] {
        assert_eq!(if_stmt.elseif_clauses.len(), 1);
        assert!(if_stmt.else_clause.is_some());
    } else {
        panic!("expected IfStatement");
    }
}

#[test]
fn numeric_for_two_values() {
    let result = parse_lua51("for i = 1, 10 do end");
    assert_no_errors(&result);
    if let Statement::NumericFor(numeric_for) = &result.block.stmts[0] {
        assert!(numeric_for.step.is_none());
    } else {
        panic!("expected NumericFor");
    }
}

#[test]
fn numeric_for_three_values() {
    let result = parse_lua51("for i = 1, 10, 2 do end");
    assert_no_errors(&result);
    if let Statement::NumericFor(numeric_for) = &result.block.stmts[0] {
        assert!(numeric_for.step.is_some());
    } else {
        panic!("expected NumericFor");
    }
}

#[test]
fn generic_for() {
    let result = parse_lua51("for k, v in pairs(t) do end");
    assert_no_errors(&result);
    if let Statement::GenericFor(generic_for) = &result.block.stmts[0] {
        assert_eq!(generic_for.names.len(), 2);
    } else {
        panic!("expected GenericFor");
    }
}

#[test]
fn function_decl_simple() {
    let result = parse_lua51("function foo() end");
    assert_no_errors(&result);
    assert!(matches!(&result.block.stmts[0], Statement::FunctionDecl(_)));
}

#[test]
fn method_declaration() {
    let result = parse_lua51("function a.b:c() end");
    assert_no_errors(&result);
    if let Statement::FunctionDecl(function_decl) = &result.block.stmts[0] {
        assert_eq!(function_decl.name.names.len(), 2); // a, b
        assert!(function_decl.name.method.is_some()); // :c
    } else {
        panic!("expected FunctionDecl");
    }
}

#[test]
fn local_function() {
    let result = parse_lua51("local function foo(x, y) return x end");
    assert_no_errors(&result);
    if let Statement::LocalFunction(local_function) = &result.block.stmts[0] {
        let param_count = local_function.body.params.len();
        assert_eq!(param_count, 2);
    } else {
        panic!("expected LocalFunction");
    }
}

#[test]
fn local_assignment_no_value() {
    let result = parse_lua51("local x");
    assert_no_errors(&result);
    if let Statement::LocalAssignment(local) = &result.block.stmts[0] {
        assert!(local.exprs.is_none());
    } else {
        panic!("expected LocalAssignment");
    }
}

#[test]
fn local_assignment_with_value() {
    let result = parse_lua51("local x = 42");
    assert_no_errors(&result);
    if let Statement::LocalAssignment(local) = &result.block.stmts[0] {
        assert!(local.exprs.is_some());
    } else {
        panic!("expected LocalAssignment");
    }
}

#[test]
fn local_multiple() {
    let result = parse_lua51("local a, b, c = 1, 2, 3");
    assert_no_errors(&result);
    if let Statement::LocalAssignment(local) = &result.block.stmts[0] {
        let name_count = local.names.len();
        assert_eq!(name_count, 3);
    } else {
        panic!("expected LocalAssignment");
    }
}

#[test]
fn simple_assignment() {
    let result = parse_lua51("x = 1");
    assert_no_errors(&result);
    assert!(matches!(&result.block.stmts[0], Statement::Assignment(_)));
}

#[test]
fn multi_assignment() {
    let result = parse_lua51("a, b, c = 1, 2, 3");
    assert_no_errors(&result);
    if let Statement::Assignment(assignment) = &result.block.stmts[0] {
        let target_count = assignment.targets.len();
        assert_eq!(target_count, 3);
    } else {
        panic!("expected Assignment");
    }
}

#[test]
fn assign_nested_field_access() {
    let result = parse_lua51("a.b.c = 1");
    assert_no_errors(&result);
    if let Statement::Assignment(assignment) = &result.block.stmts[0] {
        let target = assignment.targets.last().expect("assignment has target");
        if let Var::FieldAccess(outer) = target {
            if let Expression::Var(inner_var) = &outer.prefix {
                assert!(
                    matches!(inner_var, Var::FieldAccess(_)),
                    "expected nested FieldAccess, got {inner_var:?}"
                );
            } else {
                panic!("expected Var prefix, got {:?}", outer.prefix);
            }
        } else {
            panic!("expected FieldAccess target, got {target:?}");
        }
    } else {
        panic!("expected Assignment");
    }
}

#[test]
fn assign_index_target() {
    let result = parse_lua51("a[i] = 1");
    assert_no_errors(&result);
    if let Statement::Assignment(assignment) = &result.block.stmts[0] {
        let target = assignment.targets.last().expect("assignment has target");
        assert!(matches!(target, Var::Index(_)), "expected Index target, got {target:?}");
    } else {
        panic!("expected Assignment");
    }
}

#[test]
fn assign_mixed_chain_target() {
    let result = parse_lua51("a.b[1]:c(2).d[3] = 5");
    assert_no_errors(&result);
    if let Statement::Assignment(assignment) = &result.block.stmts[0] {
        let target = assignment.targets.last().expect("assignment has target");
        assert!(matches!(target, Var::Index(_)), "expected Index target for [3], got {target:?}");
    } else {
        panic!("expected Assignment");
    }
}

#[test]
fn function_call_stmt() {
    let result = parse_lua51("print(\"hello\")");
    assert_no_errors(&result);
    assert!(matches!(&result.block.stmts[0], Statement::FunctionCall(_)));
}

#[test]
fn method_call_stmt() {
    let result = parse_lua51("obj:method(1, 2)");
    assert_no_errors(&result);
    if let Statement::FunctionCall(call) = &result.block.stmts[0] {
        assert!(call.method.is_some());
    } else {
        panic!("expected FunctionCall");
    }
}

#[test]
fn call_with_string_arg() {
    let result = parse_lua51("require \"foo\"");
    assert_no_errors(&result);
    if let Statement::FunctionCall(call) = &result.block.stmts[0] {
        assert!(matches!(&call.args, FunctionArgs::StringLiteral(_)));
    } else {
        panic!("expected FunctionCall");
    }
}

#[test]
fn call_with_table_arg() {
    let result = parse_lua51("f{1, 2, 3}");
    assert_no_errors(&result);
    if let Statement::FunctionCall(call) = &result.block.stmts[0] {
        assert!(matches!(&call.args, FunctionArgs::TableConstructor(_)));
    } else {
        panic!("expected FunctionCall");
    }
}

#[test]
fn literal_nil() {
    let result = parse_lua51("local x = nil");
    assert_no_errors(&result);
    let expr = local_value(&result);
    assert!(matches!(*expr, Expression::Nil(_)));
}

#[test]
fn literal_true_false() {
    let result = parse_lua51("local a, b = true, false");
    assert_no_errors(&result);
    if let Statement::LocalAssignment(local) = &result.block.stmts[0] {
        let exprs = local.exprs.as_ref().expect("assignment has values");
        let first = &exprs.items[0];
        assert!(matches!(first, Expression::True(_)));
        let second = exprs.last().expect("expression list has last element");
        assert!(matches!(*second, Expression::False(_)));
    } else {
        panic!("expected LocalAssignment");
    }
}

#[test]
fn literal_number() {
    let result = parse_lua51("local x = 3.14");
    assert_no_errors(&result);
    let expr = local_value(&result);
    assert!(matches!(*expr, Expression::Number(_)));
}

#[test]
fn literal_string() {
    let result = parse_lua51("local x = \"hello world\"");
    assert_no_errors(&result);
    let expr = local_value(&result);
    assert!(matches!(*expr, Expression::StringLiteral(_)));
}

#[test]
fn precedence_add_mul() {
    // `1 + 2 * 3` should parse as `1 + (2 * 3)`.
    let result = parse_lua51("local x = 1 + 2 * 3");
    assert_no_errors(&result);
    let expr = local_value(&result);
    if let Expression::BinaryOp(outer) = expr {
        assert!(matches!(outer.op, luck_token::BinOp::Add));
        assert!(
            matches!(&outer.right, Expression::BinaryOp(inner) if matches!(inner.op, luck_token::BinOp::Mul))
        );
    } else {
        panic!("expected BinaryOp, got {expr:?}");
    }
}

#[test]
fn right_assoc_power() {
    // `2 ^ 3 ^ 4` should parse as `2 ^ (3 ^ 4)`.
    let result = parse_lua51("local x = 2 ^ 3 ^ 4");
    assert_no_errors(&result);
    let expr = local_value(&result);
    if let Expression::BinaryOp(outer) = expr {
        assert!(matches!(outer.op, luck_token::BinOp::Pow));
        assert!(matches!(&outer.left, Expression::Number(_)));
        assert!(matches!(&outer.right, Expression::BinaryOp(_)));
    } else {
        panic!("expected BinaryOp");
    }
}

#[test]
fn right_assoc_concat() {
    // `a .. b .. c` should parse as `a .. (b .. c)`.
    let result = parse_lua51("local x = a .. b .. c");
    assert_no_errors(&result);
    let expr = local_value(&result);
    if let Expression::BinaryOp(outer) = expr {
        assert!(matches!(outer.op, luck_token::BinOp::Concat));
        assert!(matches!(&outer.right, Expression::BinaryOp(_)));
    } else {
        panic!("expected BinaryOp");
    }
}

#[test]
fn unary_minus_vs_exponent_precedence() {
    // `-a ^ b` should parse as `-(a ^ b)`, not `(-a) ^ b`.
    let result = parse_lua51("local x = -a ^ b");
    assert_no_errors(&result);
    let expr = local_value(&result);
    if let Expression::UnaryOp(unary) = expr {
        assert!(matches!(unary.op, luck_token::UnOp::Neg), "outer should be unary minus");
        assert!(
            matches!(&unary.operand, Expression::BinaryOp(binary_op) if matches!(binary_op.op, luck_token::BinOp::Pow)),
            "operand should be exponentiation, got {:?}",
            unary.operand
        );
    } else {
        panic!("expected UnaryOp at top level, got {expr:?}");
    }
}

#[test]
fn precedence_comparison_vs_logical() {
    // `a < b and c > d` should parse as `(a < b) and (c > d)`.
    let result = parse_lua51("local x = a < b and c > d");
    assert_no_errors(&result);
    let expr = local_value(&result);
    if let Expression::BinaryOp(outer) = expr {
        assert!(matches!(outer.op, luck_token::BinOp::And));
        assert!(
            matches!(&outer.left, Expression::BinaryOp(left) if matches!(left.op, luck_token::BinOp::Lt))
        );
        assert!(
            matches!(&outer.right, Expression::BinaryOp(right) if matches!(right.op, luck_token::BinOp::Gt))
        );
    } else {
        panic!("expected BinaryOp, got {expr:?}");
    }
}

#[test]
fn precedence_concat_vs_comparison() {
    // `a .. b == c .. d` should parse as `(a .. b) == (c .. d)`.
    let result = parse_lua51("local x = a .. b == c .. d");
    assert_no_errors(&result);
    let expr = local_value(&result);
    if let Expression::BinaryOp(outer) = expr {
        assert!(
            matches!(outer.op, luck_token::BinOp::Eq),
            "outer should be ==, got {:?}",
            outer.op
        );
        assert!(
            matches!(&outer.left, Expression::BinaryOp(left) if matches!(left.op, luck_token::BinOp::Concat))
        );
        assert!(
            matches!(&outer.right, Expression::BinaryOp(right) if matches!(right.op, luck_token::BinOp::Concat))
        );
    } else {
        panic!("expected BinaryOp, got {expr:?}");
    }
}

#[test]
fn unary_not() {
    let result = parse_lua51("local x = not true");
    assert_no_errors(&result);
    let expr = local_value(&result);
    assert!(matches!(expr, Expression::UnaryOp(_)));
}

#[test]
fn unary_minus() {
    let result = parse_lua51("local x = -42");
    assert_no_errors(&result);
    let expr = local_value(&result);
    assert!(matches!(*expr, Expression::UnaryOp(_)));
}

#[test]
fn unary_length() {
    let result = parse_lua51("local x = #t");
    assert_no_errors(&result);
    let expr = local_value(&result);
    if let Expression::UnaryOp(unary) = expr {
        assert!(matches!(unary.op, luck_token::UnOp::Len));
    } else {
        panic!("expected UnaryOp, got {expr:?}");
    }
}

#[test]
fn table_empty() {
    let result = parse_lua51("local x = {}");
    assert_no_errors(&result);
    let expr = local_value(&result);
    if let Expression::TableConstructor(table) = expr {
        assert!(table.fields.is_empty());
    } else {
        panic!("expected TableConstructor");
    }
}

#[test]
fn table_leading_separator_is_rejected() {
    // PUC Lua rejects a leading separator: `{;}`, `{,}`, `{, 1}`.
    for source in ["local x = {;}", "local x = {,}", "local x = {, 1}"] {
        let result = parse_lua51(source);
        assert!(!result.errors.is_empty(), "expected error for {source:?}, got none");
        // Recovery still produces a table constructor.
        assert!(matches!(local_value(&result), Expression::TableConstructor(_)));
    }
}

#[test]
fn table_mixed_fields() {
    let result = parse_lua51("local x = {1, 2, x = 3, [4] = 5}");
    assert_no_errors(&result);
    let expr = local_value(&result);
    if let Expression::TableConstructor(table) = expr {
        assert_eq!(table.fields.len(), 4);
    } else {
        panic!("expected TableConstructor");
    }
}

#[test]
fn table_trailing_separator() {
    let result = parse_lua51("local x = {1,}");
    assert_no_errors(&result);
    let expr = local_value(&result);
    if let Expression::TableConstructor(table) = expr {
        assert_eq!(table.fields.len(), 1);
    } else {
        panic!("expected TableConstructor");
    }
}

#[test]
fn chained_field_access() {
    let result = parse_lua51("local x = a.b.c");
    assert_no_errors(&result);
    let expr = local_value(&result);
    if let Expression::Var(var) = expr {
        assert!(matches!(var, Var::FieldAccess(_)));
    } else {
        panic!("expected Var, got {expr:?}");
    }
}

#[test]
fn chained_index() {
    let result = parse_lua51("local x = a[1][2]");
    assert_no_errors(&result);
    let expr = local_value(&result);
    if let Expression::Var(var) = expr {
        assert!(matches!(var, Var::Index(_)));
    } else {
        panic!("expected Var, got {expr:?}");
    }
}

#[test]
fn chained_method_calls() {
    let result = parse_lua51("a:b(1):c(2)");
    assert_no_errors(&result);
    if let Statement::FunctionCall(call) = &result.block.stmts[0] {
        assert!(call.method.is_some());
    } else {
        panic!("expected FunctionCall");
    }
}

#[test]
fn chained_function_calls() {
    let result = parse_lua51("f(1)(2)");
    assert_no_errors(&result);
    if let Statement::FunctionCall(call) = &result.block.stmts[0] {
        assert!(matches!(&call.callee, Expression::FunctionCall(_)));
    } else {
        panic!("expected FunctionCall");
    }
}

#[test]
fn parenthesized() {
    let result = parse_lua51("local x = (1 + 2) * 3");
    assert_no_errors(&result);
    let expr = local_value(&result);
    if let Expression::BinaryOp(binary_op) = expr {
        assert!(matches!(binary_op.op, luck_token::BinOp::Mul));
        assert!(matches!(&binary_op.left, Expression::Parenthesized(_)));
    } else {
        panic!("expected BinaryOp, got {expr:?}");
    }
}

#[test]
fn varargs_function() {
    let result = parse_lua51("function f(...) return ... end");
    assert_no_errors(&result);
    if let Statement::FunctionDecl(function_decl) = &result.block.stmts[0] {
        assert!(function_decl.body.vararg.is_some());
    } else {
        panic!("expected FunctionDecl");
    }
}

#[test]
fn varargs_with_named_params() {
    let result = parse_lua51("function f(a, b, ...) end");
    assert_no_errors(&result);
    if let Statement::FunctionDecl(function_decl) = &result.block.stmts[0] {
        assert!(function_decl.body.vararg.is_some());
        let param_count = function_decl.body.params.len();
        assert_eq!(param_count, 2);
    } else {
        panic!("expected FunctionDecl");
    }
}

#[test]
fn return_no_values() {
    let result = parse_lua51("return");
    assert_no_errors(&result);
    assert!(matches!(
        result.block.last_stmt.as_deref().expect("block has last statement"),
        LastStatement::Return(_)
    ));
}

#[test]
fn return_multiple_values() {
    let result = parse_lua51("return 1, 2, 3");
    assert_no_errors(&result);
    if let LastStatement::Return(return_stmt) =
        result.block.last_stmt.as_deref().expect("block has last statement")
    {
        let count = return_stmt.exprs.len();
        assert_eq!(count, 3);
    } else {
        panic!("expected Return");
    }
}

#[test]
fn break_as_last_stmt() {
    let result = parse_lua51("while true do break end");
    assert_no_errors(&result);
    if let Statement::WhileLoop(while_loop) = &result.block.stmts[0] {
        assert!(matches!(
            while_loop.block.last_stmt.as_deref().expect("block has last statement"),
            LastStatement::Break(_)
        ));
    } else {
        panic!("expected WhileLoop");
    }
}

#[test]
fn empty_program() {
    let result = parse_lua51("");
    assert_no_errors(&result);
    assert_eq!(result.block.stmts, Vec::<Statement>::new());
    assert!(result.block.last_stmt.is_none());
}

#[test]
fn semicolon_separates_stmts_without_an_empty_stmt() {
    let result = parse_lua51("local x = 1; local y = 2");
    assert_no_errors(&result);
    assert_eq!(result.block.stmts.len(), 2);
}

#[test]
fn complex_expr_in_function() {
    let result = parse_lua51("local x = function() return 1 + 2 * 3 end");
    assert_no_errors(&result);
    let expr = local_value(&result);
    if let Expression::FunctionDef(function_def) = expr {
        assert!(function_def.body.block.last_stmt.is_some());
    } else {
        panic!("expected FunctionDef");
    }
}

#[test]
fn logical_operators() {
    let result = parse_lua51("local x = a and b or c");
    assert_no_errors(&result);
    // `and` binds tighter than `or`, so this is `(a and b) or c`.
    let expr = local_value(&result);
    if let Expression::BinaryOp(outer) = expr {
        assert!(matches!(outer.op, luck_token::BinOp::Or));
        assert!(
            matches!(&outer.left, Expression::BinaryOp(inner) if matches!(inner.op, luck_token::BinOp::And))
        );
    } else {
        panic!("expected BinaryOp");
    }
}

#[test]
fn comparison_operators() {
    for (source, expected_op) in [
        ("local x = a < b", luck_token::BinOp::Lt),
        ("local x = a >= b", luck_token::BinOp::Ge),
        ("local x = a ~= b", luck_token::BinOp::Ne),
        ("local x = a == b", luck_token::BinOp::Eq),
    ] {
        let result = parse_lua51(source);
        assert_no_errors(&result);
        if let Expression::BinaryOp(binary_op) = local_value(&result) {
            assert_eq!(binary_op.op, expected_op, "failed for: {source}");
        } else {
            panic!("expected BinaryOp for: {source}");
        }
    }
}

#[test]
fn field_access_then_call() {
    let result = parse_lua51("a.b.c()");
    assert_no_errors(&result);
    assert!(matches!(&result.block.stmts[0], Statement::FunctionCall(_)));
}

#[test]
fn multiline_program() {
    let source = "
local x = 10
local y = 20
function add(a, b)
    return a + b
end
local result = add(x, y)
print(result)
";
    let result = parse_lua51(source);
    assert_no_errors(&result);
    assert_eq!(result.block.stmts.len(), 5);
}

#[test]
fn multiline_string_level3_brackets() {
    let result = parse_lua51("local x = [===[hello ]]] ]==] world]===]");
    assert_no_errors(&result);
    let expr = local_value(&result);
    assert!(matches!(*expr, Expression::StringLiteral(_)));
}

#[test]
fn deeply_nested_table() {
    let result = parse_lua51("local x = {{{{}}}}");
    assert_no_errors(&result);
    let expr = local_value(&result);
    if let Expression::TableConstructor(outer) = expr {
        assert_eq!(outer.fields.len(), 1);
        if let Field::Positional { value: Expression::TableConstructor(middle), .. } =
            &outer.fields.items[0]
        {
            assert_eq!(middle.fields.len(), 1);
            if let Field::Positional { value: Expression::TableConstructor(inner), .. } =
                &middle.fields.items[0]
            {
                assert_eq!(inner.fields.len(), 1);
                assert!(matches!(
                    &inner.fields.items[0],
                    Field::Positional { value: Expression::TableConstructor(innermost), .. } if innermost.fields.is_empty()
                ));
            } else {
                panic!("expected nested TableConstructor at depth 3");
            }
        } else {
            panic!("expected nested TableConstructor at depth 2");
        }
    } else {
        panic!("expected TableConstructor");
    }
}

#[test]
fn consecutive_method_calls_four_deep() {
    let result = parse_lua51("a:b():c():d()");
    assert_no_errors(&result);
    if let Statement::FunctionCall(call) = &result.block.stmts[0] {
        assert!(call.method.is_some());
        assert!(matches!(&call.callee, Expression::FunctionCall(_)));
    } else {
        panic!("expected FunctionCall");
    }
}

#[test]
fn call_on_parenthesized_expr() {
    let result = parse_lua51("(f)(x)");
    assert_no_errors(&result);
    if let Statement::FunctionCall(call) = &result.block.stmts[0] {
        assert!(
            matches!(&call.callee, Expression::Parenthesized(_)),
            "expected Parenthesized callee, got {:?}",
            call.callee
        );
    } else {
        panic!("expected FunctionCall");
    }
}
