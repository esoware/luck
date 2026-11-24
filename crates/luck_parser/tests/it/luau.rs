use luck_allocator::Allocator;
use luck_ast::{
    Expression, InterpolatedSegment, LocalAssignmentKind, LocalFunctionKind, Statement, Suffix,
    TypeDeclarationValue, Var,
};

use crate::common::{
    assert_no_errors, chain, chain_shape, local_value, parse_luau, run_operands, target_chain,
};

#[test]
fn compound_assign_plus() {
    let allocator = Allocator::new();
    let result = parse_luau(&allocator, "x += 1");
    assert_no_errors(&result);
    assert_eq!(result.chunk.block.stmts.len(), 1);
    if let Statement::CompoundAssignment(compound_assignment) = &result.chunk.block.stmts[0] {
        assert!(matches!(&compound_assignment.var, Var::Name(name) if name.text == "x"));
        assert_eq!(compound_assignment.op, luck_token::CompoundOp::Add);
    } else {
        panic!("expected CompoundAssignment, got {:?}", result.chunk.block.stmts[0]);
    }
}

#[test]
fn compound_assign_index() {
    let allocator = Allocator::new();
    let result = parse_luau(&allocator, "t[i] *= 2");
    assert_no_errors(&result);
    assert_eq!(result.chunk.block.stmts.len(), 1);
    let Statement::CompoundAssignment(compound_assignment) = &result.chunk.block.stmts[0] else {
        panic!("expected CompoundAssignment");
    };
    assert_eq!(chain_shape(target_chain(&compound_assignment.var)), "t[]");
    assert_eq!(compound_assignment.op, luck_token::CompoundOp::Mul);
}

#[test]
fn compound_assign_field_concat() {
    let allocator = Allocator::new();
    let result = parse_luau(&allocator, r#"a.b ..= "c""#);
    assert_no_errors(&result);
    assert_eq!(result.chunk.block.stmts.len(), 1);
    let Statement::CompoundAssignment(compound_assignment) = &result.chunk.block.stmts[0] else {
        panic!("expected CompoundAssignment");
    };
    assert_eq!(chain_shape(target_chain(&compound_assignment.var)), "a.b");
    assert_eq!(compound_assignment.op, luck_token::CompoundOp::Concat);
}

#[test]
fn compound_assign_all_operators() {
    let allocator = Allocator::new();
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
        let result = parse_luau(&allocator, source);
        assert_no_errors(&result);
        if let Statement::CompoundAssignment(compound_assignment) = &result.chunk.block.stmts[0] {
            assert_eq!(compound_assignment.op, expected_op, "failed for: {source}");
        } else {
            panic!("expected CompoundAssignment for: {source}");
        }
    }
}

#[test]
fn if_expr_simple() {
    let allocator = Allocator::new();
    let result = parse_luau(&allocator, "local x = if a then 1 else 2");
    assert_no_errors(&result);
    assert_eq!(result.chunk.block.stmts.len(), 1);
    let expr = local_value(&result);
    assert!(matches!(expr, Expression::IfExpression(_)));
    if let Expression::IfExpression(if_expr) = expr {
        assert!(if_expr.elseif_clauses.is_empty(), "{:?}", if_expr.elseif_clauses);
    }
}

#[test]
fn if_expr_with_elseif() {
    let allocator = Allocator::new();
    let result = parse_luau(&allocator, "local y = if a then b elseif c then d else e");
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
    let allocator = Allocator::new();
    let result = parse_luau(&allocator, "local z = if a then if b then 1 else 2 else 3");
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
    let allocator = Allocator::new();
    let result = parse_luau(&allocator, "local x = `hello`");
    assert_no_errors(&result);
    let expr = local_value(&result);
    assert!(matches!(expr, Expression::InterpolatedString(_)));
}

