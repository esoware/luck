use luck_allocator::Allocator;
use luck_ast::{Expression, Statement};
use luck_parser::parse;

use crate::common::{assert_has_errors, assert_no_errors, local_value, parse_str};

#[test]
fn function_statement_is_a_function_declaration() {
    let allocator = Allocator::new();
    let result = parse_str(&allocator, "function foo() end");
    assert_no_errors(&result);
    assert!(matches!(&result.chunk.stmts[0], Statement::FunctionDecl(_)));
}

#[test]
fn method_declaration_keeps_its_path_and_method_name() {
    let allocator = Allocator::new();
    let result = parse_str(&allocator, "function a.b:c() end");
    assert_no_errors(&result);
    if let Statement::FunctionDecl(function_decl) = &result.chunk.stmts[0] {
        assert_eq!(function_decl.name.names.len(), 2); // a, b
        assert!(function_decl.name.method.is_some()); // :c
    } else {
        panic!("expected FunctionDecl");
    }
}

#[test]
fn local_function_keeps_its_params() {
    let allocator = Allocator::new();
    let result = parse_str(&allocator, "local function foo(x, y) return x end");
    assert_no_errors(&result);
    if let Statement::LocalFunction(local_function) = &result.chunk.stmts[0] {
        let param_count = local_function.body.params.len();
        assert_eq!(param_count, 2);
    } else {
        panic!("expected LocalFunction");
    }
}

#[test]
fn vararg_function_keeps_its_vararg() {
    let allocator = Allocator::new();
    let result = parse_str(&allocator, "function f(...) return ... end");
    assert_no_errors(&result);
    if let Statement::FunctionDecl(function_decl) = &result.chunk.stmts[0] {
        assert!(function_decl.body.vararg.is_some());
    } else {
        panic!("expected FunctionDecl");
    }
}

#[test]
fn vararg_follows_named_params() {
    let allocator = Allocator::new();
    let result = parse_str(&allocator, "function f(a, b, ...) end");
    assert_no_errors(&result);
    if let Statement::FunctionDecl(function_decl) = &result.chunk.stmts[0] {
        assert!(function_decl.body.vararg.is_some());
        let param_count = function_decl.body.params.len();
        assert_eq!(param_count, 2);
    } else {
        panic!("expected FunctionDecl");
    }
}

#[test]
fn function_expression_keeps_its_body() {
    let allocator = Allocator::new();
    let result = parse_str(&allocator, "local x = function() return 1 + 2 * 3 end");
    assert_no_errors(&result);
    let expr = local_value(&result);
    if let Expression::FunctionDef(function_def) = expr {
        assert!(matches!(*function_def.body.block.stmts, [Statement::Return(_)]));
    } else {
        panic!("expected FunctionDef");
    }
}

#[test]
fn function_declarations_keep_their_attributes() {
    let allocator = Allocator::new();
    let global = parse_str(&allocator, "@native function f() end");
    assert_no_errors(&global);
    if let Statement::FunctionDecl(function_decl) = &global.chunk.stmts[0] {
        assert_eq!(function_decl.attributes.len(), 1);
    } else {
        panic!("expected FunctionDecl");
    }

    let local = parse_str(&allocator, "@native @checked local function g() end");
    assert_no_errors(&local);
    if let Statement::LocalFunction(local_function) = &local.chunk.stmts[0] {
        assert_eq!(local_function.attributes.len(), 2);
    } else {
        panic!("expected LocalFunction");
    }
}

#[test]
fn attribute_on_local_variable_rejected() {
    let allocator = Allocator::new();
    // Luau only allows attributes before function declarations.
    let result = parse_str(&allocator, "@native local x = 1");
    assert_ne!(result.errors, Vec::<luck_diagnostics::Diagnostic>::new());
}

