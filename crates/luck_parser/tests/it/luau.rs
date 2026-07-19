use luck_ast::expr::*;
use luck_ast::{Expression, LastStatement, Statement};

use crate::common::{assert_no_errors, parse_luau};

#[test]
fn compound_assign_plus() {
    let result = parse_luau("x += 1");
    assert_no_errors(&result);
    assert_eq!(result.block.stmts.len(), 1);
    if let Statement::CompoundAssignment(ca) = &result.block.stmts[0] {
        assert!(
            matches!(&ca.var, Var::Name(t) if t.kind == luck_token::TokenKind::Identifier("x".into()))
        );
        assert_eq!(ca.op.kind, luck_token::TokenKind::PlusEqual);
    } else {
        panic!(
            "expected CompoundAssignment, got {:?}",
            result.block.stmts[0]
        );
    }
}

#[test]
fn compound_assign_index() {
    let result = parse_luau("t[i] *= 2");
    assert_no_errors(&result);
    assert_eq!(result.block.stmts.len(), 1);
    if let Statement::CompoundAssignment(ca) = &result.block.stmts[0] {
        assert!(matches!(&ca.var, Var::Index(_)));
        assert_eq!(ca.op.kind, luck_token::TokenKind::StarEqual);
    } else {
        panic!("expected CompoundAssignment");
    }
}