#[test]
fn interpolated_string_with_expr() {
    let allocator = Allocator::new();
    let result = parse_luau(&allocator, "local x = `{y}`");
    assert_no_errors(&result);
    let expr = local_value(&result);
    if let Expression::InterpolatedString(interpolated_string) = expr {
        assert_eq!(interpolated_string.segments.len(), 1);
        assert!(matches!(interpolated_string.segments[0].expr, Expression::Name(_)));
        assert!(interpolated_string.tail.value.is_empty());
    } else {
        panic!("expected InterpolatedString");
    }
}

#[test]
fn interpolated_string_multiple_exprs() {
    let allocator = Allocator::new();
    let result = parse_luau(&allocator, "local x = `a{1+2}b{3}c`");
    assert_no_errors(&result);
    let expr = local_value(&result);
    if let Expression::InterpolatedString(interpolated_string) = expr {
        let texts: Vec<&[u8]> = interpolated_string
            .segments
            .iter()
            .map(|segment| segment.text.value.as_bytes())
            .collect();
        assert_eq!(texts, [b"a", b"b"]);
        assert!(matches!(
            &*interpolated_string.segments,
            [
                InterpolatedSegment { expr: Expression::BinaryOp(_), .. },
                InterpolatedSegment { expr: Expression::Number(_), .. },
            ]
        ));
        assert_eq!(interpolated_string.tail.value, &b"c"[..]);
    } else {
        panic!("expected InterpolatedString");
    }
}

#[test]
fn function_with_param_types() {
    let allocator = Allocator::new();
    let result =
        parse_luau(&allocator, "function f(a: string, b: number): boolean return true end");
    assert_no_errors(&result);
    assert_eq!(result.chunk.block.stmts.len(), 1);
    if let Statement::FunctionDecl(function_decl) = &result.chunk.block.stmts[0] {
        let param_count = function_decl.body.params.len();
        assert_eq!(param_count, 2);
        assert!(function_decl.body.return_type.is_some());
    } else {
        panic!("expected FunctionDecl");
    }
}

#[test]
fn local_with_generic_type() {
    let allocator = Allocator::new();
    let result = parse_luau(&allocator, "local t: Array<number> = {}");
    assert_no_errors(&result);
    assert!(matches!(&result.chunk.block.stmts[0], Statement::LocalAssignment(_)));
}

#[test]
fn type_cast_expr() {
    let allocator = Allocator::new();
    let result = parse_luau(&allocator, "local x = y :: number");
    assert_no_errors(&result);
    let expr = local_value(&result);
    assert!(matches!(expr, Expression::TypeCast(_)));
}

/// Luau's grammar is `asexp ::= simpleexp ['::' Type]`, so literals, tables,
/// and call results take a cast too. The minifier emits forms like
/// `print(1::number)`, which have to parse back.
#[test]
fn type_cast_on_literals_and_tables() {
    let allocator = Allocator::new();
    for source in [
        "local x = 1 :: number",
        "local x = \"s\" :: string",
        "local x = {} :: Foo",
        "local x = true :: boolean",
        "print(1 :: number)",
        "local x = f() :: T",
    ] {
        let result = parse_luau(&allocator, source);
        assert_no_errors(&result);
    }
}

#[test]
fn type_cast_binds_tighter_than_binary_op() {
    let allocator = Allocator::new();
    let result = parse_luau(&allocator, "local x = a + b :: number");
    assert_no_errors(&result);
    let operands = run_operands(local_value(&result));
    assert!(
        matches!(operands[1], Expression::TypeCast(_)),
        "cast must bind to the right operand: a + (b :: number)"
    );
}

#[test]
fn type_cast_chained() {
    let allocator = Allocator::new();
    // Grammar: asexp ::= simpleexp ['::' Type], one cast per simpleexp.
    // Luau rejects a second `::`; chaining needs parens.
    let bare = parse_luau(&allocator, "local x = v :: A :: B");
    assert!(!bare.errors.is_empty(), "chained casts must error");
    assert_no_errors(&parse_luau(&allocator, "local x = (v :: A) :: B"));
}

