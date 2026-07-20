use luck_ast::expr::*;
use luck_ast::stmt::TypeDeclarationValue;
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
        assert_eq!(ca.op, luck_token::CompoundOp::AddAssign);
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
        assert_eq!(ca.op, luck_token::CompoundOp::MulAssign);
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
        assert_eq!(ca.op, luck_token::CompoundOp::ConcatAssign);
    } else {
        panic!("expected CompoundAssignment");
    }
}

#[test]
fn compound_assign_all_operators() {
    for (src, expected_kind) in [
        ("x += 1", luck_token::CompoundOp::AddAssign),
        ("x -= 1", luck_token::CompoundOp::SubAssign),
        ("x *= 1", luck_token::CompoundOp::MulAssign),
        ("x /= 1", luck_token::CompoundOp::DivAssign),
        ("x //= 1", luck_token::CompoundOp::FloorDivAssign),
        ("x %= 1", luck_token::CompoundOp::ModAssign),
        ("x ^= 1", luck_token::CompoundOp::PowAssign),
        ("x ..= 1", luck_token::CompoundOp::ConcatAssign),
    ] {
        let result = parse_luau(src);
        assert_no_errors(&result);
        if let Statement::CompoundAssignment(ca) = &result.block.stmts[0] {
            assert_eq!(ca.op, expected_kind, "failed for: {}", src);
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
        let exprs = la.exprs.as_ref().expect("assignment has values");
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
        let exprs = la.exprs.as_ref().expect("assignment has values");
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
        let exprs = la.exprs.as_ref().expect("assignment has values");
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
        let exprs = la.exprs.as_ref().expect("assignment has values");
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
        let exprs = la.exprs.as_ref().expect("assignment has values");
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
        let exprs = la.exprs.as_ref().expect("assignment has values");
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
        let exprs = la.exprs.as_ref().expect("assignment has values");
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
    let exprs = la.exprs.as_ref().expect("assignment has values");
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
    // Grammar: asexp ::= simpleexp ['::' Type] - one cast per simpleexp;
    // real Luau rejects a second `::` (wrap in parens to chain).
    let result = parse_luau("local x = v :: A :: B");
    assert!(!result.errors.is_empty(), "chained casts must error");
    let result = parse_luau("local x = (v :: A) :: B");
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
    let result = parse_luau("type Pair<T> = { first: T, second: T }");
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
        assert!(td.is_exported);
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
        assert!(decl.is_exported);
        assert!(matches!(
            decl.type_value,
            TypeDeclarationValue::TypeFunction(_)
        ));
    } else {
        panic!("expected TypeDeclaration");
    }
}

#[test]
fn bitwise_operators_rejected() {
    // Luau has no bitwise operators; `&`/`|` only exist in type syntax
    // (compatibility page: bitwise operators = not supported).
    for source in [
        "local a = 1 & 2",
        "local a = 1 | 2",
        "return x & y",
        "return x | y",
    ] {
        let result = parse_luau(source);
        assert!(
            !result.errors.is_empty(),
            "Luau must reject bitwise binop: {source}"
        );
    }
    // The same tokens still work in type positions.
    let result = parse_luau("type U = number | string\ntype I = A & B");
    assert!(result.errors.is_empty(), "{:?}", result.errors);
}

#[test]
fn const_bindings() {
    // Grammar: 'const' bindinglist '=' explist | 'const' 'function' NAME funcbody
    let result = parse_luau("const x = 1");
    assert_no_errors(&result);
    if let Statement::LocalAssignment(local) = &result.block.stmts[0] {
        assert!(local.is_const);
        assert!(local.exprs.is_some());
    } else {
        panic!("expected LocalAssignment, got {:?}", result.block.stmts[0]);
    }

    let result = parse_luau("const a, b = 1, 2");
    assert_no_errors(&result);

    let result = parse_luau("const n: number = 5");
    assert_no_errors(&result);

    let result = parse_luau("const function cf() end");
    assert_no_errors(&result);
    if let Statement::LocalFunction(func) = &result.block.stmts[0] {
        assert!(func.is_const);
    } else {
        panic!("expected LocalFunction, got {:?}", result.block.stmts[0]);
    }
}

#[test]
fn const_requires_initializer() {
    let result = parse_luau("const x");
    assert!(!result.errors.is_empty(), "missing initializer must error");
}

#[test]
fn const_stays_contextual() {
    // `const` remains an ordinary identifier when no binding follows.
    assert_no_errors(&parse_luau("const = 1"));
    assert_no_errors(&parse_luau("const.x = 1"));
    assert_no_errors(&parse_luau("const()"));
    assert_no_errors(&parse_luau("local const = 2\nprint(const)"));
    // And non-Luau versions never treat it specially.
    let result = luck_parser::parse("const x = 1", luck_token::LuaVersion::Lua54);
    assert!(!result.errors.is_empty(), "const decl is Luau-only");
}

#[test]
fn bracketed_and_parameterized_attributes() {
    // attribute ::= '@' NAME | '@[' parattr {',' parattr} ']'
    assert_no_errors(&parse_luau("@[deprecated] function f() end"));
    assert_no_errors(&parse_luau("@[deprecated(\"use g\")] function f() end"));
    assert_no_errors(&parse_luau("@[checked, native] function f() end"));
    // Arguments must be literals, and only deprecated takes them.
    assert!(
        !parse_luau("@[deprecated(foo())] function f() end")
            .errors
            .is_empty()
    );
    assert!(
        !parse_luau("@[native(1)] function f() end")
            .errors
            .is_empty()
    );
}

#[test]
fn attributes_on_function_expressions() {
    // simpleexp ::= attributes 'function' funcbody
    assert_no_errors(&parse_luau("local k = @native function() end"));
    assert_no_errors(&parse_luau("f(@checked function() end)"));
    assert!(
        !parse_luau("local k = @nonsense function() end")
            .errors
            .is_empty()
    );
}

#[test]
fn empty_interpolation_rejected() {
    let result = parse_luau("return `a{}b`");
    assert!(!result.errors.is_empty(), "empty interpolation must error");
    // Plain backtick strings still parse.
    assert_no_errors(&parse_luau("return `plain`"));
    assert_no_errors(&parse_luau("return ``"));
    assert_no_errors(&parse_luau("return `a{1}b`"));
}
