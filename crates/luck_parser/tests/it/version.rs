use luck_allocator::Allocator;
use luck_ast::Statement;
use luck_parser::{ParseResult, parse};
use luck_token::LuaVersion;

use crate::common::assert_no_errors;

fn assert_has_errors(result: &ParseResult<'_>) {
    assert!(!result.errors.is_empty(), "expected parse errors but got none");
}

#[test]
fn goto_in_lua51_is_identifier() {
    let allocator = Allocator::new();
    // In Lua 5.1, `goto` is an ordinary identifier.
    let result = parse(&allocator, "goto = 1", LuaVersion::Lua51);
    assert_no_errors(&result);
    assert!(matches!(&result.chunk.block.stmts[0], Statement::Assignment(_)));
}

#[test]
fn label_is_an_error_in_lua51() {
    let allocator = Allocator::new();
    // In Lua 5.1 the lexer still produces DoubleColon, which the parser
    // has nowhere to put.
    let result = parse(&allocator, "::label::", LuaVersion::Lua51);
    assert_has_errors(&result);
}

#[test]
fn attribute_is_not_parsed_in_lua53() {
    let allocator = Allocator::new();
    // Lua 5.3 has no attributes, so the statement ends at `local x` and
    // `<const> = 5` is left over.
    let result = parse(&allocator, "local x <const> = 5", LuaVersion::Lua53);
    if let Statement::LocalAssignment(local) = &result.chunk.block.stmts[0] {
        assert!(
            local.names.first().expect("declared name").attribute.is_none(),
            "should not parse attribute in Lua 5.3"
        );
        assert!(local.exprs.is_empty(), "should not reach = in Lua 5.3");
    } else {
        panic!("expected LocalAssignment");
    }
}

#[test]
fn global_in_lua54_is_identifier() {
    let allocator = Allocator::new();
    // In Lua 5.4, `global` is an identifier, not a keyword.
    let result = parse(&allocator, "global = 1", LuaVersion::Lua54);
    assert_no_errors(&result);
    assert!(matches!(&result.chunk.block.stmts[0], Statement::Assignment(_)));
}

#[test]
fn bitwise_and_in_lua51_is_error() {
    let allocator = Allocator::new();
    // The lexer rejects `&` in Lua 5.1, so the errors are lex errors.
    let result = parse(&allocator, "local x = a & b", LuaVersion::Lua51);
    assert_has_errors(&result);
}

#[test]
fn bitwise_pipe_in_lua52_is_error() {
    let allocator = Allocator::new();
    // The lexer rejects `|` in Lua 5.2.
    let result = parse(&allocator, "local x = a | b", LuaVersion::Lua52);
    assert_has_errors(&result);
}

#[test]
fn vararg_name_is_not_consumed_in_lua54() {
    let allocator = Allocator::new();
    // Lua 5.4 has no named varargs, so the vararg parameter stays unnamed.
    let result = parse(&allocator, "function f(...) return args end", LuaVersion::Lua54);
    assert_no_errors(&result);
    if let Statement::FunctionDecl(function_decl) = &result.chunk.block.stmts[0] {
        let vararg = function_decl.body.vararg.as_ref().expect("expected vararg");
        assert!(vararg.name.is_none(), "vararg should not have a name in Lua 5.4");
    } else {
        panic!("expected FunctionDecl");
    }
}

#[test]
fn break_outside_loop_rejected() {
    let allocator = Allocator::new();
    for version in [LuaVersion::Lua51, LuaVersion::Lua54, LuaVersion::Luau] {
        assert_has_errors(&parse(&allocator, "break", version));
        assert_has_errors(&parse(&allocator, "do break end", version));
        assert_has_errors(&parse(&allocator, "function f() break end", version));
        // Function bodies reset the loop context.
        assert_has_errors(&parse(
            &allocator,
            "while true do local f = function() break end end",
            version,
        ));
        assert_no_errors(&parse(&allocator, "while true do break end", version));
        assert_no_errors(&parse(&allocator, "repeat break until x", version));
        assert_no_errors(&parse(&allocator, "for i = 1, 2 do break end", version));
        assert_no_errors(&parse(&allocator, "while true do do break end end", version));
    }
}

#[test]
fn continue_outside_loop_rejected() {
    let allocator = Allocator::new();
    assert_has_errors(&parse(&allocator, "continue", LuaVersion::Luau));
    assert_has_errors(&parse(
        &allocator,
        "while true do local f = function() continue end end",
        LuaVersion::Luau,
    ));
    assert_no_errors(&parse(&allocator, "while true do continue end", LuaVersion::Luau));
}

#[test]
fn vararg_outside_vararg_function_rejected() {
    let allocator = Allocator::new();
    for version in [LuaVersion::Lua51, LuaVersion::Lua54, LuaVersion::Luau] {
        // Main chunks are vararg.
        assert_no_errors(&parse(&allocator, "return ...", version));
        assert_no_errors(&parse(&allocator, "local function f(...) return ... end", version));
        assert_has_errors(&parse(&allocator, "local function f() return ... end", version));
        // Non-vararg function nested in a vararg one.
        assert_has_errors(&parse(
            &allocator,
            "local function f(...) local g = function(x) return ... end end",
            version,
        ));
    }
}