#[test]
fn type_declaration_simple() {
    let allocator = Allocator::new();
    let result = parse_luau(&allocator, "type Foo = number");
    assert_no_errors(&result);
    assert_eq!(result.chunk.block.stmts.len(), 1);
    if let Statement::TypeDeclaration(type_declaration) = &result.chunk.block.stmts[0] {
        assert_eq!(type_declaration.name.text, "Foo");
    } else {
        panic!("expected TypeDeclaration, got {:?}", result.chunk.block.stmts[0]);
    }
}

#[test]
fn type_declaration_generic() {
    let allocator = Allocator::new();
    let result = parse_luau(&allocator, "type Pair<T> = { first: T, second: T }");
    assert_no_errors(&result);
    if let Statement::TypeDeclaration(type_declaration) = &result.chunk.block.stmts[0] {
        assert!(type_declaration.generics.is_some());
    } else {
        panic!("expected TypeDeclaration");
    }
}

#[test]
fn export_type_declaration() {
    let allocator = Allocator::new();
    let result = parse_luau(&allocator, "export type Point = { x: number, y: number }");
    assert_no_errors(&result);
    let Statement::TypeDeclaration(type_declaration) = &result.chunk.block.stmts[0] else {
        panic!("expected TypeDeclaration");
    };
    assert!(type_declaration.is_exported);
}

#[test]
fn consecutive_type_declarations() {
    let allocator = Allocator::new();
    let source = "\
type A = number
type B = string";
    let result = parse_luau(&allocator, source);
    assert_no_errors(&result);
    assert_eq!(result.chunk.block.stmts.len(), 2);
}

#[test]
fn type_function_declaration_keeps_its_body() {
    let allocator = Allocator::new();
    let result = parse_luau(
        &allocator,
        "\
type function Id(t)
\treturn t
end",
    );
    assert_no_errors(&result);
    let Statement::TypeDeclaration(type_declaration) = &result.chunk.block.stmts[0] else {
        panic!("expected TypeDeclaration");
    };
    assert!(matches!(type_declaration.value, TypeDeclarationValue::TypeFunction(_)));
}

#[test]
fn continue_in_for_loop() {
    let allocator = Allocator::new();
    let result = parse_luau(&allocator, "for i=1,10 do continue end");
    assert_no_errors(&result);
    if let Statement::NumericFor(numeric_for) = &result.chunk.block.stmts[0] {
        assert!(matches!(*numeric_for.block.stmts, [Statement::Continue(_)]));
    } else {
        panic!("expected NumericFor");
    }
}

#[test]
fn continue_as_local_name() {
    let allocator = Allocator::new();
    // `continue` used as a variable name parses as a local assignment.
    let result = parse_luau(&allocator, "local continue = 5");
    assert_no_errors(&result);
    assert!(matches!(&result.chunk.block.stmts[0], Statement::LocalAssignment(_)));
}

#[test]
fn continue_in_while_loop() {
    let allocator = Allocator::new();
    let result = parse_luau(&allocator, "while true do continue end");
    assert_no_errors(&result);
    if let Statement::WhileLoop(while_loop) = &result.chunk.block.stmts[0] {
        assert!(matches!(*while_loop.block.stmts, [Statement::Continue(_)]));
    } else {
        panic!("expected WhileLoop");
    }
}

#[test]
fn if_at_stmt_start_is_an_if_stmt() {
    let allocator = Allocator::new();
    let result = parse_luau(&allocator, "if true then x = 1 end");
    assert_no_errors(&result);
    assert!(matches!(&result.chunk.block.stmts[0], Statement::IfStatement(_)));
}

