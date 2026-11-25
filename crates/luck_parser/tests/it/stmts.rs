use luck_allocator::Allocator;
use luck_ast::{Expression, LocalAssignmentKind, LocalFunctionKind, Statement, Var};
use luck_parser::parse;
use luck_token::CompoundOp;

use crate::common::{assert_has_errors, assert_no_errors, chain_shape, parse_str, target_chain};

#[test]
fn do_end_is_a_do_block() {
    let allocator = Allocator::new();
    let result = parse_str(&allocator, "do end");
    assert_no_errors(&result);
    assert_eq!(result.chunk.stmts.len(), 1);
    assert!(matches!(&result.chunk.stmts[0], Statement::DoBlock(_)));
}

#[test]
fn nested_do_blocks_nest_in_the_tree() {
    let allocator = Allocator::new();
    let result = parse_str(&allocator, "do do do end end end");
    assert_no_errors(&result);
    assert_eq!(result.chunk.stmts.len(), 1);
    if let Statement::DoBlock(outer) = &result.chunk.stmts[0] {
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
fn while_loop_keeps_its_condition() {
    let allocator = Allocator::new();
    let result = parse_str(&allocator, "while true do end");
    assert_no_errors(&result);
    assert_eq!(result.chunk.stmts.len(), 1);
    if let Statement::WhileLoop(while_loop) = &result.chunk.stmts[0] {
        assert!(matches!(&while_loop.condition, Expression::True(_)));
    } else {
        panic!("expected WhileLoop");
    }
}

#[test]
fn repeat_loop_keeps_its_condition() {
    let allocator = Allocator::new();
    let result = parse_str(&allocator, "repeat until false");
    assert_no_errors(&result);
    assert_eq!(result.chunk.stmts.len(), 1);
    if let Statement::RepeatLoop(repeat_loop) = &result.chunk.stmts[0] {
        assert!(matches!(&repeat_loop.condition, Expression::False(_)));
    } else {
        panic!("expected RepeatLoop");
    }
}

#[test]
fn if_then_end_is_an_if_statement() {
    let allocator = Allocator::new();
    let result = parse_str(&allocator, "if true then end");
    assert_no_errors(&result);
    assert_eq!(result.chunk.stmts.len(), 1);
    assert!(matches!(&result.chunk.stmts[0], Statement::IfStatement(_)));
}

#[test]
fn if_statement_keeps_its_elseif_and_else_clauses() {
    let allocator = Allocator::new();
    let result = parse_str(&allocator, "if x then a() elseif y then b() else c() end");
    assert_no_errors(&result);
    if let Statement::IfStatement(if_stmt) = &result.chunk.stmts[0] {
        assert_eq!(if_stmt.elseif_clauses.len(), 1);
        assert!(if_stmt.else_clause.is_some());
    } else {
        panic!("expected IfStatement");
    }
}

#[test]
fn numeric_for_without_a_step_has_none() {
    let allocator = Allocator::new();
    let result = parse_str(&allocator, "for i = 1, 10 do end");
    assert_no_errors(&result);
    if let Statement::NumericFor(numeric_for) = &result.chunk.stmts[0] {
        assert!(numeric_for.step.is_none());
    } else {
        panic!("expected NumericFor");
    }
}

#[test]
fn numeric_for_keeps_its_step() {
    let allocator = Allocator::new();
    let result = parse_str(&allocator, "for i = 1, 10, 2 do end");
    assert_no_errors(&result);
    if let Statement::NumericFor(numeric_for) = &result.chunk.stmts[0] {
        assert!(numeric_for.step.is_some());
    } else {
        panic!("expected NumericFor");
    }
}

#[test]
fn generic_for_keeps_each_name() {
    let allocator = Allocator::new();
    let result = parse_str(&allocator, "for k, v in pairs(t) do end");
    assert_no_errors(&result);
    if let Statement::GenericFor(generic_for) = &result.chunk.stmts[0] {
        assert_eq!(generic_for.names.len(), 2);
    } else {
        panic!("expected GenericFor");
    }
}

#[test]
fn local_without_values_has_no_exprs() {
    let allocator = Allocator::new();
    let result = parse_str(&allocator, "local x");
    assert_no_errors(&result);
    if let Statement::LocalAssignment(local) = &result.chunk.stmts[0] {
        assert!(local.exprs.is_empty());
    } else {
        panic!("expected LocalAssignment");
    }
}

#[test]
fn local_keeps_its_value() {
    let allocator = Allocator::new();
    let result = parse_str(&allocator, "local x = 42");
    assert_no_errors(&result);
    if let Statement::LocalAssignment(local) = &result.chunk.stmts[0] {
        assert!(!local.exprs.is_empty());
    } else {
        panic!("expected LocalAssignment");
    }
}

#[test]
fn local_keeps_each_name() {
    let allocator = Allocator::new();
    let result = parse_str(&allocator, "local a, b, c = 1, 2, 3");
    assert_no_errors(&result);
    if let Statement::LocalAssignment(local) = &result.chunk.stmts[0] {
        let name_count = local.names.len();
        assert_eq!(name_count, 3);
    } else {
        panic!("expected LocalAssignment");
    }
}

#[test]
fn name_equals_value_is_an_assignment() {
    let allocator = Allocator::new();
    let result = parse_str(&allocator, "x = 1");
    assert_no_errors(&result);
    assert!(matches!(&result.chunk.stmts[0], Statement::Assignment(_)));
}

#[test]
fn assignment_keeps_each_target() {
    let allocator = Allocator::new();
    let result = parse_str(&allocator, "a, b, c = 1, 2, 3");
    assert_no_errors(&result);
    if let Statement::Assignment(assignment) = &result.chunk.stmts[0] {
        let target_count = assignment.targets.len();
        assert_eq!(target_count, 3);
    } else {
        panic!("expected Assignment");
    }
}

#[test]
fn field_chain_is_an_assignment_target() {
    let allocator = Allocator::new();
    let result = parse_str(&allocator, "a.b.c = 1");
    assert_no_errors(&result);
    let Statement::Assignment(assignment) = &result.chunk.stmts[0] else {
        panic!("expected Assignment");
    };
    let target = assignment.targets.last().expect("assignment has target");
    assert_eq!(chain_shape(target_chain(target)), "a.b.c");
}

#[test]
fn index_is_an_assignment_target() {
    let allocator = Allocator::new();
    let result = parse_str(&allocator, "a[i] = 1");
    assert_no_errors(&result);
    let Statement::Assignment(assignment) = &result.chunk.stmts[0] else {
        panic!("expected Assignment");
    };
    let target = assignment.targets.last().expect("assignment has target");
    assert_eq!(chain_shape(target_chain(target)), "a[]");
}

#[test]
fn chain_ending_in_an_index_is_an_assignment_target() {
    let allocator = Allocator::new();
    let result = parse_str(&allocator, "a.b[1]:c(2).d[3] = 5");
    assert_no_errors(&result);
    let Statement::Assignment(assignment) = &result.chunk.stmts[0] else {
        panic!("expected Assignment");
    };
    let target = assignment.targets.last().expect("assignment has target");
    assert_eq!(chain_shape(target_chain(target)), "a.b[]:c().d[]");
}

#[test]
fn call_on_a_name_is_a_call_statement() {
    let allocator = Allocator::new();
    let result = parse_str(&allocator, "print(\"hello\")");
    assert_no_errors(&result);
    assert!(matches!(&result.chunk.stmts[0], Statement::FunctionCall(_)));
}

#[test]
fn method_call_is_a_call_statement() {
    let allocator = Allocator::new();
    let result = parse_str(&allocator, "obj:method(1, 2)");
    assert_no_errors(&result);
    let Statement::FunctionCall(call) = &result.chunk.stmts[0] else {
        panic!("expected FunctionCall");
    };
    assert_eq!(chain_shape(call), "obj:method()");
}

#[test]
fn a_parenthesized_variable_is_no_assignment_target() {
    let allocator = Allocator::new();
    for source in ["(a) = 1", "a, (b) = 1, 2", "(a.b) = 1", "((a)) = 1"] {
        let result = parse_str(&allocator, source);
        let [error] = result.errors.as_slice() else {
            panic!("{source}: {:?}", result.errors);
        };
        assert_eq!(error.message, "invalid assignment target", "{source}");
    }
    assert_no_errors(&parse_str(&allocator, "(a).b = 1 (a)[1] = 2 (f)()"));
}

#[test]
fn return_without_values_is_a_return() {
    let allocator = Allocator::new();
    let result = parse_str(&allocator, "return");
    assert_no_errors(&result);
    assert!(matches!(*result.chunk.stmts, [Statement::Return(_)]));
}

#[test]
fn return_keeps_each_value() {
    let allocator = Allocator::new();
    let result = parse_str(&allocator, "return 1, 2, 3");
    assert_no_errors(&result);
    if let [Statement::Return(return_stmt)] = &*result.chunk.stmts {
        let n = return_stmt.exprs.len();
        assert_eq!(n, 3);
    } else {
        panic!("expected Return");
    }
}

#[test]
fn break_takes_a_trailing_semicolon() {
    let allocator = Allocator::new();
    let result = parse_str(&allocator, "while true do break; end");
    assert_no_errors(&result);
    if let Statement::WhileLoop(while_loop) = &result.chunk.stmts[0] {
        assert!(matches!(*while_loop.block.stmts, [Statement::Break(_)]));
    } else {
        panic!("expected WhileLoop");
    }
}

#[test]
fn empty_source_has_no_stmts() {
    let allocator = Allocator::new();
    let result = parse_str(&allocator, "");
    assert_no_errors(&result);
    assert!(result.chunk.stmts.is_empty(), "{:?}", result.chunk.stmts);
}

#[test]
fn semicolon_separates_stmts_without_an_empty_stmt() {
    let allocator = Allocator::new();
    let result = parse_str(&allocator, "local x = 1; local y = 2");
    assert_no_errors(&result);
    assert_eq!(result.chunk.stmts.len(), 2);
}

#[test]
fn call_after_fields_is_a_call_statement() {
    let allocator = Allocator::new();
    let result = parse_str(&allocator, "a.b.c()");
    assert_no_errors(&result);
    assert!(matches!(&result.chunk.stmts[0], Statement::FunctionCall(_)));
}

#[test]
fn statements_on_separate_lines_need_no_separator() {
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
    let result = parse_str(&allocator, source);
    assert_no_errors(&result);
    assert_eq!(result.chunk.stmts.len(), 5);
}

#[test]
fn return_takes_a_trailing_semicolon() {
    let allocator = Allocator::new();
    let result = parse_str(&allocator, "return 1;");
    assert_no_errors(&result);
    let [Statement::Return(return_stmt)] = &*result.chunk.stmts else {
        panic!("expected Return, got {:?}", result.chunk.stmts);
    };
    assert_eq!(return_stmt.exprs.len(), 1);
    assert_eq!(return_stmt.span.end, 9, "the `;` is the return's");
}

#[test]
fn compound_assignment_keeps_its_target_and_operator() {
    let allocator = Allocator::new();
    let result = parse_str(&allocator, "x += 1");
    assert_no_errors(&result);
    assert_eq!(result.chunk.stmts.len(), 1);
    if let Statement::CompoundAssignment(compound_assignment) = &result.chunk.stmts[0] {
        assert!(matches!(&compound_assignment.var, Var::Name(name) if name.text == "x"));
        assert_eq!(compound_assignment.op, CompoundOp::Add);
    } else {
        panic!("expected CompoundAssignment, got {:?}", result.chunk.stmts[0]);
    }
}

#[test]
fn index_is_a_compound_assignment_target() {
    let allocator = Allocator::new();
    let result = parse_str(&allocator, "t[i] *= 2");
    assert_no_errors(&result);
    assert_eq!(result.chunk.stmts.len(), 1);
    let Statement::CompoundAssignment(compound_assignment) = &result.chunk.stmts[0] else {
        panic!("expected CompoundAssignment");
    };
    assert_eq!(chain_shape(target_chain(&compound_assignment.var)), "t[]");
    assert_eq!(compound_assignment.op, CompoundOp::Mul);
}

#[test]
fn field_is_a_compound_assignment_target() {
    let allocator = Allocator::new();
    let result = parse_str(&allocator, r#"a.b ..= "c""#);
    assert_no_errors(&result);
    assert_eq!(result.chunk.stmts.len(), 1);
    let Statement::CompoundAssignment(compound_assignment) = &result.chunk.stmts[0] else {
        panic!("expected CompoundAssignment");
    };
    assert_eq!(chain_shape(target_chain(&compound_assignment.var)), "a.b");
    assert_eq!(compound_assignment.op, CompoundOp::Concat);
}

#[test]
fn each_compound_operator_is_its_op() {
    let allocator = Allocator::new();
    for (source, expected_op) in [
        ("x += 1", CompoundOp::Add),
        ("x -= 1", CompoundOp::Sub),
        ("x *= 1", CompoundOp::Mul),
        ("x /= 1", CompoundOp::Div),
        ("x //= 1", CompoundOp::FloorDiv),
        ("x %= 1", CompoundOp::Mod),
        ("x ^= 1", CompoundOp::Pow),
        ("x ..= 1", CompoundOp::Concat),
    ] {
        let result = parse_str(&allocator, source);
        assert_no_errors(&result);
        if let Statement::CompoundAssignment(compound_assignment) = &result.chunk.stmts[0] {
            assert_eq!(compound_assignment.op, expected_op, "failed for: {source}");
        } else {
            panic!("expected CompoundAssignment for: {source}");
        }
    }
}

#[test]
fn continue_is_a_statement_in_a_for_loop() {
    let allocator = Allocator::new();
    let result = parse_str(&allocator, "for i=1,10 do continue end");
    assert_no_errors(&result);
    if let Statement::NumericFor(numeric_for) = &result.chunk.stmts[0] {
        assert!(matches!(*numeric_for.block.stmts, [Statement::Continue(_)]));
    } else {
        panic!("expected NumericFor");
    }
}

#[test]
fn continue_is_a_name_after_local() {
    let allocator = Allocator::new();
    // `continue` used as a variable name parses as a local assignment.
    let result = parse_str(&allocator, "local continue = 5");
    assert_no_errors(&result);
    assert!(matches!(&result.chunk.stmts[0], Statement::LocalAssignment(_)));
}

#[test]
fn continue_is_a_statement_in_a_while_loop() {
    let allocator = Allocator::new();
    let result = parse_str(&allocator, "while true do continue end");
    assert_no_errors(&result);
    if let Statement::WhileLoop(while_loop) = &result.chunk.stmts[0] {
        assert!(matches!(*while_loop.block.stmts, [Statement::Continue(_)]));
    } else {
        panic!("expected WhileLoop");
    }
}

#[test]
fn if_at_stmt_start_is_an_if_stmt() {
    let allocator = Allocator::new();
    let result = parse_str(&allocator, "if true then x = 1 end");
    assert_no_errors(&result);
    assert!(matches!(&result.chunk.stmts[0], Statement::IfStatement(_)));
}

#[test]
fn break_ends_its_block_as_a_statement() {
    let allocator = Allocator::new();
    // Luau's grammar takes `break` only as the last statement of its block,
    // and the tree keeps it as a statement.
    let result = parse_str(&allocator, "while true do break end");
    assert_no_errors(&result);
    if let Statement::WhileLoop(while_loop) = &result.chunk.stmts[0] {
        assert!(matches!(*while_loop.block.stmts, [Statement::Break(_)]));
    } else {
        panic!("expected WhileLoop");
    }
}

#[test]
fn statement_after_break_is_an_error() {
    let allocator = Allocator::new();
    let result = parse_str(&allocator, "while true do break print(1) end");
    assert_ne!(result.errors, Vec::<luck_diagnostics::Diagnostic>::new());
}

#[test]
fn continue_before_parentheses_is_a_call() {
    let allocator = Allocator::new();
    let source = "continue()";
    let result = parse_str(&allocator, source);
    assert_no_errors(&result);
    assert!(
        matches!(&result.chunk.stmts[0], Statement::FunctionCall(_)),
        "expected FunctionCall, got {:?}",
        result.chunk.stmts[0]
    );
}

#[test]
fn continue_before_a_field_is_an_assignment_target() {
    let allocator = Allocator::new();
    let source = "continue.field = 1";
    let result = parse_str(&allocator, source);
    assert_no_errors(&result);
    assert!(
        matches!(&result.chunk.stmts[0], Statement::Assignment(_)),
        "expected Assignment via field access, got {:?}",
        result.chunk.stmts[0]
    );
}

#[test]
fn const_is_a_const_local_or_function() {
    let allocator = Allocator::new();
    // Grammar: 'const' bindinglist '=' explist | 'const' 'function' NAME funcbody
    let result = parse_str(&allocator, "const x = 1");
    assert_no_errors(&result);
    if let Statement::LocalAssignment(local) = &result.chunk.stmts[0] {
        assert_eq!(local.kind, LocalAssignmentKind::Const);
        assert!(!local.exprs.is_empty());
    } else {
        panic!("expected LocalAssignment, got {:?}", result.chunk.stmts[0]);
    }

    assert_no_errors(&parse_str(&allocator, "const a, b = 1, 2"));

    assert_no_errors(&parse_str(&allocator, "const n: number = 5"));

    let function = parse_str(&allocator, "const function cf() end");
    assert_no_errors(&function);
    if let Statement::LocalFunction(local_function) = &function.chunk.stmts[0] {
        assert_eq!(local_function.kind, LocalFunctionKind::Const);
    } else {
        panic!("expected LocalFunction, got {:?}", function.chunk.stmts[0]);
    }
}

#[test]
fn const_requires_initializer() {
    let allocator = Allocator::new();
    let result = parse_str(&allocator, "const x");
    assert!(!result.errors.is_empty(), "missing initializer must error");
}

#[test]
fn const_stays_contextual() {
    let allocator = Allocator::new();
    // `const` remains an ordinary identifier when no binding follows.
    assert_no_errors(&parse_str(&allocator, "const = 1"));
    assert_no_errors(&parse_str(&allocator, "const.x = 1"));
    assert_no_errors(&parse_str(&allocator, "const()"));
    assert_no_errors(&parse_str(
        &allocator,
        "\
local const = 2
print(const)",
    ));
}

#[test]
fn value_exports_parse_and_enforce_module_rules() {
    let allocator = Allocator::new();
    let result = parse_str(
        &allocator,
        "\
export local mutable: number = 1
export const FIXED = 2
@native
export function run() return mutable end",
    );
    assert_no_errors(&result);
    let [
        Statement::LocalAssignment(local),
        Statement::LocalAssignment(constant),
        Statement::LocalFunction(function),
    ] = &*result.chunk.stmts
    else {
        panic!("expected an exported local, const, and function: {:?}", result.chunk.stmts);
    };
    assert_eq!(local.kind, LocalAssignmentKind::ExportLocal);
    assert_eq!(constant.kind, LocalAssignmentKind::ExportConst);
    assert_eq!(function.kind, LocalFunctionKind::Export);
    assert_eq!(function.attributes.len(), 1);

    for source in [
        "\
export local x = 1
export local x = 2",
        "if true then export local x = 1 end",
        "\
export local x = 1
return x",
        "\
if true then return end
export local x = 1",
        "export local function f() end",
    ] {
        assert!(!parse_str(&allocator, source).errors.is_empty(), "{source} must be rejected");
    }

    // A type-only export still coexists with a module return.
    assert_no_errors(&parse_str(
        &allocator,
        "\
export type T = number
return 1",
    ));

    // `export` remains contextual when it does not introduce a declaration.
    assert_no_errors(&parse_str(
        &allocator,
        "\
export = 5
export += 1
export()
return export",
    ));
}

#[test]
fn const_needs_one_value_per_name_unless_the_last_is_multi_valued() {
    let allocator = Allocator::new();
    for source in
        ["const x, y = 1", "const x, y = (f())", "const x = 1, 2", "export const x, y = 1"]
    {
        let result = parse_str(&allocator, source);
        assert!(!result.errors.is_empty(), "luau reports a missing initializer in {source}");
    }
}

#[test]
fn break_outside_loop_rejected() {
    let allocator = Allocator::new();
    assert_has_errors(&parse(&allocator, b"break"));
    assert_has_errors(&parse(&allocator, b"do break end"));
    assert_has_errors(&parse(&allocator, b"function f() break end"));
    // Function bodies reset the loop context.
    assert_has_errors(&parse(&allocator, b"while true do local f = function() break end end"));
    assert_no_errors(&parse(&allocator, b"while true do break end"));
    assert_no_errors(&parse(&allocator, b"repeat break until x"));
    assert_no_errors(&parse(&allocator, b"for i = 1, 2 do break end"));
    assert_no_errors(&parse(&allocator, b"while true do do break end end"));
}

#[test]
fn continue_outside_loop_rejected() {
    let allocator = Allocator::new();
    assert_has_errors(&parse(&allocator, b"continue"));
    assert_has_errors(&parse(&allocator, b"while true do local f = function() continue end end"));
    assert_no_errors(&parse(&allocator, b"while true do continue end"));
}

#[test]
fn a_condition_opens_with_its_binding() {
    let allocator = Allocator::new();
    let result = parse_str(
        &allocator,
        "if local a: T = f() then elseif const b = g() then end while local c = h() do end",
    );
    assert_no_errors(&result);
    let [Statement::IfStatement(if_stmt), Statement::WhileLoop(while_loop)] = &*result.chunk.stmts
    else {
        panic!("{:?}", result.chunk.stmts);
    };
    let binding = if_stmt.binding.as_deref().expect("the `if` binds `a`");
    assert_eq!(binding.name.name.text.as_str(), "a");
    assert!(binding.name.type_annotation.is_some());
    assert!(!binding.is_const);
    assert_eq!(binding.span.text(b"if local a: T = f()"), b"local a: T");
    assert!(matches!(if_stmt.condition, Expression::Suffixed(_)));
    let clause_binding = if_stmt.elseif_clauses[0].binding.as_deref().expect("`elseif` binds `b`");
    assert_eq!(clause_binding.name.name.text.as_str(), "b");
    assert!(clause_binding.is_const);
    let loop_binding = while_loop.binding.as_deref().expect("the `while` binds `c`");
    assert_eq!(loop_binding.name.name.text.as_str(), "c");
}

#[test]
fn an_if_expression_condition_opens_with_its_binding() {
    let allocator = Allocator::new();
    let result =
        parse_str(&allocator, "return if local a = f() then a elseif const b = g() then b else c");
    assert_no_errors(&result);
    let [Statement::Return(return_stmt)] = &*result.chunk.stmts else {
        panic!("{:?}", result.chunk.stmts);
    };
    let [Expression::IfExpression(if_expr)] = &*return_stmt.exprs else {
        panic!("{:?}", return_stmt.exprs);
    };
    assert!(if_expr.binding.as_deref().is_some_and(|binding| !binding.is_const));
    assert!(if_expr.elseif_clauses[0].binding.as_deref().is_some_and(|binding| binding.is_const));
}

#[test]
fn const_before_anything_but_a_name_is_a_variable() {
    let allocator = Allocator::new();
    for source in ["if const then end", "if const.x then end", "if const == 1 then end"] {
        let result = parse_str(&allocator, source);
        assert_no_errors(&result);
        let [Statement::IfStatement(if_stmt)] = &*result.chunk.stmts else {
            panic!("{source}: {:?}", result.chunk.stmts);
        };
        assert!(if_stmt.binding.is_none(), "{source}");
    }
    let loop_result = parse_str(&allocator, "while const do end");
    assert_no_errors(&loop_result);
    let [Statement::WhileLoop(while_loop)] = &*loop_result.chunk.stmts else {
        panic!("{:?}", loop_result.chunk.stmts);
    };
    assert!(while_loop.binding.is_none());
    let expr_result = parse_str(&allocator, "return if const then 1 else 2");
    assert_no_errors(&expr_result);
    let [Statement::Return(return_stmt)] = &*expr_result.chunk.stmts else {
        panic!("{:?}", expr_result.chunk.stmts);
    };
    let [Expression::IfExpression(if_expr)] = &*return_stmt.exprs else {
        panic!("{:?}", return_stmt.exprs);
    };
    assert!(if_expr.binding.is_none());
}

#[test]
fn a_conditional_binding_binds_a_single_name() {
    let allocator = Allocator::new();
    let source = "if local a, b: T, c = f() then end";
    let result = parse_str(&allocator, source);
    let [error] = &*result.errors else {
        panic!("{:?}", result.errors);
    };
    assert_eq!(error.message, "`if local` binds a single name");
    assert_eq!(error.span, Some(luck_token::Span::new(10, 11)));
    let Statement::IfStatement(if_stmt) = &result.chunk.stmts[0] else {
        panic!("{:?}", result.chunk.stmts);
    };
    assert!(if_stmt.binding.is_some());

    let loop_result = parse_str(&allocator, "while const a, b = f() do end");
    assert_eq!(loop_result.errors[0].message, "`while const` binds a single name");
}

#[test]
fn a_conditional_binding_needs_its_initializer() {
    let allocator = Allocator::new();
    for source in ["if local a then end", "while local a do end", "return if local a then 1 else 2"]
    {
        assert_has_errors(&parse_str(&allocator, source));
    }
}