#[test]
fn unknown_lua_attribute_rejected() {
    let allocator = Allocator::new();
    for version in [LuaVersion::Lua54, LuaVersion::Lua55] {
        assert_no_errors(&parse(&allocator, "local x <const> = 1", version));
        assert_no_errors(&parse(&allocator, "local x <close> = nil", version));
        assert_has_errors(&parse(&allocator, "local x <foo> = 1", version));
    }
}

#[test]
fn multiple_close_attributes_rejected() {
    let allocator = Allocator::new();
    // 5.4 manual section 3.3.7: at most one to-be-closed variable per list.
    assert_has_errors(&parse(&allocator, "local x <close>, y <close> = a, b", LuaVersion::Lua54));
    assert_no_errors(&parse(&allocator, "local x <close>, y <const> = a, 1", LuaVersion::Lua54));
}

#[test]
fn close_attribute_on_globals_rejected() {
    let allocator = Allocator::new();
    // 5.5 manual section 3.3.7: only local variables can have the close
    // attribute.
    assert_has_errors(&parse(&allocator, "global x <close>", LuaVersion::Lua55));
    assert_has_errors(&parse(&allocator, "global <close> x", LuaVersion::Lua55));
    assert_has_errors(&parse(&allocator, "global <close> *", LuaVersion::Lua55));
    assert_has_errors(&parse(&allocator, "global <close> x <const>", LuaVersion::Lua55));
    assert_no_errors(&parse(&allocator, "global x <const>", LuaVersion::Lua55));
    assert_no_errors(&parse(&allocator, "global <const> *", LuaVersion::Lua55));
}

#[test]
fn empty_stmt_gated_by_version() {
    let allocator = Allocator::new();
    // 5.1 and Luau have no empty statement: `;` only follows a statement.
    for version in [LuaVersion::Lua51, LuaVersion::Luau] {
        assert_has_errors(&parse(&allocator, ";", version));
        assert_has_errors(&parse(&allocator, ";local x = 1", version));
        assert_has_errors(&parse(&allocator, "local x = 1;;", version));
        assert_no_errors(&parse(&allocator, "local x = 1;", version));
        assert_no_errors(&parse(&allocator, "local x = 1; local y = 2;", version));
    }
    for version in [LuaVersion::Lua52, LuaVersion::Lua54] {
        assert_no_errors(&parse(&allocator, ";", version));
        assert_no_errors(&parse(&allocator, ";;local x = 1;;", version));
    }
}

#[test]
fn ambiguous_newline_call_gated_by_version() {
    let allocator = Allocator::new();
    // 5.1 and Luau reject a call whose `(` starts a new line.
    let source = "\
a = b
(f)(g)";
    assert_has_errors(&parse(&allocator, source, LuaVersion::Lua51));
    assert_has_errors(&parse(&allocator, source, LuaVersion::Luau));
    assert_no_errors(&parse(&allocator, source, LuaVersion::Lua54));
    // Same line: fine everywhere.
    assert_no_errors(&parse(&allocator, "a = b(f)(g)", LuaVersion::Lua51));
    // Non-paren call args never trigger the check.
    assert_no_errors(&parse(
        &allocator,
        "\
a = b
{ x = 1 }",
        LuaVersion::Lua51,
    ));
}

#[test]
fn luau_function_attributes_must_be_known_and_unique() {
    let allocator = Allocator::new();
    assert_no_errors(&parse(&allocator, "@native function f() end", LuaVersion::Luau));
    assert_no_errors(&parse(&allocator, "@checked @native function f() end", LuaVersion::Luau));
    assert_has_errors(&parse(&allocator, "@totallymadeup function f() end", LuaVersion::Luau));
    assert_has_errors(&parse(&allocator, "@native @native function f() end", LuaVersion::Luau));
}

#[test]
fn ambiguous_newline_check_covers_method_calls() {
    let allocator = Allocator::new();
    // `funcargs` and `parseFunctionArgs` apply the line check to `a:b(...)` too.
    for version in [LuaVersion::Lua51, LuaVersion::Luau] {
        let result = parse(
            &allocator,
            "\
a:b
(c)",
            version,
        );
        assert!(
            result.errors.iter().any(|e| e.message.contains("ambiguous")),
            "{version:?} must reject a method call whose `(` starts a new line: {:?}",
            result.errors
        );
    }
}

#[test]
fn luau_lone_carriage_return_does_not_make_a_call_ambiguous() {
    let allocator = Allocator::new();
    // Luau's lexer counts only `\n` as a line break.
    let result = parse(&allocator, "f\r(x)", LuaVersion::Luau);
    assert!(result.errors.is_empty(), "luau accepts this program: {:?}", result.errors);
}
