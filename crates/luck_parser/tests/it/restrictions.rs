use luck_allocator::Allocator;
use luck_ast::Statement;
use luck_parser::{ParseResult, parse};

use crate::common::assert_no_errors;

fn assert_has_errors(result: &ParseResult<'_>) {
    assert!(!result.errors.is_empty(), "expected parse errors but got none");
}

#[test]
fn goto_and_global_are_names() {
    let allocator = Allocator::new();
    for source in [
        "goto = 1",
        "global = 1",
        "local global = 1",
        "global.x = 1",
        "global()",
        "global \"x\"",
        "global {}",
        "function global() end",
        "return goto, global",
    ] {
        let result = parse(&allocator, source.as_bytes());
        assert_no_errors(&result);
        assert!(
            !matches!(result.chunk.block.stmts.first(), Some(Statement::Error(_))),
            "{source}: {:?}",
            result.chunk.block.stmts
        );
    }
}

#[test]
fn labels_gotos_and_global_declarations_are_errors() {
    let allocator = Allocator::new();
    for source in ["::label::", "goto done", "global x", "global function f() end", "global *"] {
        assert_has_errors(&parse(&allocator, source.as_bytes()));
    }
}

#[test]
fn a_local_takes_no_attribute() {
    let allocator = Allocator::new();
    // The statement ends at `local x`, and `<const> = 5` is left over.
    let result = parse(&allocator, b"local x <const> = 5");
    assert_has_errors(&result);
    let Some(Statement::LocalAssignment(local)) = result.chunk.block.stmts.first() else {
        panic!("expected LocalAssignment, got {:?}", result.chunk.block.stmts);
    };
    assert!(local.exprs.is_empty(), "should not reach `=`");
    assert_has_errors(&parse(&allocator, b"local <close> f = io.open(\"file\")"));
}

#[test]
fn xor_not_and_shifts_are_errors() {
    let allocator = Allocator::new();
    for source in ["local x = a ~ b", "local x = ~a", "local x = a << 1", "local x = a >> 1"] {
        assert_has_errors(&parse(&allocator, source.as_bytes()));
    }
}

#[test]
fn a_vararg_takes_no_name() {
    let allocator = Allocator::new();
    let result = parse(&allocator, b"function f(...) return args end");
    assert_no_errors(&result);
    let Some(Statement::FunctionDecl(function_decl)) = result.chunk.block.stmts.first() else {
        panic!("expected FunctionDecl, got {:?}", result.chunk.block.stmts);
    };
    assert!(function_decl.body.vararg.is_some(), "expected vararg");
    assert_has_errors(&parse(&allocator, b"function f(...args) return args end"));
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
fn a_semicolon_only_follows_a_statement() {
    let allocator = Allocator::new();
    // There's no empty statement: `;` only follows a statement.
    assert_has_errors(&parse(&allocator, b";"));
    assert_has_errors(&parse(&allocator, b";local x = 1"));
    assert_has_errors(&parse(&allocator, b"local x = 1;;"));
    assert_no_errors(&parse(&allocator, b"local x = 1;"));
    assert_no_errors(&parse(&allocator, b"local x = 1; local y = 2;"));
}

#[test]
fn a_call_whose_parenthesis_starts_a_line_is_ambiguous() {
    let allocator = Allocator::new();
    assert_has_errors(&parse(
        &allocator,
        b"\
a = b
(f)(g)",
    ));
    // Same line: fine.
    assert_no_errors(&parse(&allocator, b"a = b(f)(g)"));
    // Non-paren call args never trigger the check.
    assert_no_errors(&parse(
        &allocator,
        b"\
a = b
{ x = 1 }",
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

#[test]
fn ambiguous_newline_check_covers_method_calls() {
    let allocator = Allocator::new();
    // `parseFunctionArgs` applies the line check to `a:b(...)` too.
    let result = parse(
        &allocator,
        b"\
a:b
(c)",
    );
    assert!(
        result.errors.iter().any(|e| e.message.contains("ambiguous")),
        "must reject a method call whose `(` starts a new line: {:?}",
        result.errors
    );
}

#[test]
fn a_lone_carriage_return_does_not_make_a_call_ambiguous() {
    let allocator = Allocator::new();
    // Luau's lexer counts only `\n` as a line break.
    let result = parse(&allocator, b"f\r(x)");
    assert!(result.errors.is_empty(), "luau accepts this program: {:?}", result.errors);
}
