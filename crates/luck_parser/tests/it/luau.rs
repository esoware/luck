use luck_ast::{
    ElseIfExprClause, Expression, LastStatement, LocalFunctionKind, Statement, Suffix,
    TypeDeclarationValue, Var,
};

use crate::common::{
    assert_no_errors, chain, chain_shape, local_value, parse_luau, run_operands, target_chain,
};

#[test]
fn compound_assign_plus() {
    let result = parse_luau("x += 1");
    assert_no_errors(&result);
    assert_eq!(result.block.stmts.len(), 1);
    if let Statement::CompoundAssignment(compound_assignment) = &result.block.stmts[0] {
        assert!(
            matches!(&compound_assignment.var, Var::Name(token) if token.kind == luck_token::TokenKind::Identifier("x".into()))
        );
        assert_eq!(compound_assignment.op, luck_token::CompoundOp::Add);
    } else {
        panic!("expected CompoundAssignment, got {:?}", result.block.stmts[0]);
    }
}

#[test]
fn compound_assign_index() {
    let result = parse_luau("t[i] *= 2");
    assert_no_errors(&result);
    assert_eq!(result.block.stmts.len(), 1);
    let Statement::CompoundAssignment(compound_assignment) = &result.block.stmts[0] else {
        panic!("expected CompoundAssignment");
    };
    assert_eq!(chain_shape(target_chain(&compound_assignment.var)), "t[]");
    assert_eq!(compound_assignment.op, luck_token::CompoundOp::Mul);
}