#[test]
fn compound_assign_field_concat() {
    let result = parse_luau(r#"a.b ..= "c""#);
    assert_no_errors(&result);
    assert_eq!(result.block.stmts.len(), 1);
    if let Statement::CompoundAssignment(ca) = &result.block.stmts[0] {
        assert!(matches!(&ca.var, Var::FieldAccess(_)));
        assert_eq!(ca.op.kind, luck_token::TokenKind::DotDotEqual);
    } else {
        panic!("expected CompoundAssignment");
    }
}

#[test]
fn compound_assign_all_operators() {
    for (src, expected_kind) in [
        ("x += 1", luck_token::TokenKind::PlusEqual),
        ("x -= 1", luck_token::TokenKind::MinusEqual),
        ("x *= 1", luck_token::TokenKind::StarEqual),
        ("x /= 1", luck_token::TokenKind::SlashEqual),
        ("x //= 1", luck_token::TokenKind::FloorDivEqual),
        ("x %= 1", luck_token::TokenKind::PercentEqual),
        ("x ^= 1", luck_token::TokenKind::CaretEqual),
        ("x ..= 1", luck_token::TokenKind::DotDotEqual),
    ] {
        let result = parse_luau(src);
        assert_no_errors(&result);
        if let Statement::CompoundAssignment(ca) = &result.block.stmts[0] {
            assert_eq!(ca.op.kind, expected_kind, "failed for: {}", src);
        } else {
            panic!("expected CompoundAssignment for: {}", src);
        }
    }
}

#[test]
fn if_expression_simple() {
    let result = parse_luau("local x = if a then 1 else 2");
    assert_no_errors(&result);
    assert_eq!(result.block.stmts.len(), 1);
    if let Statement::LocalAssignment(la) = &result.block.stmts[0] {
        let (_, exprs) = la.equal_and_exprs.as_ref().expect("assignment has values");
        let expr = exprs.last_item().expect("expression list has last element");
        assert!(matches!(expr, Expression::IfExpression(_)));
        if let Expression::IfExpression(ie) = expr {
            assert!(ie.elseif_clauses.is_empty());
        }
    } else {
        panic!("expected LocalAssignment");
    }
}

#[test]
fn if_expression_with_elseif() {
    let result = parse_luau("local y = if a then b elseif c then d else e");
    assert_no_errors(&result);
    if let Statement::LocalAssignment(la) = &result.block.stmts[0] {
        let (_, exprs) = la.equal_and_exprs.as_ref().expect("assignment has values");
        let expr = exprs.last_item().expect("expression list has last element");
        if let Expression::IfExpression(ie) = expr {
            assert_eq!(ie.elseif_clauses.len(), 1);
        } else {
            panic!("expected IfExpression");
        }
    } else {
        panic!("expected LocalAssignment");
    }
}

#[test]
fn if_expression_nested() {
    let result = parse_luau("local z = if a then if b then 1 else 2 else 3");
    assert_no_errors(&result);
    if let Statement::LocalAssignment(la) = &result.block.stmts[0] {
        let (_, exprs) = la.equal_and_exprs.as_ref().expect("assignment has values");
        let expr = exprs.last_item().expect("expression list has last element");
        if let Expression::IfExpression(ie) = expr {
            assert!(matches!(&ie.then_expr, Expression::IfExpression(_)));
        } else {
            panic!("expected IfExpression");
        }
    } else {
        panic!("expected LocalAssignment");
    }
}

#[test]
fn interp_string_plain() {
    let result = parse_luau("local x = `hello`");
    assert_no_errors(&result);
    if let Statement::LocalAssignment(la) = &result.block.stmts[0] {
        let (_, exprs) = la.equal_and_exprs.as_ref().expect("assignment has values");
        let expr = exprs.last_item().expect("expression list has last element");
        assert!(matches!(expr, Expression::InterpolatedString(_)));
    } else {
        panic!("expected LocalAssignment");
    }
}

#[test]
fn interp_string_with_expression() {
    let result = parse_luau("local x = `{y}`");
    assert_no_errors(&result);
    if let Statement::LocalAssignment(la) = &result.block.stmts[0] {
        let (_, exprs) = la.equal_and_exprs.as_ref().expect("assignment has values");
        let expr = exprs.last_item().expect("expression list has last element");
        if let Expression::InterpolatedString(is) = expr {
            // InterpBegin (with expr y) + InterpEnd
            assert_eq!(is.segments.len(), 2);
            assert!(is.segments[0].expr.is_some());
            assert!(is.segments[1].expr.is_none());
        } else {
            panic!("expected InterpolatedString");
        }
    } else {
        panic!("expected LocalAssignment");
    }
}

#[test]
fn interp_string_multiple_expressions() {
    let result = parse_luau("local x = `a{1+2}b{3}c`");
    assert_no_errors(&result);
    if let Statement::LocalAssignment(la) = &result.block.stmts[0] {
        let (_, exprs) = la.equal_and_exprs.as_ref().expect("assignment has values");
        let expr = exprs.last_item().expect("expression list has last element");
        if let Expression::InterpolatedString(is) = expr {
            // InterpBegin("a") + expr(1+2) -> InterpMid("b") + expr(3) -> InterpEnd("c")
            assert_eq!(is.segments.len(), 3);
            assert!(is.segments[0].expr.is_some()); // 1+2
            assert!(is.segments[1].expr.is_some()); // 3
            assert!(is.segments[2].expr.is_none()); // end
        } else {
            panic!("expected InterpolatedString");
        }
    } else {
        panic!("expected LocalAssignment");
    }
}

#[test]
fn local_with_type_annotation() {
    let result = parse_luau("local x: number = 5");
    assert_no_errors(&result);
    assert_eq!(result.block.stmts.len(), 1);
    assert!(matches!(
        &result.block.stmts[0],
        Statement::LocalAssignment(_)
    ));
}

#[test]
fn function_with_param_types() {
    let result = parse_luau("function f(a: string, b: number): boolean return true end");
    assert_no_errors(&result);
    assert_eq!(result.block.stmts.len(), 1);
    if let Statement::FunctionDecl(fd) = &result.block.stmts[0] {
        let param_count = fd.body.params.len();
        assert_eq!(param_count, 2);
        assert!(fd.body.return_type.is_some());
    } else {
        panic!("expected FunctionDecl");
    }
}

#[test]
fn local_with_generic_type() {
    let result = parse_luau("local t: Array<number> = {}");
    assert_no_errors(&result);
    assert!(matches!(
        &result.block.stmts[0],
        Statement::LocalAssignment(_)
    ));
}

#[test]
fn type_cast_expression() {
    let result = parse_luau("local x = y :: number");
    assert_no_errors(&result);
    if let Statement::LocalAssignment(la) = &result.block.stmts[0] {
        let (_, exprs) = la.equal_and_exprs.as_ref().expect("assignment has values");
        let expr = exprs.last_item().expect("expression list has last element");
        assert!(matches!(expr, Expression::TypeCast(_)));
    } else {
        panic!("expected LocalAssignment");
    }
}

/// `::` applies to any simpleexp, not just prefix expressions. Literals,
/// tables, and call results must accept a cast (Luau `asexp = simpleexp
/// ['::' Type]`). Regression test for the parser gap that also made the
/// minifier emit unparseable `print(1::number)`.
#[test]
fn type_cast_on_literals_and_tables() {
    for source in [
        "local x = 1 :: number",
        "local x = \"s\" :: string",
        "local x = {} :: Foo",
        "local x = true :: boolean",
        "print(1 :: number)",
        "local x = f() :: T",
    ] {
        let result = parse_luau(source);
        assert_no_errors(&result);
    }
}

/// A cast binds to the simpleexp, tighter than binary operators:
/// `a + b :: number` is `a + (b :: number)`.
#[test]
fn type_cast_binds_tighter_than_binary_op() {
    let result = parse_luau("local x = a + b :: number");
    assert_no_errors(&result);
    let Statement::LocalAssignment(la) = &result.block.stmts[0] else {
        panic!("expected LocalAssignment");
    };
    let (_, exprs) = la.equal_and_exprs.as_ref().expect("assignment has values");
    let expr = exprs.last_item().expect("has last element");
    let Expression::BinaryOp(binop) = expr else {
        panic!("expected top-level BinaryOp, got {expr:?}");
    };
    assert!(
        matches!(binop.right, Expression::TypeCast(_)),
        "cast must bind to the right operand: a + (b :: number)"
    );
}

/// Chained assertions parse left-to-right.
#[test]
fn type_cast_chained() {
    let result = parse_luau("local x = v :: A :: B");
    assert_no_errors(&result);
}

#[test]
fn type_declaration_simple() {
    let result = parse_luau("type Foo = number");
    assert_no_errors(&result);
    assert_eq!(result.block.stmts.len(), 1);
    if let Statement::TypeDeclaration(td) = &result.block.stmts[0] {
        assert_eq!(
            td.name.kind,
            luck_token::TokenKind::Identifier("Foo".into())
        );
    } else {
        panic!("expected TypeDeclaration, got {:?}", result.block.stmts[0]);
    }
}

#[test]
fn type_declaration_generic() {
    let result = parse_luau("type Pair<T> = {T, T}");
    assert_no_errors(&result);
    if let Statement::TypeDeclaration(td) = &result.block.stmts[0] {
        assert!(td.generics.is_some());
    } else {
        panic!("expected TypeDeclaration");
    }
}

#[test]
fn export_type_declaration() {
    let result = parse_luau("export type Bar = string");
    assert_no_errors(&result);
    if let Statement::TypeDeclaration(td) = &result.block.stmts[0] {
        assert!(td.export_token.is_some());
    } else {
        panic!("expected TypeDeclaration");
    }
}

#[test]
fn type_declaration_record() {
    let result = parse_luau("export type Point = {x: number, y: number}");
    assert_no_errors(&result);
    assert!(matches!(
        &result.block.stmts[0],
        Statement::TypeDeclaration(_)
    ));
}

#[test]
fn consecutive_type_declarations() {
    let source = "type A = number\ntype B = string";
    let result = parse_luau(source);
    assert_no_errors(&result);
    assert_eq!(result.block.stmts.len(), 2);
}

#[test]
fn type_function_declaration() {
    let result = parse_luau("type function identity(t) return t end");
    assert_no_errors(&result);
    assert!(matches!(
        &result.block.stmts[0],
        Statement::TypeDeclaration(_)
    ));
}

#[test]
fn continue_in_for_loop() {
    let result = parse_luau("for i=1,10 do continue end");
    assert_no_errors(&result);
    if let Statement::NumericFor(nf) = &result.block.stmts[0] {
        assert!(matches!(
            nf.block.last_stmt.as_deref(),
            Some(LastStatement::Continue(_))
        ));
    } else {
        panic!("expected NumericFor");
    }
}

#[test]
fn continue_as_identifier() {
    // `continue` used as a variable name - should parse as assignment
    let result = parse_luau("local continue = 5");
    assert_no_errors(&result);
    assert!(matches!(
        &result.block.stmts[0],
        Statement::LocalAssignment(_)
    ));
}

#[test]
fn continue_in_while_loop() {
    let result = parse_luau("while true do continue end");
    assert_no_errors(&result);
    if let Statement::WhileLoop(wl) = &result.block.stmts[0] {
        assert!(matches!(
            wl.block.last_stmt.as_deref(),
            Some(LastStatement::Continue(_))
        ));
    } else {
        panic!("expected WhileLoop");
    }
}

#[test]
fn native_function_attribute() {
    let result = parse_luau("@native function f() end");
    assert_no_errors(&result);
    assert_eq!(result.block.stmts.len(), 1);
    assert!(matches!(&result.block.stmts[0], Statement::FunctionDecl(_)));
}

#[test]
fn native_local_function_attribute() {
    let result = parse_luau("@native local function f() end");
    assert_no_errors(&result);
    assert!(matches!(
        &result.block.stmts[0],
        Statement::LocalFunction(_)
    ));
}

#[test]
fn if_statement_in_luau() {
    let result = parse_luau("if true then x = 1 end");
    assert_no_errors(&result);
    assert!(matches!(&result.block.stmts[0], Statement::IfStatement(_)));
}

#[test]
fn break_in_luau() {
    // Luau keeps 5.1's `laststat` grammar (extended with `continue`):
    // break is a last statement, not a free-standing one.
    let result = parse_luau("while true do break end");
    assert_no_errors(&result);
    if let Statement::WhileLoop(wl) = &result.block.stmts[0] {
        assert!(wl.block.stmts.is_empty());
        assert!(matches!(
            wl.block.last_stmt.as_deref(),
            Some(LastStatement::Break(_))
        ));
    } else {
        panic!("expected WhileLoop");
    }
}

#[test]
fn break_mid_block_rejected_in_luau() {
    // Statements after `break` are a grammar error in Luau.
    let result = parse_luau("while true do break print(1) end");
    assert!(!result.errors.is_empty());
}

#[test]
fn numeric_for_with_type_annotation() {
    let result = parse_luau("for i: number = 1, 10 do end");
    assert_no_errors(&result);
    assert_eq!(result.block.stmts.len(), 1);
    assert!(matches!(&result.block.stmts[0], Statement::NumericFor(_)));
}

#[test]
fn generic_for_with_type_annotations() {
    let result = parse_luau("for k: string, v: number in pairs(t) do end");
    assert_no_errors(&result);
    assert_eq!(result.block.stmts.len(), 1);
    assert!(matches!(&result.block.stmts[0], Statement::GenericFor(_)));
}

#[test]
fn continue_as_function_call() {
    let source = "continue()";
    let result = parse_luau(source);
    assert_no_errors(&result);
    assert!(
        matches!(&result.block.stmts[0], Statement::FunctionCall(_)),
        "expected FunctionCall, got {:?}",
        result.block.stmts[0]
    );
}

#[test]
fn continue_as_field_access() {
    let source = "continue.field = 1";
    let result = parse_luau(source);
    assert_no_errors(&result);
    assert!(
        matches!(&result.block.stmts[0], Statement::Assignment(_)),
        "expected Assignment via field access, got {:?}",
        result.block.stmts[0]
    );
}

#[test]
fn nested_interpolated_strings() {
    // Nested interpolated strings are valid Luau but not yet supported by the
    // parser. This test documents the current behavior: parsing produces errors
    // rather than panicking.
    let source = "`outer{`inner{x}`}`";
    let result = parse_luau(source);
    assert!(
        !result.errors.is_empty(),
        "nested interpolated strings are not yet supported; expected parse errors"
    );
}

#[test]
fn nested_generic_type_annotation() {
    let source = "local x: Map<string, Array<number>> = {}";
    let result = parse_luau(source);
    assert_no_errors(&result);
}

#[test]
fn function_attributes_are_kept_on_ast() {
    let result = parse_luau("@native function f() end");
    assert_no_errors(&result);
    if let Statement::FunctionDecl(decl) = &result.block.stmts[0] {
        assert_eq!(decl.attributes.len(), 1);
    } else {
        panic!("expected FunctionDecl");
    }

    let result = parse_luau("@native @checked local function g() end");
    assert_no_errors(&result);
    if let Statement::LocalFunction(func) = &result.block.stmts[0] {
        assert_eq!(func.attributes.len(), 2);
    } else {
        panic!("expected LocalFunction");
    }
}

#[test]
fn attribute_on_local_variable_rejected() {
    // Luau only allows attributes before function declarations.
    let result = parse_luau("@native local x = 1");
    assert!(!result.errors.is_empty());
}

#[test]
fn function_attributes_roundtrip_through_codegen() {
    let source = "@native function f() end";
    let result = parse_luau(source);
    assert_no_errors(&result);
    let output = luck_codegen::compact(&result.block, &result.source);
    assert!(
        output.contains("@native"),
        "attribute dropped from output: {output:?}"
    );
    let reparsed = parse_luau(&output);
    assert_no_errors(&reparsed);
}

#[test]
fn type_function_roundtrips_through_codegen() {
    let source = "type function Pair(t)\n\treturn t\nend";
    let result = parse_luau(source);
    assert_no_errors(&result);
    let output = luck_codegen::compact(&result.block, &result.source);
    assert!(
        output.contains("type function Pair") || output.contains("type function\u{20}Pair"),
        "type function mangled: {output:?}"
    );
    assert!(
        !output.contains('='),
        "type function must not gain an '=': {output:?}"
    );
    let reparsed = parse_luau(&output);
    assert_no_errors(&reparsed);
}

#[test]
fn export_type_function_parses() {
    let result = parse_luau("export type function Id(t) return t end");
    assert_no_errors(&result);
    if let Statement::TypeDeclaration(decl) = &result.block.stmts[0] {
        assert!(decl.export_token.is_some());
        assert!(decl.function_token.is_some());
        assert!(decl.equal.is_none());
    } else {
        panic!("expected TypeDeclaration");
    }
}