#[test]
fn break_ends_its_block_in_luau_as_a_statement() {
    let allocator = Allocator::new();
    // Luau keeps 5.1's `laststat` grammar, extended with `continue`, and
    // its `break` is a statement in the tree, as every dialect's is.
    let result = parse_luau(&allocator, "while true do break end");
    assert_no_errors(&result);
    if let Statement::WhileLoop(while_loop) = &result.chunk.block.stmts[0] {
        assert!(matches!(*while_loop.block.stmts, [Statement::Break(_)]));
    } else {
        panic!("expected WhileLoop");
    }
}

#[test]
fn break_mid_block_rejected_in_luau() {
    let allocator = Allocator::new();
    // Statements after `break` are a grammar error in Luau.
    let result = parse_luau(&allocator, "while true do break print(1) end");
    assert_ne!(result.errors, Vec::<luck_diagnostics::Diagnostic>::new());
}

#[test]
fn continue_as_function_call() {
    let allocator = Allocator::new();
    let source = "continue()";
    let result = parse_luau(&allocator, source);
    assert_no_errors(&result);
    assert!(
        matches!(&result.chunk.block.stmts[0], Statement::FunctionCall(_)),
        "expected FunctionCall, got {:?}",
        result.chunk.block.stmts[0]
    );
}

#[test]
fn continue_as_field_access() {
    let allocator = Allocator::new();
    let source = "continue.field = 1";
    let result = parse_luau(&allocator, source);
    assert_no_errors(&result);
    assert!(
        matches!(&result.chunk.block.stmts[0], Statement::Assignment(_)),
        "expected Assignment via field access, got {:?}",
        result.chunk.block.stmts[0]
    );
}

#[test]
fn nested_interpolated_string_stmt_is_an_error() {
    let allocator = Allocator::new();
    // A bare string isn't a statement, so this has to report errors, and the
    // nesting mustn't make the parser panic.
    let source = "`outer{`inner{x}`}`";
    let result = parse_luau(&allocator, source);
    assert!(
        !result.errors.is_empty(),
        "an interpolated string isn't a statement; expected parse errors"
    );
}

#[test]
fn nested_generic_type_annotation() {
    let allocator = Allocator::new();
    let source = "local x: Map<string, Array<number>> = {}";
    let result = parse_luau(&allocator, source);
    assert_no_errors(&result);
}

#[test]
fn function_attributes_are_kept_on_ast() {
    let allocator = Allocator::new();
    let global = parse_luau(&allocator, "@native function f() end");
    assert_no_errors(&global);
    if let Statement::FunctionDecl(function_decl) = &global.chunk.block.stmts[0] {
        assert_eq!(function_decl.attributes.len(), 1);
    } else {
        panic!("expected FunctionDecl");
    }

    let local = parse_luau(&allocator, "@native @checked local function g() end");
    assert_no_errors(&local);
    if let Statement::LocalFunction(local_function) = &local.chunk.block.stmts[0] {
        assert_eq!(local_function.attributes.len(), 2);
    } else {
        panic!("expected LocalFunction");
    }
}

#[test]
fn attribute_on_local_variable_rejected() {
    let allocator = Allocator::new();
    // Luau only allows attributes before function declarations.
    let result = parse_luau(&allocator, "@native local x = 1");
    assert_ne!(result.errors, Vec::<luck_diagnostics::Diagnostic>::new());
}

#[test]
fn export_type_function_parses() {
    let allocator = Allocator::new();
    let result = parse_luau(&allocator, "export type function Id(t) return t end");
    assert_no_errors(&result);
    if let Statement::TypeDeclaration(type_declaration) = &result.chunk.block.stmts[0] {
        assert!(type_declaration.is_exported);
        assert!(matches!(type_declaration.value, TypeDeclarationValue::TypeFunction(_)));
    } else {
        panic!("expected TypeDeclaration");
    }
}

