use luck_allocator::Allocator;
use luck_ast::{Expression, Field, FunctionArgs, GetSpan as _, Statement, Suffix};
use luck_parser::ParseResult;
use luck_token::{BinOp, LuaVersion, Span, UnOp};

use crate::common::{
    assert_no_errors, chain, chain_shape, local_value, run_operands, run_ops, target_chain,
};

fn parse_lua51<'ast>(allocator: &'ast Allocator, source: &'ast str) -> ParseResult<'ast> {
    luck_parser::parse(allocator, source.as_bytes(), LuaVersion::Lua51)
}

#[test]
fn do_end() {
    let allocator = Allocator::new();
    let result = parse_lua51(&allocator, "do end");
    assert_no_errors(&result);
    assert_eq!(result.chunk.block.stmts.len(), 1);
    assert!(matches!(&result.chunk.block.stmts[0], Statement::DoBlock(_)));
}

#[test]
fn nested_do_blocks() {
    let allocator = Allocator::new();
    let result = parse_lua51(&allocator, "do do do end end end");
    assert_no_errors(&result);
    assert_eq!(result.chunk.block.stmts.len(), 1);
    if let Statement::DoBlock(outer) = &result.chunk.block.stmts[0] {
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
    let allocator = Allocator::new();
    let result = parse_lua51(&allocator, "while true do end");
    assert_no_errors(&result);
    assert_eq!(result.chunk.block.stmts.len(), 1);
    if let Statement::WhileLoop(while_loop) = &result.chunk.block.stmts[0] {
        assert!(matches!(&while_loop.condition, Expression::True(_)));
    } else {
        panic!("expected WhileLoop");
    }
}

#[test]
fn repeat_until() {
    let allocator = Allocator::new();
    let result = parse_lua51(&allocator, "repeat until false");
    assert_no_errors(&result);
    assert_eq!(result.chunk.block.stmts.len(), 1);
    if let Statement::RepeatLoop(repeat_loop) = &result.chunk.block.stmts[0] {
        assert!(matches!(&repeat_loop.condition, Expression::False(_)));
    } else {
        panic!("expected RepeatLoop");
    }
}

#[test]
fn if_then_end() {
    let allocator = Allocator::new();
    let result = parse_lua51(&allocator, "if true then end");
    assert_no_errors(&result);
    assert_eq!(result.chunk.block.stmts.len(), 1);
    assert!(matches!(&result.chunk.block.stmts[0], Statement::IfStatement(_)));
}

#[test]
fn if_elseif_else() {
    let allocator = Allocator::new();
    let result = parse_lua51(&allocator, "if x then a() elseif y then b() else c() end");
    assert_no_errors(&result);
    if let Statement::IfStatement(if_stmt) = &result.chunk.block.stmts[0] {
        assert_eq!(if_stmt.elseif_clauses.len(), 1);
        assert!(if_stmt.else_clause.is_some());
    } else {
        panic!("expected IfStatement");
    }
}

#[test]
fn numeric_for_two_values() {
    let allocator = Allocator::new();
    let result = parse_lua51(&allocator, "for i = 1, 10 do end");
    assert_no_errors(&result);
    if let Statement::NumericFor(numeric_for) = &result.chunk.block.stmts[0] {
        assert!(numeric_for.step.is_none());
    } else {
        panic!("expected NumericFor");
    }
}

#[test]
fn numeric_for_three_values() {
    let allocator = Allocator::new();
    let result = parse_lua51(&allocator, "for i = 1, 10, 2 do end");
    assert_no_errors(&result);
    if let Statement::NumericFor(numeric_for) = &result.chunk.block.stmts[0] {
        assert!(numeric_for.step.is_some());
    } else {
        panic!("expected NumericFor");
    }
}

#[test]
fn generic_for() {
    let allocator = Allocator::new();
    let result = parse_lua51(&allocator, "for k, v in pairs(t) do end");
    assert_no_errors(&result);
    if let Statement::GenericFor(generic_for) = &result.chunk.block.stmts[0] {
        assert_eq!(generic_for.names.len(), 2);
    } else {
        panic!("expected GenericFor");
    }
}

#[test]
fn function_decl_simple() {
    let allocator = Allocator::new();
    let result = parse_lua51(&allocator, "function foo() end");
    assert_no_errors(&result);
    assert!(matches!(&result.chunk.block.stmts[0], Statement::FunctionDecl(_)));
}

#[test]
fn method_declaration() {
    let allocator = Allocator::new();
    let result = parse_lua51(&allocator, "function a.b:c() end");
    assert_no_errors(&result);
    if let Statement::FunctionDecl(function_decl) = &result.chunk.block.stmts[0] {
        assert_eq!(function_decl.name.names.len(), 2); // a, b
        assert!(function_decl.name.method.is_some()); // :c
    } else {
        panic!("expected FunctionDecl");
    }
}

#[test]
fn local_function() {
    let allocator = Allocator::new();
    let result = parse_lua51(&allocator, "local function foo(x, y) return x end");
    assert_no_errors(&result);
    if let Statement::LocalFunction(local_function) = &result.chunk.block.stmts[0] {
        let param_count = local_function.body.params.len();
        assert_eq!(param_count, 2);
    } else {
        panic!("expected LocalFunction");
    }
}

#[test]
fn local_assignment_no_value() {
    let allocator = Allocator::new();
    let result = parse_lua51(&allocator, "local x");
    assert_no_errors(&result);
    if let Statement::LocalAssignment(local) = &result.chunk.block.stmts[0] {
        assert!(local.exprs.is_empty());
    } else {
        panic!("expected LocalAssignment");
    }
}

#[test]
fn local_assignment_with_value() {
    let allocator = Allocator::new();
    let result = parse_lua51(&allocator, "local x = 42");
    assert_no_errors(&result);
    if let Statement::LocalAssignment(local) = &result.chunk.block.stmts[0] {
        assert!(!local.exprs.is_empty());
    } else {
        panic!("expected LocalAssignment");
    }
}

#[test]
fn local_multiple() {
    let allocator = Allocator::new();
    let result = parse_lua51(&allocator, "local a, b, c = 1, 2, 3");
    assert_no_errors(&result);
    if let Statement::LocalAssignment(local) = &result.chunk.block.stmts[0] {
        let name_count = local.names.len();
        assert_eq!(name_count, 3);
    } else {
        panic!("expected LocalAssignment");
    }
}

#[test]
fn simple_assignment() {
    let allocator = Allocator::new();
    let result = parse_lua51(&allocator, "x = 1");
    assert_no_errors(&result);
    assert!(matches!(&result.chunk.block.stmts[0], Statement::Assignment(_)));
}

#[test]
fn multi_assignment() {
    let allocator = Allocator::new();
    let result = parse_lua51(&allocator, "a, b, c = 1, 2, 3");
    assert_no_errors(&result);
    if let Statement::Assignment(assignment) = &result.chunk.block.stmts[0] {
        let target_count = assignment.targets.len();
        assert_eq!(target_count, 3);
    } else {
        panic!("expected Assignment");
    }
}

#[test]
fn assign_nested_field_access() {
    let allocator = Allocator::new();
    let result = parse_lua51(&allocator, "a.b.c = 1");
    assert_no_errors(&result);
    let Statement::Assignment(assignment) = &result.chunk.block.stmts[0] else {
        panic!("expected Assignment");
    };
    let target = assignment.targets.last().expect("assignment has target");
    assert_eq!(chain_shape(target_chain(target)), "a.b.c");
}

#[test]
fn assign_index_target() {
    let allocator = Allocator::new();
    let result = parse_lua51(&allocator, "a[i] = 1");
    assert_no_errors(&result);
    let Statement::Assignment(assignment) = &result.chunk.block.stmts[0] else {
        panic!("expected Assignment");
    };
    let target = assignment.targets.last().expect("assignment has target");
    assert_eq!(chain_shape(target_chain(target)), "a[]");
}

#[test]
fn assign_mixed_chain_target() {
    let allocator = Allocator::new();
    let result = parse_lua51(&allocator, "a.b[1]:c(2).d[3] = 5");
    assert_no_errors(&result);
    let Statement::Assignment(assignment) = &result.chunk.block.stmts[0] else {
        panic!("expected Assignment");
    };
    let target = assignment.targets.last().expect("assignment has target");
    assert_eq!(chain_shape(target_chain(target)), "a.b[]:c().d[]");
}

#[test]
fn function_call_stmt() {
    let allocator = Allocator::new();
    let result = parse_lua51(&allocator, "print(\"hello\")");
    assert_no_errors(&result);
    assert!(matches!(&result.chunk.block.stmts[0], Statement::FunctionCall(_)));
}

#[test]
fn method_call_stmt() {
    let allocator = Allocator::new();
    let result = parse_lua51(&allocator, "obj:method(1, 2)");
    assert_no_errors(&result);
    let Statement::FunctionCall(call) = &result.chunk.block.stmts[0] else {
        panic!("expected FunctionCall");
    };
    assert_eq!(chain_shape(call), "obj:method()");
}

#[test]
fn call_with_string_arg() {
    let allocator = Allocator::new();
    let result = parse_lua51(&allocator, "require \"foo\"");
    assert_no_errors(&result);
    let Statement::FunctionCall(call) = &result.chunk.block.stmts[0] else {
        panic!("expected FunctionCall");
    };
    assert!(
        matches!(call.suffixes.last(), Suffix::Call(args) if matches!(args, FunctionArgs::StringLiteral(_)))
    );
}

#[test]
fn call_with_table_arg() {
    let allocator = Allocator::new();
    let result = parse_lua51(&allocator, "f{1, 2, 3}");
    assert_no_errors(&result);
    let Statement::FunctionCall(call) = &result.chunk.block.stmts[0] else {
        panic!("expected FunctionCall");
    };
    assert!(
        matches!(call.suffixes.last(), Suffix::Call(args) if matches!(args, FunctionArgs::TableConstructor(_)))
    );
}

#[test]
fn literal_nil() {
    let allocator = Allocator::new();
    let result = parse_lua51(&allocator, "local x = nil");
    assert_no_errors(&result);
    let expr = local_value(&result);
    assert!(matches!(*expr, Expression::Nil(_)));
}

#[test]
fn literal_true_false() {
    let allocator = Allocator::new();
    let result = parse_lua51(&allocator, "local a, b = true, false");
    assert_no_errors(&result);
    if let Statement::LocalAssignment(local) = &result.chunk.block.stmts[0] {
        let exprs = &local.exprs;
        let first = &exprs[0];
        assert!(matches!(first, Expression::True(_)));
        let second = exprs.last().expect("expression list has last element");
        assert!(matches!(*second, Expression::False(_)));
    } else {
        panic!("expected LocalAssignment");
    }
}

#[test]
fn literal_number() {
    let allocator = Allocator::new();
    let result = parse_lua51(&allocator, "local x = 3.14");
    assert_no_errors(&result);
    let expr = local_value(&result);
    assert!(matches!(*expr, Expression::Number(_)));
}

#[test]
fn literal_string() {
    let allocator = Allocator::new();
    let result = parse_lua51(&allocator, "local x = \"hello world\"");
    assert_no_errors(&result);
    let expr = local_value(&result);
    assert!(matches!(*expr, Expression::StringLiteral(_)));
}

#[test]
fn precedence_add_mul() {
    let allocator = Allocator::new();
    // `1 + 2 * 3` should parse as `1 + (2 * 3)`.
    let result = parse_lua51(&allocator, "local x = 1 + 2 * 3");
    assert_no_errors(&result);
    let expr = local_value(&result);
    assert_eq!(run_ops(expr), [BinOp::Add]);
    assert_eq!(run_ops(run_operands(expr)[1]), [BinOp::Mul]);
}

#[test]
fn right_assoc_power() {
    let allocator = Allocator::new();
    // `2 ^ 3 ^ 4` is one run, which groups from the right: `2 ^ (3 ^ 4)`.
    let result = parse_lua51(&allocator, "local x = 2 ^ 3 ^ 4");
    assert_no_errors(&result);
    let expr = local_value(&result);
    assert_eq!(run_ops(expr), [BinOp::Pow, BinOp::Pow]);
    assert!(run_operands(expr).iter().all(|operand| matches!(operand, Expression::Number(_))));
}

#[test]
fn right_assoc_concat() {
    let allocator = Allocator::new();
    // `a .. b .. c` is one run, which groups from the right: `a .. (b .. c)`.
    let result = parse_lua51(&allocator, "local x = a .. b .. c");
    assert_no_errors(&result);
    assert_eq!(run_ops(local_value(&result)), [BinOp::Concat, BinOp::Concat]);
}

#[test]
fn unary_minus_vs_exponent_precedence() {
    let allocator = Allocator::new();
    // `-a ^ b` should parse as `-(a ^ b)`, not `(-a) ^ b`.
    let result = parse_lua51(&allocator, "local x = -a ^ b");
    assert_no_errors(&result);
    let expr = local_value(&result);
    let Expression::UnaryOp(unary) = expr else {
        panic!("expected UnaryOp at top level, got {expr:?}");
    };
    assert_eq!(unary.op, UnOp::Neg, "outer should be unary minus");
    assert_eq!(run_ops(&unary.operand), [BinOp::Pow]);
}

#[test]
fn precedence_comparison_vs_logical() {
    let allocator = Allocator::new();
    // `a < b and c > d` should parse as `(a < b) and (c > d)`.
    let result = parse_lua51(&allocator, "local x = a < b and c > d");
    assert_no_errors(&result);
    let expr = local_value(&result);
    assert_eq!(run_ops(expr), [BinOp::And]);
    let operands = run_operands(expr);
    assert_eq!(run_ops(operands[0]), [BinOp::Lt]);
    assert_eq!(run_ops(operands[1]), [BinOp::Gt]);
}

#[test]
fn precedence_concat_vs_comparison() {
    let allocator = Allocator::new();
    // `a .. b == c .. d` should parse as `(a .. b) == (c .. d)`.
    let result = parse_lua51(&allocator, "local x = a .. b == c .. d");
    assert_no_errors(&result);
    let expr = local_value(&result);
    assert_eq!(run_ops(expr), [BinOp::Eq]);
    let operands = run_operands(expr);
    assert_eq!(run_ops(operands[0]), [BinOp::Concat]);
    assert_eq!(run_ops(operands[1]), [BinOp::Concat]);
}

#[test]
fn unary_not() {
    let allocator = Allocator::new();
    let result = parse_lua51(&allocator, "local x = not true");
    assert_no_errors(&result);
    let expr = local_value(&result);
    assert!(matches!(expr, Expression::UnaryOp(_)));
}

#[test]
fn unary_minus() {
    let allocator = Allocator::new();
    let result = parse_lua51(&allocator, "local x = -42");
    assert_no_errors(&result);
    let expr = local_value(&result);
    assert!(matches!(*expr, Expression::UnaryOp(_)));
}

#[test]
fn unary_length() {
    let allocator = Allocator::new();
    let result = parse_lua51(&allocator, "local x = #t");
    assert_no_errors(&result);
    let expr = local_value(&result);
    if let Expression::UnaryOp(unary) = expr {
        assert!(matches!(unary.op, UnOp::Len));
    } else {
        panic!("expected UnaryOp, got {expr:?}");
    }
}

#[test]
fn table_empty() {
    let allocator = Allocator::new();
    let result = parse_lua51(&allocator, "local x = {}");
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
    let allocator = Allocator::new();
    // PUC Lua rejects a leading separator: `{;}`, `{,}`, `{, 1}`.
    for source in ["local x = {;}", "local x = {,}", "local x = {, 1}"] {
        let result = parse_lua51(&allocator, source);
        assert!(!result.errors.is_empty(), "expected error for {source:?}, got none");
        // Recovery still produces a table constructor.
        assert!(matches!(local_value(&result), Expression::TableConstructor(_)));
    }
}

#[test]
fn table_mixed_fields() {
    let allocator = Allocator::new();
    let result = parse_lua51(&allocator, "local x = {1, 2, x = 3, [4] = 5}");
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
    let allocator = Allocator::new();
    let result = parse_lua51(&allocator, "local x = {1,}");
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
    let allocator = Allocator::new();
    let result = parse_lua51(&allocator, "local x = a.b.c");
    assert_no_errors(&result);
    assert_eq!(chain_shape(chain(local_value(&result))), "a.b.c");
}

#[test]
fn chained_index() {
    let allocator = Allocator::new();
    let result = parse_lua51(&allocator, "local x = a[1][2]");
    assert_no_errors(&result);
    assert_eq!(chain_shape(chain(local_value(&result))), "a[][]");
}

#[test]
fn chained_method_calls() {
    let allocator = Allocator::new();
    let result = parse_lua51(&allocator, "a:b(1):c(2)");
    assert_no_errors(&result);
    let Statement::FunctionCall(call) = &result.chunk.block.stmts[0] else {
        panic!("expected FunctionCall");
    };
    assert_eq!(chain_shape(call), "a:b():c()");
}

#[test]
fn chained_function_calls() {
    let allocator = Allocator::new();
    let result = parse_lua51(&allocator, "f(1)(2)");
    assert_no_errors(&result);
    let Statement::FunctionCall(call) = &result.chunk.block.stmts[0] else {
        panic!("expected FunctionCall");
    };
    assert_eq!(chain_shape(call), "f()()");
}

#[test]
fn parenthesized() {
    let allocator = Allocator::new();
    let result = parse_lua51(&allocator, "local x = (1 + 2) * 3");
    assert_no_errors(&result);
    let expr = local_value(&result);
    assert_eq!(run_ops(expr), [BinOp::Mul]);
    let sum = run_operands(expr)[0];
    assert_eq!(run_ops(sum), [BinOp::Add], "parentheses group without a node of their own");
    assert_eq!(result.groupings.around(sum.span()), Some(Span::new(10, 17)));
    assert_eq!(expr.span(), Span::new(10, 21), "a node spans the parentheses it starts with");
}

#[test]
fn parentheses_change_only_what_a_call_or_vararg_yields() {
    let allocator = Allocator::new();
    let result = parse_lua51(&allocator, "local function f(...) return (f()), (...), ((x)) end");
    assert_no_errors(&result);
    let Some(Statement::LocalFunction(function)) = result.chunk.block.stmts.first() else {
        panic!("expected a local function, got {:?}", result.chunk.block.stmts);
    };
    let Some(Statement::Return(return_stmt)) = function.body.block.stmts.last() else {
        panic!("expected a return");
    };
    assert!(matches!(
        *return_stmt.exprs,
        [Expression::Parenthesized(_), Expression::Parenthesized(_), Expression::Name(_)]
    ));
}

#[test]
fn a_parenthesized_variable_is_no_assignment_target() {
    let allocator = Allocator::new();
    for source in ["(a) = 1", "a, (b) = 1, 2", "(a.b) = 1", "((a)) = 1"] {
        let result = parse_lua51(&allocator, source);
        let [error] = result.errors.as_slice() else {
            panic!("{source}: {:?}", result.errors);
        };
        assert_eq!(error.message, "invalid assignment target", "{source}");
    }
    assert_no_errors(&parse_lua51(&allocator, "(a).b = 1 (a)[1] = 2 (f)()"));
}

#[test]
fn varargs_function() {
    let allocator = Allocator::new();
    let result = parse_lua51(&allocator, "function f(...) return ... end");
    assert_no_errors(&result);
    if let Statement::FunctionDecl(function_decl) = &result.chunk.block.stmts[0] {
        assert!(function_decl.body.vararg.is_some());
    } else {
        panic!("expected FunctionDecl");
    }
}

#[test]
fn varargs_with_named_params() {
    let allocator = Allocator::new();
    let result = parse_lua51(&allocator, "function f(a, b, ...) end");
    assert_no_errors(&result);
    if let Statement::FunctionDecl(function_decl) = &result.chunk.block.stmts[0] {
        assert!(function_decl.body.vararg.is_some());
        let param_count = function_decl.body.params.len();
        assert_eq!(param_count, 2);
    } else {
        panic!("expected FunctionDecl");
    }
}

#[test]
fn return_no_values() {
    let allocator = Allocator::new();
    let result = parse_lua51(&allocator, "return");
    assert_no_errors(&result);
    assert!(matches!(*result.chunk.block.stmts, [Statement::Return(_)]));
}

#[test]
fn return_multiple_values() {
    let allocator = Allocator::new();
    let result = parse_lua51(&allocator, "return 1, 2, 3");
    assert_no_errors(&result);
    if let [Statement::Return(return_stmt)] = &*result.chunk.block.stmts {
        let n = return_stmt.exprs.len();
        assert_eq!(n, 3);
    } else {
        panic!("expected Return");
    }
}

#[test]
fn break_as_last_stmt() {
    let allocator = Allocator::new();
    let result = parse_lua51(&allocator, "while true do break; end");
    assert_no_errors(&result);
    if let Statement::WhileLoop(while_loop) = &result.chunk.block.stmts[0] {
        assert!(matches!(*while_loop.block.stmts, [Statement::Break(_)]));
    } else {
        panic!("expected WhileLoop");
    }
}

#[test]
fn empty_program() {
    let allocator = Allocator::new();
    let result = parse_lua51(&allocator, "");
    assert_no_errors(&result);
    assert!(result.chunk.block.stmts.is_empty(), "{:?}", result.chunk.block.stmts);
}

#[test]
fn semicolon_separates_stmts_without_an_empty_stmt() {
    let allocator = Allocator::new();
    let result = parse_lua51(&allocator, "local x = 1; local y = 2");
    assert_no_errors(&result);
    assert_eq!(result.chunk.block.stmts.len(), 2);
}

#[test]
fn complex_expr_in_function() {
    let allocator = Allocator::new();
    let result = parse_lua51(&allocator, "local x = function() return 1 + 2 * 3 end");
    assert_no_errors(&result);
    let expr = local_value(&result);
    if let Expression::FunctionDef(function_def) = expr {
        assert!(matches!(*function_def.body.block.stmts, [Statement::Return(_)]));
    } else {
        panic!("expected FunctionDef");
    }
}

#[test]
fn logical_operators() {
    let allocator = Allocator::new();
    let result = parse_lua51(&allocator, "local x = a and b or c");
    assert_no_errors(&result);
    // `and` binds tighter than `or`, so this is `(a and b) or c`.
    let expr = local_value(&result);
    assert_eq!(run_ops(expr), [BinOp::Or]);
    assert_eq!(run_ops(run_operands(expr)[0]), [BinOp::And]);
}

#[test]
fn comparison_operators() {
    let allocator = Allocator::new();
    for (source, expected_op) in [
        ("local x = a < b", BinOp::Lt),
        ("local x = a >= b", BinOp::Ge),
        ("local x = a ~= b", BinOp::Ne),
        ("local x = a == b", BinOp::Eq),
    ] {
        let result = parse_lua51(&allocator, source);
        assert_no_errors(&result);
        assert_eq!(run_ops(local_value(&result)), [expected_op], "failed for: {source}");
    }
}

#[test]
fn field_access_then_call() {
    let allocator = Allocator::new();
    let result = parse_lua51(&allocator, "a.b.c()");
    assert_no_errors(&result);
    assert!(matches!(&result.chunk.block.stmts[0], Statement::FunctionCall(_)));
}

#[test]
fn multiline_program() {
    let allocator = Allocator::new();
    let source = "
local x = 10
local y = 20
function add(a, b)
    return a + b
end
local result = add(x, y)
print(result)
";
    let result = parse_lua51(&allocator, source);
    assert_no_errors(&result);
    assert_eq!(result.chunk.block.stmts.len(), 5);
}

#[test]
fn multiline_string_level3_brackets() {
    let allocator = Allocator::new();
    let result = parse_lua51(&allocator, "local x = [===[hello ]]] ]==] world]===]");
    assert_no_errors(&result);
    let expr = local_value(&result);
    assert!(matches!(*expr, Expression::StringLiteral(_)));
}

#[test]
fn deeply_nested_table() {
    let allocator = Allocator::new();
    let result = parse_lua51(&allocator, "local x = {{{{}}}}");
    assert_no_errors(&result);
    let expr = local_value(&result);
    if let Expression::TableConstructor(outer) = expr {
        assert_eq!(outer.fields.len(), 1);
        if let Field::Positional(Expression::TableConstructor(middle)) = &outer.fields[0] {
            assert_eq!(middle.fields.len(), 1);
            if let Field::Positional(Expression::TableConstructor(inner)) = &middle.fields[0] {
                assert_eq!(inner.fields.len(), 1);
                assert!(matches!(
                    &inner.fields[0],
                    Field::Positional(Expression::TableConstructor(innermost)) if innermost.fields.is_empty()
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
    let allocator = Allocator::new();
    let result = parse_lua51(&allocator, "a:b():c():d()");
    assert_no_errors(&result);
    let Statement::FunctionCall(call) = &result.chunk.block.stmts[0] else {
        panic!("expected FunctionCall");
    };
    assert_eq!(chain_shape(call), "a:b():c():d()");
}

#[test]
fn call_on_parenthesized_expr() {
    let allocator = Allocator::new();
    let result = parse_lua51(&allocator, "(f)(x); ((g).y).z(w); (h())()");
    assert_no_errors(&result);
    let [
        Statement::FunctionCall(call),
        Statement::FunctionCall(joined),
        Statement::FunctionCall(truncated),
    ] = &*result.chunk.block.stmts
    else {
        panic!("expected three calls, got {:?}", result.chunk.block.stmts);
    };
    assert_eq!(chain_shape(call), "f()");
    assert_eq!(chain_shape(joined), "g.y.z()", "a parenthesized chain reads on");
    assert_eq!(joined.span, Span::new(8, 20));
    assert_eq!(chain_shape(truncated), "()()", "a parenthesized call takes one value");
}

#[test]
fn a_call_spans_the_parentheses_its_callee_is_in() {
    let allocator = Allocator::new();
    let source = "(f)(x); (\"s\"):rep(2); (a).b.c()";
    let result = parse_lua51(&allocator, source);
    assert_no_errors(&result);
    let calls: Vec<&str> = result
        .chunk
        .block
        .stmts
        .iter()
        .filter_map(|stmt| {
            if let Statement::FunctionCall(chain) = stmt { chain.last_call() } else { None }
        })
        .map(|call| str::from_utf8(call.span().text(source.as_bytes())).unwrap())
        .collect();
    assert_eq!(calls, ["(f)(x)", "(\"s\"):rep(2)", "(a).b.c()"]);
    let Some(Statement::FunctionCall(fields)) = result.chunk.block.stmts.last() else {
        panic!("expected a call last");
    };
    assert_eq!(fields.head(1).span().text(source.as_bytes()), b"(a).b");
    assert_eq!(fields.head(0).span().text(source.as_bytes()), b"a", "a prefix alone is itself");
}