#[test]
fn bracketed_attributes_take_literal_arguments() {
    let allocator = Allocator::new();
    // attribute ::= '@' NAME | '@[' parattr {',' parattr} ']'
    assert_no_errors(&parse_str(&allocator, "@[deprecated] function f() end"));
    assert_no_errors(&parse_str(&allocator, "@[deprecated({use = \"g\"})] function f() end"));
    assert_no_errors(&parse_str(&allocator, "@[checked, native] function f() end"));
    // Arguments must be literals, and only deprecated takes them.
    assert_ne!(
        parse_str(&allocator, "@[deprecated(foo())] function f() end").errors,
        Vec::<luck_diagnostics::Diagnostic>::new()
    );
    assert_ne!(
        parse_str(&allocator, "@[native(1)] function f() end").errors,
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
        let errors = parse_str(&allocator, source).errors;
        assert!(
            errors.iter().any(|error| error.message == "attribute arguments must be literals"),
            "{source}: {errors:?}"
        );
    }
}

#[test]
fn function_expressions_take_attributes() {
    let allocator = Allocator::new();
    // simpleexp ::= attributes 'function' funcbody
    assert_no_errors(&parse_str(&allocator, "local k = @native function() end"));
    assert_no_errors(&parse_str(&allocator, "f(@checked function() end)"));
    assert_ne!(
        parse_str(&allocator, "local k = @nonsense function() end").errors,
        Vec::<luck_diagnostics::Diagnostic>::new()
    );
}

#[test]
fn attribute_is_accepted_on_const_function() {
    let allocator = Allocator::new();
    let result = parse_str(&allocator, "@native const function f() end");
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
        let result = parse_str(&allocator, source);
        assert_no_errors(&result);
        let Statement::LocalFunction(function) = &result.chunk.stmts[0] else {
            panic!("expected LocalFunction: {:?}", result.chunk.stmts);
        };
        let start = function.keyword_start as usize;
        assert!(source[start..].starts_with(keyword), "{source}");
    }
}

#[test]
fn deprecated_takes_only_a_table_of_use_and_reason_strings() {
    let allocator = Allocator::new();
    for source in [
        "@[deprecated(\"x\")] function f() end",
        "@[deprecated(1, 2)] function f() end",
        "@[deprecated {foo = \"x\"}] function f() end",
        "@[deprecated {use = 1}] function f() end",
        "@[deprecated {\"x\"}] function f() end",
    ] {
        let result = parse_str(&allocator, source);
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
        let result = parse_str(&allocator, source);
        assert!(!result.errors.is_empty(), "luau reports a missing attribute name in {source:?}");
    }
}

#[test]
fn simple_interpolated_string_is_an_attribute_literal() {
    let allocator = Allocator::new();
    // Luau lexes a backtick string with no holes as a constant string.
    let result = parse_str(&allocator, "@[deprecated {use = `x`}] function f() end");
    assert_no_errors(&result);
}

#[test]
fn a_vararg_takes_no_name() {
    let allocator = Allocator::new();
    let result = parse(&allocator, b"function f(...) return args end");
    assert_no_errors(&result);
    let Some(Statement::FunctionDecl(function_decl)) = result.chunk.stmts.first() else {
        panic!("expected FunctionDecl, got {:?}", result.chunk.stmts);
    };
    assert!(function_decl.body.vararg.is_some(), "expected vararg");
    assert_has_errors(&parse(&allocator, b"function f(...args) return args end"));
}

#[test]
fn vararg_outside_vararg_function_rejected() {
    let allocator = Allocator::new();
    // Main chunks are vararg.
    assert_no_errors(&parse(&allocator, b"return ..."));
    assert_no_errors(&parse(&allocator, b"local function f(...) return ... end"));
    assert_has_errors(&parse(&allocator, b"local function f() return ... end"));
    // Non-vararg function nested in a vararg one.
    assert_has_errors(&parse(
        &allocator,
        b"local function f(...) local g = function(x) return ... end end",
    ));
}

#[test]
fn function_attributes_must_be_known_and_unique() {
    let allocator = Allocator::new();
    assert_no_errors(&parse(&allocator, b"@native function f() end"));
    assert_no_errors(&parse(&allocator, b"@checked @native function f() end"));
    assert_has_errors(&parse(&allocator, b"@totallymadeup function f() end"));
    assert_has_errors(&parse(&allocator, b"@native @native function f() end"));
}