#[test]
fn bitwise_operators_rejected() {
    let allocator = Allocator::new();
    // Luau has no bitwise operators; `&` and `|` only exist in type syntax.
    for source in ["local a = 1 & 2", "local a = 1 | 2", "return x & y", "return x | y"] {
        let result = parse_luau(&allocator, source);
        assert!(!result.errors.is_empty(), "Luau must reject bitwise binop: {source}");
    }
    // The same tokens still work in type positions.
    let result = parse_luau(
        &allocator,
        "\
type U = number | string
type I = A & B",
    );
    assert!(result.errors.is_empty(), "{:?}", result.errors);
}

#[test]
fn const_bindings() {
    let allocator = Allocator::new();
    // Grammar: 'const' bindinglist '=' explist | 'const' 'function' NAME funcbody
    let result = parse_luau(&allocator, "const x = 1");
    assert_no_errors(&result);
    if let Statement::LocalAssignment(local) = &result.chunk.block.stmts[0] {
        assert_eq!(local.kind, LocalAssignmentKind::Const);
        assert!(!local.exprs.is_empty());
    } else {
        panic!("expected LocalAssignment, got {:?}", result.chunk.block.stmts[0]);
    }

    assert_no_errors(&parse_luau(&allocator, "const a, b = 1, 2"));

    assert_no_errors(&parse_luau(&allocator, "const n: number = 5"));

    let function = parse_luau(&allocator, "const function cf() end");
    assert_no_errors(&function);
    if let Statement::LocalFunction(local_function) = &function.chunk.block.stmts[0] {
        assert_eq!(local_function.kind, LocalFunctionKind::Const);
    } else {
        panic!("expected LocalFunction, got {:?}", function.chunk.block.stmts[0]);
    }
}

#[test]
fn const_requires_initializer() {
    let allocator = Allocator::new();
    let result = parse_luau(&allocator, "const x");
    assert!(!result.errors.is_empty(), "missing initializer must error");
}

#[test]
fn const_stays_contextual() {
    let allocator = Allocator::new();
    // `const` remains an ordinary identifier when no binding follows.
    assert_no_errors(&parse_luau(&allocator, "const = 1"));
    assert_no_errors(&parse_luau(&allocator, "const.x = 1"));
    assert_no_errors(&parse_luau(&allocator, "const()"));
    assert_no_errors(&parse_luau(
        &allocator,
        "\
local const = 2
print(const)",
    ));
    // And non-Luau versions never treat it specially.
    let result = luck_parser::parse(&allocator, b"const x = 1", luck_token::LuaVersion::Lua54);
    assert!(!result.errors.is_empty(), "const decl is Luau-only");
}

#[test]
fn bracketed_and_parameterized_attributes() {
    let allocator = Allocator::new();
    // attribute ::= '@' NAME | '@[' parattr {',' parattr} ']'
    assert_no_errors(&parse_luau(&allocator, "@[deprecated] function f() end"));
    assert_no_errors(&parse_luau(&allocator, "@[deprecated({use = \"g\"})] function f() end"));
    assert_no_errors(&parse_luau(&allocator, "@[checked, native] function f() end"));
    // Arguments must be literals, and only deprecated takes them.
    assert_ne!(
        parse_luau(&allocator, "@[deprecated(foo())] function f() end").errors,
        Vec::<luck_diagnostics::Diagnostic>::new()
    );
    assert_ne!(
        parse_luau(&allocator, "@[native(1)] function f() end").errors,
        Vec::<luck_diagnostics::Diagnostic>::new()
    );
}

#[test]
fn a_literal_in_parentheses_is_no_attribute_argument() {
    let allocator = Allocator::new();
    for source in [
        "@[deprecated(({use = \"g\"}))] function f() end",
        "@[deprecated({use = (\"g\")})] function f() end",
    ] {
        let errors = parse_luau(&allocator, source).errors;
        assert!(
            errors.iter().any(|error| error.message == "attribute arguments must be literals"),
            "{source}: {errors:?}"
        );
    }
}

