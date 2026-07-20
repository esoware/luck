use luck_ast::expr::*;
use luck_ast::shared::Field;
use luck_ast::{Expression, LastStatement, Statement};
use luck_token::LuaVersion;

use crate::common::assert_no_errors;
use luck_parser::ParseResult;

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
    if let Statement::WhileLoop(w) = &result.block.stmts[0] {
        assert!(matches!(&w.condition, Expression::True(_)));
    } else {
        panic!("expected WhileLoop");
    }
}

#[test]
fn repeat_until() {
    let result = parse_lua51("repeat until false");
    assert_no_errors(&result);
    assert_eq!(result.block.stmts.len(), 1);
    if let Statement::RepeatLoop(r) = &result.block.stmts[0] {
        assert!(matches!(&r.condition, Expression::False(_)));
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
    if let Statement::IfStatement(stmt) = &result.block.stmts[0] {
        assert_eq!(stmt.elseif_clauses.len(), 1);
        assert!(stmt.else_clause.is_some());
    } else {
        panic!("expected IfStatement");
    }
}

#[test]
fn numeric_for_two_values() {
    let result = parse_lua51("for i = 1, 10 do end");
    assert_no_errors(&result);
    if let Statement::NumericFor(f) = &result.block.stmts[0] {
        assert!(f.step.is_none());
    } else {
        panic!("expected NumericFor");
    }
}

#[test]
fn numeric_for_three_values() {
    let result = parse_lua51("for i = 1, 10, 2 do end");
    assert_no_errors(&result);
    if let Statement::NumericFor(f) = &result.block.stmts[0] {
        assert!(f.step.is_some());
    } else {
        panic!("expected NumericFor");
    }
}

#[test]
fn generic_for() {
    let result = parse_lua51("for k, v in pairs(t) do end");
    assert_no_errors(&result);
    if let Statement::GenericFor(f) = &result.block.stmts[0] {
        assert_eq!(f.names.len(), 2);
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
    if let Statement::FunctionDecl(f) = &result.block.stmts[0] {
        assert_eq!(f.name.names.len(), 2); // a, b
        assert!(f.name.method.is_some()); // :c
    } else {
        panic!("expected FunctionDecl");
    }
}

#[test]
fn local_function() {
    let result = parse_lua51("local function foo(x, y) return x end");
    assert_no_errors(&result);
    if let Statement::LocalFunction(f) = &result.block.stmts[0] {
        let param_count = f.body.params.len();
        assert_eq!(param_count, 2);
    } else {
        panic!("expected LocalFunction");
    }
}

#[test]
fn local_assignment_no_value() {
    let result = parse_lua51("local x");
    assert_no_errors(&result);
    if let Statement::LocalAssignment(la) = &result.block.stmts[0] {
        assert!(la.exprs.is_none());
    } else {
        panic!("expected LocalAssignment");
    }
}

#[test]
fn local_assignment_with_value() {
    let result = parse_lua51("local x = 42");
    assert_no_errors(&result);
    if let Statement::LocalAssignment(la) = &result.block.stmts[0] {
        assert!(la.exprs.is_some());
    } else {
        panic!("expected LocalAssignment");
    }
}

#[test]
fn local_multiple() {
    let result = parse_lua51("local a, b, c = 1, 2, 3");
    assert_no_errors(&result);
    if let Statement::LocalAssignment(la) = &result.block.stmts[0] {
        let name_count = la.names.len();
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
    if let Statement::Assignment(a) = &result.block.stmts[0] {
        let target_count = a.targets.len();
        assert_eq!(target_count, 3);
    } else {
        panic!("expected Assignment");
    }
}

#[test]
fn assign_nested_field_access() {
    let result = parse_lua51("a.b.c = 1");
    assert_no_errors(&result);
    if let Statement::Assignment(a) = &result.block.stmts[0] {
        let target = a.targets.last_item().expect("assignment has target");
        if let Var::FieldAccess(outer) = target {
            if let Expression::Var(inner_var) = &outer.prefix {
                assert!(
                    matches!(inner_var, Var::FieldAccess(_)),
                    "expected nested FieldAccess, got {:?}",
                    inner_var
                );
            } else {
                panic!("expected Var prefix, got {:?}", outer.prefix);
            }
        } else {
            panic!("expected FieldAccess target, got {:?}", target);
        }
    } else {
        panic!("expected Assignment");
    }
}

#[test]
fn assign_index_target() {
    let result = parse_lua51("a[i] = 1");
    assert_no_errors(&result);
    if let Statement::Assignment(a) = &result.block.stmts[0] {
        let target = a.targets.last_item().expect("assignment has target");
        assert!(
            matches!(target, Var::Index(_)),
            "expected Index target, got {:?}",
            target
        );
    } else {
        panic!("expected Assignment");
    }
}

#[test]
fn assign_mixed_chain_target() {
    let result = parse_lua51("a.b[1]:c(2).d[3] = 5");
    assert_no_errors(&result);
    if let Statement::Assignment(a) = &result.block.stmts[0] {
        let target = a.targets.last_item().expect("assignment has target");
        assert!(
            matches!(target, Var::Index(_)),
            "expected Index target for [3], got {:?}",
            target
        );
    } else {
        panic!("expected Assignment");
    }
}

#[test]
fn function_call_statement() {
    let result = parse_lua51("print(\"hello\")");
    assert_no_errors(&result);
    assert!(matches!(&result.block.stmts[0], Statement::FunctionCall(_)));
}

#[test]
fn method_call_statement() {
    let result = parse_lua51("obj:method(1, 2)");
    assert_no_errors(&result);
    if let Statement::FunctionCall(fc) = &result.block.stmts[0] {
        assert!(fc.call.method.is_some());
    } else {
        panic!("expected FunctionCall");
    }
}

#[test]
fn call_with_string_arg() {
    let result = parse_lua51("require \"foo\"");
    assert_no_errors(&result);
    if let Statement::FunctionCall(fc) = &result.block.stmts[0] {
        assert!(matches!(&fc.call.args, FunctionArgs::StringLiteral(_)));
    } else {
        panic!("expected FunctionCall");
    }
}

#[test]
fn call_with_table_arg() {
    let result = parse_lua51("f{1, 2, 3}");
    assert_no_errors(&result);
    if let Statement::FunctionCall(fc) = &result.block.stmts[0] {
        assert!(matches!(&fc.call.args, FunctionArgs::TableConstructor(_)));
    } else {
        panic!("expected FunctionCall");
    }
}

#[test]
fn literal_nil() {
    let result = parse_lua51("local x = nil");
    assert_no_errors(&result);
    if let Statement::LocalAssignment(la) = &result.block.stmts[0] {
        let exprs = la.exprs.as_ref().expect("assignment has values");
        let expr = exprs.last_item().expect("expression list has last element");
        assert!(matches!(*expr, Expression::Nil(_)));
    } else {
        panic!();
    }
}

#[test]
fn literal_true_false() {
    let result = parse_lua51("local a, b = true, false");
    assert_no_errors(&result);
    if let Statement::LocalAssignment(la) = &result.block.stmts[0] {
        let exprs = la.exprs.as_ref().expect("assignment has values");
        let first = &exprs.items[0];
        assert!(matches!(first, Expression::True(_)));
        let second = exprs.last_item().expect("expression list has last element");
        assert!(matches!(*second, Expression::False(_)));
    } else {
        panic!("expected LocalAssignment");
    }
}

#[test]
fn literal_number() {
    let result = parse_lua51("local x = 3.14");
    assert_no_errors(&result);
    if let Statement::LocalAssignment(la) = &result.block.stmts[0] {
        let exprs = la.exprs.as_ref().expect("assignment has values");
        let expr = exprs.last_item().expect("expression list has last element");
        assert!(matches!(*expr, Expression::Number(_)));
    } else {
        panic!("expected LocalAssignment");
    }
}

#[test]
fn literal_string() {
    let result = parse_lua51("local x = \"hello world\"");
    assert_no_errors(&result);
    if let Statement::LocalAssignment(la) = &result.block.stmts[0] {
        let exprs = la.exprs.as_ref().expect("assignment has values");
        let expr = exprs.last_item().expect("expression list has last element");
        assert!(matches!(*expr, Expression::StringLiteral(_)));
    } else {
        panic!("expected LocalAssignment");
    }
}

#[test]
fn precedence_add_mul() {
    // `1 + 2 * 3` should parse as `1 + (2 * 3)`
    let result = parse_lua51("local x = 1 + 2 * 3");
    assert_no_errors(&result);
    if let Statement::LocalAssignment(la) = &result.block.stmts[0] {
        let exprs = la.exprs.as_ref().expect("assignment has values");
        let expr = exprs.last_item().expect("expression list has last element");
        if let Expression::BinaryOp(outer) = expr {
            assert!(matches!(outer.op, luck_token::BinOp::Add));
            assert!(
                matches!(&outer.right, Expression::BinaryOp(inner) if matches!(inner.op, luck_token::BinOp::Mul))
            );
        } else {
            panic!("expected BinaryOp, got {:?}", expr);
        }
    } else {
        panic!();
    }
}

#[test]
fn right_assoc_power() {
    // `2 ^ 3 ^ 4` should parse as `2 ^ (3 ^ 4)`
    let result = parse_lua51("local x = 2 ^ 3 ^ 4");
    assert_no_errors(&result);
    if let Statement::LocalAssignment(la) = &result.block.stmts[0] {
        let exprs = la.exprs.as_ref().expect("assignment has values");
        let expr = exprs.last_item().expect("expression list has last element");
        if let Expression::BinaryOp(outer) = expr {
            assert!(matches!(outer.op, luck_token::BinOp::Pow));
            assert!(matches!(&outer.left, Expression::Number(_)));
            assert!(matches!(&outer.right, Expression::BinaryOp(_)));
        } else {
            panic!("expected BinaryOp");
        }
    } else {
        panic!();
    }
}

#[test]
fn right_assoc_concat() {
    // `a .. b .. c` should parse as `a .. (b .. c)`
    let result = parse_lua51("local x = a .. b .. c");
    assert_no_errors(&result);
    if let Statement::LocalAssignment(la) = &result.block.stmts[0] {
        let exprs = la.exprs.as_ref().expect("assignment has values");
        let expr = exprs.last_item().expect("expression list has last element");
        if let Expression::BinaryOp(outer) = expr {
            assert!(matches!(outer.op, luck_token::BinOp::Concat));
            assert!(matches!(&outer.right, Expression::BinaryOp(_)));
        } else {
            panic!("expected BinaryOp");
        }
    } else {
        panic!();
    }
}

#[test]
fn unary_minus_vs_exponent_precedence() {
    // `-a ^ b` should parse as `-(a ^ b)`, not `(-a) ^ b`
    let result = parse_lua51("local x = -a ^ b");
    assert_no_errors(&result);
    if let Statement::LocalAssignment(la) = &result.block.stmts[0] {
        let exprs = la.exprs.as_ref().expect("assignment has values");
        let expr = exprs.last_item().expect("expression list has last element");
        if let Expression::UnaryOp(unary) = expr {
            assert!(
                matches!(unary.op, luck_token::UnOp::Neg),
                "outer should be unary minus"
            );
            assert!(
                matches!(&unary.operand, Expression::BinaryOp(binop) if matches!(binop.op, luck_token::BinOp::Pow)),
                "operand should be exponentiation, got {:?}",
                unary.operand
            );
        } else {
            panic!("expected UnaryOp at top level, got {:?}", expr);
        }
    } else {
        panic!();
    }
}

#[test]
fn precedence_comparison_vs_logical() {
    // `a < b and c > d` should parse as `(a < b) and (c > d)`
    let result = parse_lua51("local x = a < b and c > d");
    assert_no_errors(&result);
    if let Statement::LocalAssignment(la) = &result.block.stmts[0] {
        let exprs = la.exprs.as_ref().expect("assignment has values");
        let expr = exprs.last_item().expect("expression list has last element");
        if let Expression::BinaryOp(outer) = expr {
            assert!(matches!(outer.op, luck_token::BinOp::And));
            assert!(
                matches!(&outer.left, Expression::BinaryOp(l) if matches!(l.op, luck_token::BinOp::Lt))
            );
            assert!(
                matches!(&outer.right, Expression::BinaryOp(r) if matches!(r.op, luck_token::BinOp::Gt))
            );
        } else {
            panic!("expected BinaryOp, got {:?}", expr);
        }
    } else {
        panic!();
    }
}

#[test]
fn precedence_concat_vs_comparison() {
    // `a .. b == c .. d` should parse as `(a .. b) == (c .. d)`
    let result = parse_lua51("local x = a .. b == c .. d");
    assert_no_errors(&result);
    if let Statement::LocalAssignment(la) = &result.block.stmts[0] {
        let exprs = la.exprs.as_ref().expect("assignment has values");
        let expr = exprs.last_item().expect("expression list has last element");
        if let Expression::BinaryOp(outer) = expr {
            assert!(
                matches!(outer.op, luck_token::BinOp::Eq),
                "outer should be ==, got {:?}",
                outer.op
            );
            assert!(
                matches!(&outer.left, Expression::BinaryOp(l) if matches!(l.op, luck_token::BinOp::Concat))
            );
            assert!(
                matches!(&outer.right, Expression::BinaryOp(r) if matches!(r.op, luck_token::BinOp::Concat))
            );
        } else {
            panic!("expected BinaryOp, got {:?}", expr);
        }
    } else {
        panic!();
    }
}

#[test]
fn unary_not() {
    let result = parse_lua51("local x = not true");
    assert_no_errors(&result);
    if let Statement::LocalAssignment(la) = &result.block.stmts[0] {
        let exprs = la.exprs.as_ref().expect("assignment has values");
        let expr = exprs.last_item().expect("expression list has last element");
        assert!(matches!(expr, Expression::UnaryOp(_)));
    } else {
        panic!();
    }
}

#[test]
fn unary_minus() {
    let result = parse_lua51("local x = -42");
    assert_no_errors(&result);
    if let Statement::LocalAssignment(la) = &result.block.stmts[0] {
        let exprs = la.exprs.as_ref().expect("assignment has values");
        let expr = exprs.last_item().expect("expression list has last element");
        assert!(matches!(*expr, Expression::UnaryOp(_)));
    } else {
        panic!("expected LocalAssignment");
    }
}

#[test]
fn unary_length() {
    let result = parse_lua51("local x = #t");
    assert_no_errors(&result);
    if let Statement::LocalAssignment(la) = &result.block.stmts[0] {
        let exprs = la.exprs.as_ref().expect("assignment has values");
        let expr = exprs.last_item().expect("expression list has last element");
        if let Expression::UnaryOp(unary) = expr {
            assert!(matches!(unary.op, luck_token::UnOp::Len));
        } else {
            panic!("expected UnaryOp, got {:?}", expr);
        }
    } else {
        panic!("expected LocalAssignment");
    }
}

#[test]
fn table_empty() {
    let result = parse_lua51("local x = {}");
    assert_no_errors(&result);
    if let Statement::LocalAssignment(la) = &result.block.stmts[0] {
        let exprs = la.exprs.as_ref().expect("assignment has values");
        let expr = exprs.last_item().expect("expression list has last element");
        if let Expression::TableConstructor(t) = expr {
            assert!(t.fields.is_empty());
        } else {
            panic!("expected TableConstructor");
        }
    } else {
        panic!("expected LocalAssignment");
    }
}

#[test]
fn table_leading_separator_is_rejected() {
    // PUC Lua rejects a leading separator: `{;}`, `{,}`, `{, 1}`.
    for source in ["local x = {;}", "local x = {,}", "local x = {, 1}"] {
        let result = parse_lua51(source);
        assert!(
            !result.errors.is_empty(),
            "expected error for {source:?}, got none"
        );
        // Recovery still produces a table constructor.
        if let Statement::LocalAssignment(la) = &result.block.stmts[0] {
            let exprs = la.exprs.as_ref().expect("assignment has values");
            let expr = exprs.last_item().expect("expression list has last element");
            assert!(matches!(*expr, Expression::TableConstructor(_)));
        } else {
            panic!("expected LocalAssignment");
        }
    }
}

#[test]
fn table_mixed_fields() {
    let result = parse_lua51("local x = {1, 2, x = 3, [4] = 5}");
    assert_no_errors(&result);
    if let Statement::LocalAssignment(la) = &result.block.stmts[0] {
        let exprs = la.exprs.as_ref().expect("assignment has values");
        let expr = exprs.last_item().expect("expression list has last element");
        if let Expression::TableConstructor(t) = expr {
            assert_eq!(t.fields.len(), 4);
        } else {
            panic!("expected TableConstructor");
        }
    } else {
        panic!();
    }
}

#[test]
fn table_trailing_separator() {
    let result = parse_lua51("local x = {1,}");
    assert_no_errors(&result);
    if let Statement::LocalAssignment(la) = &result.block.stmts[0] {
        let exprs = la.exprs.as_ref().expect("assignment has values");
        let expr = exprs.last_item().expect("expression list has last element");
        if let Expression::TableConstructor(t) = expr {
            assert_eq!(t.fields.len(), 1);
        } else {
            panic!("expected TableConstructor");
        }
    } else {
        panic!("expected LocalAssignment");
    }
}

#[test]
fn chained_field_access() {
    let result = parse_lua51("local x = a.b.c");
    assert_no_errors(&result);
    if let Statement::LocalAssignment(la) = &result.block.stmts[0] {
        let exprs = la.exprs.as_ref().expect("assignment has values");
        let expr = exprs.last_item().expect("expression list has last element");
        if let Expression::Var(v) = expr {
            assert!(matches!(v, Var::FieldAccess(_)));
        } else {
            panic!("expected Var, got {:?}", expr);
        }
    } else {
        panic!("expected LocalAssignment");
    }
}

#[test]
fn chained_index() {
    let result = parse_lua51("local x = a[1][2]");
    assert_no_errors(&result);
    if let Statement::LocalAssignment(la) = &result.block.stmts[0] {
        let exprs = la.exprs.as_ref().expect("assignment has values");
        let expr = exprs.last_item().expect("expression list has last element");
        if let Expression::Var(v) = expr {
            assert!(matches!(v, Var::Index(_)));
        } else {
            panic!("expected Var, got {:?}", expr);
        }
    } else {
        panic!("expected LocalAssignment");
    }
}

#[test]
fn chained_method_calls() {
    let result = parse_lua51("a:b(1):c(2)");
    assert_no_errors(&result);
    if let Statement::FunctionCall(fc) = &result.block.stmts[0] {
        assert!(fc.call.method.is_some());
    } else {
        panic!("expected FunctionCall");
    }
}

#[test]
fn chained_function_calls() {
    let result = parse_lua51("f(1)(2)");
    assert_no_errors(&result);
    if let Statement::FunctionCall(fc) = &result.block.stmts[0] {
        assert!(matches!(&fc.call.callee, Expression::FunctionCall(_)));
    } else {
        panic!("expected FunctionCall");
    }
}

#[test]
fn parenthesized() {
    let result = parse_lua51("local x = (1 + 2) * 3");
    assert_no_errors(&result);
    if let Statement::LocalAssignment(la) = &result.block.stmts[0] {
        let exprs = la.exprs.as_ref().expect("assignment has values");
        let expr = exprs.last_item().expect("expression list has last element");
        if let Expression::BinaryOp(binop) = expr {
            assert!(matches!(binop.op, luck_token::BinOp::Mul));
            assert!(matches!(&binop.left, Expression::Parenthesized(_)));
        } else {
            panic!("expected BinaryOp, got {:?}", expr);
        }
    } else {
        panic!("expected LocalAssignment");
    }
}

#[test]
fn varargs_function() {
    let result = parse_lua51("function f(...) return ... end");
    assert_no_errors(&result);
    if let Statement::FunctionDecl(f) = &result.block.stmts[0] {
        assert!(f.body.vararg.is_some());
    } else {
        panic!("expected FunctionDecl");
    }
}

#[test]
fn varargs_with_named_params() {
    let result = parse_lua51("function f(a, b, ...) end");
    assert_no_errors(&result);
    if let Statement::FunctionDecl(f) = &result.block.stmts[0] {
        assert!(f.body.vararg.is_some());
        let param_count = f.body.params.len();
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
        result
            .block
            .last_stmt
            .as_deref()
            .expect("block has last statement"),
        LastStatement::Return(_)
    ));
}

#[test]
fn return_multiple_values() {
    let result = parse_lua51("return 1, 2, 3");
    assert_no_errors(&result);
    if let LastStatement::Return(r) = result
        .block
        .last_stmt
        .as_deref()
        .expect("block has last statement")
    {
        let count = r.exprs.len();
        assert_eq!(count, 3);
    } else {
        panic!("expected Return");
    }
}

#[test]
fn break_as_last_statement() {
    let result = parse_lua51("while true do break end");
    assert_no_errors(&result);
    if let Statement::WhileLoop(w) = &result.block.stmts[0] {
        assert!(matches!(
            w.block
                .last_stmt
                .as_deref()
                .expect("block has last statement"),
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
    assert!(result.block.stmts.is_empty());
    assert!(result.block.last_stmt.is_none());
}

#[test]
fn semicolons_as_separators() {
    let result = parse_lua51("local x = 1; local y = 2");
    assert_no_errors(&result);
    assert_eq!(result.block.stmts.len(), 2);
}

#[test]
fn complex_expression_in_function() {
    let result = parse_lua51("local x = function() return 1 + 2 * 3 end");
    assert_no_errors(&result);
    if let Statement::LocalAssignment(la) = &result.block.stmts[0] {
        let exprs = la.exprs.as_ref().expect("assignment has values");
        let expr = exprs.last_item().expect("expression list has last element");
        if let Expression::FunctionDef(fd) = expr {
            assert!(fd.body.block.last_stmt.is_some());
        } else {
            panic!("expected FunctionDef");
        }
    } else {
        panic!("expected LocalAssignment");
    }
}

#[test]
fn logical_operators() {
    let result = parse_lua51("local x = a and b or c");
    assert_no_errors(&result);
    // Should parse as `(a and b) or c` since `and` has higher precedence than `or`
    if let Statement::LocalAssignment(la) = &result.block.stmts[0] {
        let exprs = la.exprs.as_ref().expect("assignment has values");
        let expr = exprs.last_item().expect("expression list has last element");
        if let Expression::BinaryOp(outer) = expr {
            assert!(matches!(outer.op, luck_token::BinOp::Or));
            assert!(
                matches!(&outer.left, Expression::BinaryOp(inner) if matches!(inner.op, luck_token::BinOp::And))
            );
        } else {
            panic!("expected BinaryOp");
        }
    } else {
        panic!();
    }
}

#[test]
fn comparison_operators() {
    for (src, expected_kind) in [
        ("local x = a < b", luck_token::BinOp::Lt),
        ("local x = a >= b", luck_token::BinOp::Ge),
        ("local x = a ~= b", luck_token::BinOp::Ne),
        ("local x = a == b", luck_token::BinOp::Eq),
    ] {
        let result = parse_lua51(src);
        assert_no_errors(&result);
        if let Statement::LocalAssignment(la) = &result.block.stmts[0] {
            let exprs = la.exprs.as_ref().expect("assignment has values");
            let expr = exprs.last_item().expect("expression list has last element");
            if let Expression::BinaryOp(binop) = expr {
                assert_eq!(binop.op, expected_kind, "failed for: {}", src);
            } else {
                panic!("expected BinaryOp for: {}", src);
            }
        } else {
            panic!("expected LocalAssignment for: {}", src);
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
    let source = r#"
local x = 10
local y = 20
function add(a, b)
    return a + b
end
local result = add(x, y)
print(result)
"#;
    let result = parse_lua51(source);
    assert_no_errors(&result);
    assert_eq!(result.block.stmts.len(), 5);
}

#[test]
fn multiline_string_level3_brackets() {
    let result = parse_lua51("local x = [===[hello ]]] ]==] world]===]");
    assert_no_errors(&result);
    if let Statement::LocalAssignment(la) = &result.block.stmts[0] {
        let exprs = la.exprs.as_ref().expect("assignment has values");
        let expr = exprs.last_item().expect("expression list has last element");
        assert!(matches!(*expr, Expression::StringLiteral(_)));
    } else {
        panic!("expected LocalAssignment");
    }
}

#[test]
fn deeply_nested_table() {
    let result = parse_lua51("local x = {{{{}}}}");
    assert_no_errors(&result);
    if let Statement::LocalAssignment(la) = &result.block.stmts[0] {
        let exprs = la.exprs.as_ref().expect("assignment has values");
        let expr = exprs.last_item().expect("expression list has last element");
        if let Expression::TableConstructor(outer) = expr {
            assert_eq!(outer.fields.len(), 1);
            if let Field::Positional {
                value: Expression::TableConstructor(mid),
                ..
            } = &outer.fields.items[0]
            {
                assert_eq!(mid.fields.len(), 1);
                if let Field::Positional {
                    value: Expression::TableConstructor(inner),
                    ..
                } = &mid.fields.items[0]
                {
                    assert_eq!(inner.fields.len(), 1);
                    assert!(matches!(
                        &inner.fields.items[0],
                        Field::Positional { value: Expression::TableConstructor(t), .. } if t.fields.is_empty()
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
    } else {
        panic!("expected LocalAssignment");
    }
}

#[test]
fn consecutive_method_calls_four_deep() {
    let result = parse_lua51("a:b():c():d()");
    assert_no_errors(&result);
    if let Statement::FunctionCall(fc) = &result.block.stmts[0] {
        assert!(fc.call.method.is_some());
        assert!(matches!(&fc.call.callee, Expression::FunctionCall(_)));
    } else {
        panic!("expected FunctionCall");
    }
}

#[test]
fn call_on_parenthesized_expression() {
    let result = parse_lua51("(f)(x)");
    assert_no_errors(&result);
    if let Statement::FunctionCall(fc) = &result.block.stmts[0] {
        assert!(
            matches!(&fc.call.callee, Expression::Parenthesized(_)),
            "expected Parenthesized callee, got {:?}",
            fc.call.callee
        );
    } else {
        panic!("expected FunctionCall");
    }
}