#[test]
fn compound_assign_field_concat() {
    let result = parse_luau(r#"a.b ..= "c""#);
    assert_no_errors(&result);
    assert_eq!(result.block.stmts.len(), 1);
    let Statement::CompoundAssignment(compound_assignment) = &result.block.stmts[0] else {
        panic!("expected CompoundAssignment");
    };
    assert_eq!(chain_shape(target_chain(&compound_assignment.var)), "a.b");
    assert_eq!(compound_assignment.op, luck_token::CompoundOp::Concat);
}

#[test]
fn compound_assign_all_operators() {
    for (source, expected_op) in [
        ("x += 1", luck_token::CompoundOp::Add),
        ("x -= 1", luck_token::CompoundOp::Sub),
        ("x *= 1", luck_token::CompoundOp::Mul),
        ("x /= 1", luck_token::CompoundOp::Div),
        ("x //= 1", luck_token::CompoundOp::FloorDiv),
        ("x %= 1", luck_token::CompoundOp::Mod),
        ("x ^= 1", luck_token::CompoundOp::Pow),
        ("x ..= 1", luck_token::CompoundOp::Concat),
    ] {
        let result = parse_luau(source);
        assert_no_errors(&result);
        if let Statement::CompoundAssignment(compound_assignment) = &result.block.stmts[0] {
            assert_eq!(compound_assignment.op, expected_op, "failed for: {source}");
        } else {
            panic!("expected CompoundAssignment for: {source}");
        }
    }
}

#[test]
fn if_expr_simple() {
    let result = parse_luau("local x = if a then 1 else 2");
    assert_no_errors(&result);
    assert_eq!(result.block.stmts.len(), 1);
    let expr = local_value(&result);
    assert!(matches!(expr, Expression::IfExpression(_)));
    if let Expression::IfExpression(if_expr) = expr {
        assert_eq!(if_expr.elseif_clauses, Vec::<ElseIfExprClause>::new());
    }
}

#[test]
fn if_expr_with_elseif() {
    let result = parse_luau("local y = if a then b elseif c then d else e");
    assert_no_errors(&result);
    let expr = local_value(&result);
    if let Expression::IfExpression(if_expr) = expr {
        assert_eq!(if_expr.elseif_clauses.len(), 1);
    } else {
        panic!("expected IfExpression");
    }
}

#[test]
fn if_expr_nested() {
    let result = parse_luau("local z = if a then if b then 1 else 2 else 3");
    assert_no_errors(&result);
    let expr = local_value(&result);
    if let Expression::IfExpression(if_expr) = expr {
        assert!(matches!(&if_expr.then_expr, Expression::IfExpression(_)));
    } else {
        panic!("expected IfExpression");
    }
}

#[test]
fn interpolated_string_plain() {
    let result = parse_luau("local x = `hello`");
    assert_no_errors(&result);
    let expr = local_value(&result);
    assert!(matches!(expr, Expression::InterpolatedString(_)));
}

#[test]
fn interpolated_string_with_expr() {
    let result = parse_luau("local x = `{y}`");
    assert_no_errors(&result);
    let expr = local_value(&result);
    if let Expression::InterpolatedString(interpolated_string) = expr {
        // InterpolatedStringBegin (with expr y) + InterpolatedStringEnd
        assert_eq!(interpolated_string.segments.len(), 2);
        assert!(interpolated_string.segments[0].expr.is_some());
        assert!(interpolated_string.segments[1].expr.is_none());
    } else {
        panic!("expected InterpolatedString");
    }
}

#[test]
fn interpolated_string_multiple_exprs() {
    let result = parse_luau("local x = `a{1+2}b{3}c`");
    assert_no_errors(&result);
    let expr = local_value(&result);
    if let Expression::InterpolatedString(interpolated_string) = expr {
        // InterpolatedStringBegin("a") + expr(1+2) -> InterpolatedStringMiddle("b") + expr(3)
        // -> InterpolatedStringEnd("c")
        assert_eq!(interpolated_string.segments.len(), 3);
        assert!(interpolated_string.segments[0].expr.is_some()); // 1+2
        assert!(interpolated_string.segments[1].expr.is_some()); // 3
        assert!(interpolated_string.segments[2].expr.is_none()); // end
    } else {
        panic!("expected InterpolatedString");
    }
}

#[test]
fn function_with_param_types() {
    let result = parse_luau("function f(a: string, b: number): boolean return true end");
    assert_no_errors(&result);
    assert_eq!(result.block.stmts.len(), 1);
    if let Statement::FunctionDecl(function_decl) = &result.block.stmts[0] {
        let param_count = function_decl.body.params.len();
        assert_eq!(param_count, 2);
        assert!(function_decl.body.return_type.is_some());
    } else {
        panic!("expected FunctionDecl");
    }
}

#[test]
fn local_with_generic_type() {
    let result = parse_luau("local t: Array<number> = {}");
    assert_no_errors(&result);
    assert!(matches!(&result.block.stmts[0], Statement::LocalAssignment(_)));
}

#[test]
fn type_cast_expr() {
    let result = parse_luau("local x = y :: number");
    assert_no_errors(&result);
    let expr = local_value(&result);
    assert!(matches!(expr, Expression::TypeCast(_)));
}

/// Luau's grammar is `asexp ::= simpleexp ['::' Type]`, so literals, tables,
/// and call results take a cast too. The minifier emits forms like
/// `print(1::number)`, which have to parse back.
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

#[test]
fn type_cast_binds_tighter_than_binary_op() {
    let result = parse_luau("local x = a + b :: number");
    assert_no_errors(&result);
    let operands = run_operands(local_value(&result));
    assert!(
        matches!(operands[1], Expression::TypeCast(_)),
        "cast must bind to the right operand: a + (b :: number)"
    );
}

#[test]
fn type_cast_chained() {
    // Grammar: asexp ::= simpleexp ['::' Type], one cast per simpleexp.
    // Luau rejects a second `::`; chaining needs parens.
    let bare = parse_luau("local x = v :: A :: B");
    assert!(!bare.errors.is_empty(), "chained casts must error");
    assert_no_errors(&parse_luau("local x = (v :: A) :: B"));
}

#[test]
fn type_declaration_simple() {
    let result = parse_luau("type Foo = number");
    assert_no_errors(&result);
    assert_eq!(result.block.stmts.len(), 1);
    if let Statement::TypeDeclaration(type_declaration) = &result.block.stmts[0] {
        assert_eq!(type_declaration.name.kind, luck_token::TokenKind::Identifier("Foo".into()));
    } else {
        panic!("expected TypeDeclaration, got {:?}", result.block.stmts[0]);
    }
}

#[test]
fn type_declaration_generic() {
    let result = parse_luau("type Pair<T> = { first: T, second: T }");
    assert_no_errors(&result);
    if let Statement::TypeDeclaration(type_declaration) = &result.block.stmts[0] {
        assert!(type_declaration.generics.is_some());
    } else {
        panic!("expected TypeDeclaration");
    }
}

#[test]
fn export_type_declaration() {
    let result = parse_luau("export type Point = { x: number, y: number }");
    assert_no_errors(&result);
    let Statement::TypeDeclaration(type_declaration) = &result.block.stmts[0] else {
        panic!("expected TypeDeclaration");
    };
    assert!(type_declaration.is_exported);
}

#[test]
fn consecutive_type_declarations() {
    let source = "type A = number\ntype B = string";
    let result = parse_luau(source);
    assert_no_errors(&result);
    assert_eq!(result.block.stmts.len(), 2);
}

#[test]
fn type_function_declaration_keeps_its_body() {
    let result = parse_luau("type function Id(t)\n\treturn t\nend");
    assert_no_errors(&result);
    let Statement::TypeDeclaration(type_declaration) = &result.block.stmts[0] else {
        panic!("expected TypeDeclaration");
    };
    assert!(matches!(type_declaration.type_value, TypeDeclarationValue::TypeFunction(_)));
}

#[test]
fn continue_in_for_loop() {
    let result = parse_luau("for i=1,10 do continue end");
    assert_no_errors(&result);
    if let Statement::NumericFor(numeric_for) = &result.block.stmts[0] {
        assert!(matches!(numeric_for.block.last_stmt.as_deref(), Some(LastStatement::Continue(_))));
    } else {
        panic!("expected NumericFor");
    }
}

#[test]
fn continue_as_local_name() {
    // `continue` used as a variable name parses as a local assignment.
    let result = parse_luau("local continue = 5");
    assert_no_errors(&result);
    assert!(matches!(&result.block.stmts[0], Statement::LocalAssignment(_)));
}

#[test]
fn continue_in_while_loop() {
    let result = parse_luau("while true do continue end");
    assert_no_errors(&result);
    if let Statement::WhileLoop(while_loop) = &result.block.stmts[0] {
        assert!(matches!(while_loop.block.last_stmt.as_deref(), Some(LastStatement::Continue(_))));
    } else {
        panic!("expected WhileLoop");
    }
}

#[test]
fn if_at_stmt_start_is_an_if_stmt() {
    let result = parse_luau("if true then x = 1 end");
    assert_no_errors(&result);
    assert!(matches!(&result.block.stmts[0], Statement::IfStatement(_)));
}

#[test]
fn break_is_a_last_stmt_in_luau() {
    // Luau keeps 5.1's `laststat` grammar (extended with `continue`):
    // break is a last statement, not a free-standing one.
    let result = parse_luau("while true do break end");
    assert_no_errors(&result);
    if let Statement::WhileLoop(while_loop) = &result.block.stmts[0] {
        assert_eq!(while_loop.block.stmts, Vec::<Statement>::new());
        assert!(matches!(while_loop.block.last_stmt.as_deref(), Some(LastStatement::Break(_))));
    } else {
        panic!("expected WhileLoop");
    }
}

#[test]
fn break_mid_block_rejected_in_luau() {
    // Statements after `break` are a grammar error in Luau.
    let result = parse_luau("while true do break print(1) end");
    assert_ne!(result.errors, Vec::<luck_token::SourceError>::new());
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
fn nested_interpolated_string_stmt_is_an_error() {
    // A bare string isn't a statement, so this has to report errors, and the
    // nesting mustn't make the parser panic.
    let source = "`outer{`inner{x}`}`";
    let result = parse_luau(source);
    assert!(
        !result.errors.is_empty(),
        "an interpolated string isn't a statement; expected parse errors"
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
    let global = parse_luau("@native function f() end");
    assert_no_errors(&global);
    if let Statement::FunctionDecl(function_decl) = &global.block.stmts[0] {
        assert_eq!(function_decl.attributes.len(), 1);
    } else {
        panic!("expected FunctionDecl");
    }

    let local = parse_luau("@native @checked local function g() end");
    assert_no_errors(&local);
    if let Statement::LocalFunction(local_function) = &local.block.stmts[0] {
        assert_eq!(local_function.attributes.len(), 2);
    } else {
        panic!("expected LocalFunction");
    }
}

#[test]
fn attribute_on_local_variable_rejected() {
    // Luau only allows attributes before function declarations.
    let result = parse_luau("@native local x = 1");
    assert_ne!(result.errors, Vec::<luck_token::SourceError>::new());
}

#[test]
fn export_type_function_parses() {
    let result = parse_luau("export type function Id(t) return t end");
    assert_no_errors(&result);
    if let Statement::TypeDeclaration(type_declaration) = &result.block.stmts[0] {
        assert!(type_declaration.is_exported);
        assert!(matches!(type_declaration.type_value, TypeDeclarationValue::TypeFunction(_)));
    } else {
        panic!("expected TypeDeclaration");
    }
}

#[test]
fn bitwise_operators_rejected() {
    // Luau has no bitwise operators; `&` and `|` only exist in type syntax.
    for source in ["local a = 1 & 2", "local a = 1 | 2", "return x & y", "return x | y"] {
        let result = parse_luau(source);
        assert!(!result.errors.is_empty(), "Luau must reject bitwise binop: {source}");
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

    assert_no_errors(&parse_luau("const a, b = 1, 2"));

    assert_no_errors(&parse_luau("const n: number = 5"));

    let function = parse_luau("const function cf() end");
    assert_no_errors(&function);
    if let Statement::LocalFunction(local_function) = &function.block.stmts[0] {
        assert_eq!(local_function.kind, LocalFunctionKind::Const);
    } else {
        panic!("expected LocalFunction, got {:?}", function.block.stmts[0]);
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
    assert_no_errors(&parse_luau("@[deprecated({use = \"g\"})] function f() end"));
    assert_no_errors(&parse_luau("@[checked, native] function f() end"));
    // Arguments must be literals, and only deprecated takes them.
    assert_ne!(
        parse_luau("@[deprecated(foo())] function f() end").errors,
        Vec::<luck_token::SourceError>::new()
    );
    assert_ne!(
        parse_luau("@[native(1)] function f() end").errors,
        Vec::<luck_token::SourceError>::new()
    );
}

#[test]
fn attributes_on_function_exprs() {
    // simpleexp ::= attributes 'function' funcbody
    assert_no_errors(&parse_luau("local k = @native function() end"));
    assert_no_errors(&parse_luau("f(@checked function() end)"));
    assert_ne!(
        parse_luau("local k = @nonsense function() end").errors,
        Vec::<luck_token::SourceError>::new()
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

#[test]
fn an_unterminated_interpolated_string_is_one_error() {
    let result = parse_luau("local s = `{x}a\nlocal after = 1");
    let [error] = result.errors.as_slice() else {
        panic!("{:?}", result.errors);
    };
    assert_eq!(error.message, "unterminated interpolated string");
    let [Statement::LocalAssignment(_), Statement::LocalAssignment(after)] =
        result.block.stmts.as_slice()
    else {
        panic!("{:?}", result.block.stmts);
    };
    assert_eq!(after.names.first().and_then(|name| name.name.identifier()), Some("after"));
}

#[test]
fn integer_literals_have_a_distinct_ast_node() {
    let result = parse_luau(
        "return 1i, 1_000_000i, 1i_, 0xffffffffffffffffi, \
         0b1000000000000000000000000000000000000000000000000000000000000000i",
    );
    assert_no_errors(&result);
    let Some(last) = result.block.last_stmt.as_deref() else {
        panic!("expected return");
    };
    let LastStatement::Return(return_stmt) = last else {
        panic!("expected return");
    };
    assert!(return_stmt.exprs.iter().all(|expr| matches!(expr, Expression::Integer(_))));

    for source in [
        "return 9223372036854775808i",
        "return 0x10000000000000000i",
        "return 123ii",
        "return 123.0i",
        "return 0b250i",
    ] {
        assert!(!parse_luau(source).errors.is_empty(), "{source} must be rejected");
    }

    // The suffix binds to the literal, not the unary minus, so the
    // decimal boundary value overflows even when negated; i64::MIN is
    // only spellable as a hex bit pattern.
    assert_no_errors(&parse_luau("return -123i"));
    assert_no_errors(&parse_luau("return 0x8000000000000000i"));
    assert!(
        !parse_luau("return -9223372036854775808i").errors.is_empty(),
        "decimal i64::MIN spelling must overflow"
    );
}

#[test]
fn explicit_type_instantiation_is_a_suffix() {
    let result = parse_luau("return module.f<<number, string>>(value)");
    assert_no_errors(&result);
    let Some(LastStatement::Return(return_stmt)) = result.block.last_stmt.as_deref() else {
        panic!("expected return");
    };
    let call = chain(return_stmt.exprs.first().expect("a returned value"));
    assert_eq!(chain_shape(call), "module.f<<>>()");
    let Some(Suffix::TypeInstantiation { type_args, .. }) = call.suffixes.get(1) else {
        panic!("expected explicit type instantiation");
    };
    assert_eq!(type_args.args.len(), 2);

    for source in [
        "return f<<number>>",
        "return t.f<<number>>()",
        "return t:f<<number>>()",
        "return t[\"f\"]<<number>>()",
    ] {
        assert_no_errors(&parse_luau(source));
    }

    let method = parse_luau("return t:f<<number, string>>()");
    let Some(LastStatement::Return(method_return)) = method.block.last_stmt.as_deref() else {
        panic!("expected return");
    };
    let method_chain = chain(method_return.exprs.first().expect("a returned value"));
    let Suffix::Call(method_call) = method_chain.suffixes.last() else {
        panic!("expected method call");
    };
    assert_eq!(
        method_call.explicit_type_args.as_ref().expect("method type arguments").args.len(),
        2
    );
}

#[test]
fn value_exports_parse_and_enforce_module_rules() {
    let result = parse_luau(
        "export local mutable: number = 1\n\
         export const FIXED = 2\n\
         @native\nexport function run() return mutable end",
    );
    assert_no_errors(&result);
    let Statement::LocalAssignment(local) = &result.block.stmts[0] else {
        panic!("expected exported local");
    };
    assert!(local.is_exported);
    assert!(!local.is_const);
    let Statement::LocalAssignment(constant) = &result.block.stmts[1] else {
        panic!("expected exported const");
    };
    assert!(constant.is_exported);
    assert!(constant.is_const);
    let Statement::LocalFunction(function) = &result.block.stmts[2] else {
        panic!("expected exported function");
    };
    assert_eq!(function.kind, LocalFunctionKind::Export);
    assert_eq!(function.attributes.len(), 1);

    for source in [
        "export local x = 1\nexport local x = 2",
        "if true then export local x = 1 end",
        "export local x = 1\nreturn x",
        "if true then return end\nexport local x = 1",
        "export local function f() end",
    ] {
        assert!(!parse_luau(source).errors.is_empty(), "{source} must be rejected");
    }

    // A type-only export still coexists with a module return.
    assert_no_errors(&parse_luau("export type T = number\nreturn 1"));

    // `export` remains contextual when it does not introduce a declaration.
    assert_no_errors(&parse_luau("export = 5\nexport += 1\nexport()\nreturn export"));
}

#[test]
fn luau_only_features_are_errors_in_lua54() {
    for source in ["return 1i", "return f<<number>>()", "export local x = 1", "type T = ~nil"] {
        let result = luck_parser::parse(source, luck_token::LuaVersion::Lua54);
        assert!(!result.errors.is_empty(), "{source} must be Luau-only");
    }
}

#[test]
fn attribute_is_accepted_on_const_function() {
    let result = parse_luau("@native const function f() end");
    assert_no_errors(&result);
}

#[test]
fn deprecated_arguments_are_validated() {
    for source in [
        "@[deprecated(\"x\")] function f() end",
        "@[deprecated(1, 2)] function f() end",
        "@[deprecated {foo = \"x\"}] function f() end",
        "@[deprecated {use = 1}] function f() end",
        "@[deprecated {\"x\"}] function f() end",
    ] {
        let result = parse_luau(source);
        assert!(!result.errors.is_empty(), "luau rejects the arguments in {source}");
    }
}

#[test]
fn attribute_name_must_directly_follow_the_at_sign() {
    for source in
        ["@ native function f() end", "@ [native] function f() end", "@\nnative function f() end"]
    {
        let result = parse_luau(source);
        assert!(!result.errors.is_empty(), "luau reports a missing attribute name in {source:?}");
    }
}

#[test]
fn const_needs_one_value_per_name_unless_the_last_is_multi_valued() {
    for source in
        ["const x, y = 1", "const x, y = (f())", "const x = 1, 2", "export const x, y = 1"]
    {
        let result = parse_luau(source);
        assert!(!result.errors.is_empty(), "luau reports a missing initializer in {source}");
    }
}

#[test]
fn type_function_cannot_reference_an_outer_local() {
    for source in [
        "local x = 1\ntype function f() return x end",
        "local x = 1\ntype function f() local function g() return x end return g end",
        "local function g() end\ntype function f() return g() end",
    ] {
        let result = parse_luau(source);
        assert!(!result.errors.is_empty(), "luau rejects the outer local in {source}");
    }
}

#[test]
fn simple_interpolated_string_is_an_attribute_literal() {
    // Luau lexes a backtick string with no holes as a constant string.
    let result = parse_luau("@[deprecated {use = `x`}] function f() end");
    assert_no_errors(&result);
}