#[test]
fn attributes_on_function_exprs() {
    let allocator = Allocator::new();
    // simpleexp ::= attributes 'function' funcbody
    assert_no_errors(&parse_luau(&allocator, "local k = @native function() end"));
    assert_no_errors(&parse_luau(&allocator, "f(@checked function() end)"));
    assert_ne!(
        parse_luau(&allocator, "local k = @nonsense function() end").errors,
        Vec::<luck_diagnostics::Diagnostic>::new()
    );
}

#[test]
fn empty_interpolation_rejected() {
    let allocator = Allocator::new();
    for (source, hole) in [
        ("local s = `a{}b`", "{}"),
        ("local s = `{x}a{}b`", "{}"),
        ("local s = `{}a{x}b`", "{}"),
        ("local s = `{x}a{ --[[c]] }b`", "{ --[[c]] }"),
    ] {
        let result = parse_luau(&allocator, source);
        let [error] = result.errors.as_slice() else {
            panic!("{source}: {:?}", result.errors);
        };
        assert_eq!(
            (&*error.message, error.span.unwrap().text(source.as_bytes())),
            ("malformed interpolated string: expected expression inside `{}`", hole.as_bytes()),
            "{source}"
        );
    }
    // Plain backtick strings still parse.
    assert_no_errors(&parse_luau(&allocator, "return `plain`"));
    assert_no_errors(&parse_luau(&allocator, "return ``"));
    assert_no_errors(&parse_luau(&allocator, "return `a{1}b`"));
    let unterminated = parse_luau(&allocator, "local s = `plain");
    assert_eq!(
        unterminated.errors.iter().map(|e| &*e.message).collect::<Vec<_>>(),
        ["unterminated interpolated string"]
    );
}

#[test]
fn an_unterminated_interpolated_string_is_one_error() {
    let allocator = Allocator::new();
    let result = parse_luau(
        &allocator,
        "\
local s = `{x}a
local after = 1",
    );
    let [error] = result.errors.as_slice() else {
        panic!("{:?}", result.errors);
    };
    assert_eq!(error.message, "unterminated interpolated string");
    let [Statement::LocalAssignment(_), Statement::LocalAssignment(after)] =
        &*result.chunk.block.stmts
    else {
        panic!("{:?}", result.chunk.block.stmts);
    };
    assert_eq!(after.names.first().map(|name| name.name.text.as_str()), Some("after"));
}

#[test]
fn integer_literals_have_a_distinct_ast_node() {
    let allocator = Allocator::new();
    let result = parse_luau(
        &allocator,
        "return 1i, 1_000_000i, 1i_, 0xffffffffffffffffi, \
         0b1000000000000000000000000000000000000000000000000000000000000000i",
    );
    assert_no_errors(&result);
    let [Statement::Return(return_stmt)] = &*result.chunk.block.stmts else {
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
        assert!(!parse_luau(&allocator, source).errors.is_empty(), "{source} must be rejected");
    }

    // The suffix binds to the literal, not the unary minus, so the
    // decimal boundary value overflows even when negated; i64::MIN is
    // only spellable as a hex bit pattern.
    assert_no_errors(&parse_luau(&allocator, "return -123i"));
    assert_no_errors(&parse_luau(&allocator, "return 0x8000000000000000i"));
    assert!(
        !parse_luau(&allocator, "return -9223372036854775808i").errors.is_empty(),
        "decimal i64::MIN spelling must overflow"
    );
}

#[test]
fn explicit_type_instantiation_is_a_suffix() {
    let allocator = Allocator::new();
    let result = parse_luau(&allocator, "return module.f<<number, string>>(value)");
    assert_no_errors(&result);
    let [Statement::Return(return_stmt)] = &*result.chunk.block.stmts else {
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
        assert_no_errors(&parse_luau(&allocator, source));
    }

    let method = parse_luau(&allocator, "return t:f<<number, string>>()");
    let [Statement::Return(method_return)] = &*method.chunk.block.stmts else {
        panic!("expected return");
    };
    let method_chain = chain(method_return.exprs.first().expect("a returned value"));
    let Suffix::MethodCall(method_call) = method_chain.suffixes.last() else {
        panic!("expected method call");
    };
    assert_eq!(method_call.type_args.as_ref().expect("method type arguments").args.len(), 2);
}

#[test]
fn value_exports_parse_and_enforce_module_rules() {
    let allocator = Allocator::new();
    let result = parse_luau(
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
    ] = &*result.chunk.block.stmts
    else {
        panic!("expected an exported local, const, and function: {:?}", result.chunk.block.stmts);
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
        assert!(!parse_luau(&allocator, source).errors.is_empty(), "{source} must be rejected");
    }

    // A type-only export still coexists with a module return.
    assert_no_errors(&parse_luau(
        &allocator,
        "\
export type T = number
return 1",
    ));

    // `export` remains contextual when it does not introduce a declaration.
    assert_no_errors(&parse_luau(
        &allocator,
        "\
export = 5
export += 1
export()
return export",
    ));
}

#[test]
fn luau_only_features_are_errors_in_lua54() {
    let allocator = Allocator::new();
    for source in ["return 1i", "return f<<number>>()", "export local x = 1", "type T = ~nil"] {
        let result =
            luck_parser::parse(&allocator, source.as_bytes(), luck_token::LuaVersion::Lua54);
        assert!(!result.errors.is_empty(), "{source} must be Luau-only");
    }
}

#[test]
fn attribute_is_accepted_on_const_function() {
    let allocator = Allocator::new();
    let result = parse_luau(&allocator, "@native const function f() end");
    assert_no_errors(&result);
}

#[test]
fn a_local_function_keeps_where_its_keyword_starts_past_its_attributes() {
    let allocator = Allocator::new();
    for (source, keyword) in [
        ("local function f() end", "local"),
        ("@native --[[ export ]] const function f() end", "const"),
        ("@native @checked export function f() end", "export"),
    ] {
        let result = parse_luau(&allocator, source);
        assert_no_errors(&result);
        let Statement::LocalFunction(function) = &result.chunk.block.stmts[0] else {
            panic!("expected LocalFunction: {:?}", result.chunk.block.stmts);
        };
        let start = usize::try_from(function.keyword_start).unwrap();
        assert!(source.get(start..).is_some_and(|rest| rest.starts_with(keyword)), "{source}");
    }
}

#[test]
fn deprecated_arguments_are_validated() {
    let allocator = Allocator::new();
    for source in [
        "@[deprecated(\"x\")] function f() end",
        "@[deprecated(1, 2)] function f() end",
        "@[deprecated {foo = \"x\"}] function f() end",
        "@[deprecated {use = 1}] function f() end",
        "@[deprecated {\"x\"}] function f() end",
    ] {
        let result = parse_luau(&allocator, source);
        assert!(!result.errors.is_empty(), "luau rejects the arguments in {source}");
    }
}

#[test]
fn attribute_name_must_directly_follow_the_at_sign() {
    let allocator = Allocator::new();
    for source in [
        "@ native function f() end",
        "@ [native] function f() end",
        "\
@
native function f() end",
    ] {
        let result = parse_luau(&allocator, source);
        assert!(!result.errors.is_empty(), "luau reports a missing attribute name in {source:?}");
    }
}

#[test]
fn const_needs_one_value_per_name_unless_the_last_is_multi_valued() {
    let allocator = Allocator::new();
    for source in
        ["const x, y = 1", "const x, y = (f())", "const x = 1, 2", "export const x, y = 1"]
    {
        let result = parse_luau(&allocator, source);
        assert!(!result.errors.is_empty(), "luau reports a missing initializer in {source}");
    }
}

#[test]
fn simple_interpolated_string_is_an_attribute_literal() {
    let allocator = Allocator::new();
    // Luau lexes a backtick string with no holes as a constant string.
    let result = parse_luau(&allocator, "@[deprecated {use = `x`}] function f() end");
    assert_no_errors(&result);
}
