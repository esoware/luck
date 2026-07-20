use luck_ast::Statement;
use luck_token::LuaVersion;

use crate::common::assert_no_errors;
use luck_parser::ParseResult;

fn parse(source: &str, version: LuaVersion) -> ParseResult {
    luck_parser::parse(source, version)
}

fn assert_has_errors(result: &ParseResult) {
    assert!(
        !result.errors.is_empty(),
        "expected parse errors but got none"
    );
}

#[test]
fn goto_in_lua51_is_identifier() {
    // In Lua 5.1, `goto` is just an identifier. `goto label` parses as
    // a function call or assignment, not a goto statement.
    let result = parse("goto = 1", LuaVersion::Lua51);
    assert_no_errors(&result);
    // `goto` is an identifier so `goto = 1` is assignment
    assert!(matches!(&result.block.stmts[0], Statement::Assignment(_)));
}

#[test]
fn double_colon_label_in_lua51() {
    // `::label::` in Lua 5.1 - lexer produces DoubleColon which is unexpected
    let result = parse("::label::", LuaVersion::Lua51);
    assert_has_errors(&result);
}

#[test]
fn attribute_in_lua53_not_parsed() {
    // `local x <const> = 5` in Lua 5.3 - `<` is not an attribute syntax.
    // The parser sees `local x` then stops (no `=` or `,`), so `<const> = 5` is left unparsed.
    // No attribute is attached to the local.
    let result = parse("local x <const> = 5", LuaVersion::Lua53);
    if let Statement::LocalAssignment(la) = &result.block.stmts[0] {
        assert!(
            la.names.get(0).expect("declared name").attrib.is_none(),
            "should not parse attribute in Lua 5.3"
        );
        assert!(la.exprs.is_none(), "should not reach = in Lua 5.3");
    } else {
        panic!("expected LocalAssignment");
    }
}

#[test]
fn global_in_lua54_is_identifier() {
    // In Lua 5.4, `global` is an identifier, not a keyword.
    // `global = 1` should parse as assignment.
    let result = parse("global = 1", LuaVersion::Lua54);
    assert_no_errors(&result);
    assert!(matches!(&result.block.stmts[0], Statement::Assignment(_)));
}

#[test]
fn bitwise_and_in_lua51_is_error() {
    // The lexer rejects `&` in Lua 5.1, so we get lex errors
    let result = parse("local x = a & b", LuaVersion::Lua51);
    assert_has_errors(&result);
}

#[test]
fn bitwise_pipe_in_lua52_is_error() {
    // The lexer rejects `|` in Lua 5.2
    let result = parse("local x = a | b", LuaVersion::Lua52);
    assert_has_errors(&result);
}

#[test]
fn named_vararg_in_lua54_not_consumed() {
    // In Lua 5.4, `...args` - the `args` after `...` is not consumed as vararg name
    let result = parse("function f(...) return args end", LuaVersion::Lua54);
    assert_no_errors(&result);
    if let Statement::FunctionDecl(f) = &result.block.stmts[0] {
        let vararg = f.body.vararg.as_ref().expect("expected vararg");
        assert!(
            vararg.name.is_none(),
            "vararg should not have a name in Lua 5.4"
        );
    } else {
        panic!("expected FunctionDecl");
    }
}

#[test]
fn break_outside_loop_rejected() {
    for version in [LuaVersion::Lua51, LuaVersion::Lua54, LuaVersion::Luau] {
        assert_has_errors(&parse("break", version));
        assert_has_errors(&parse("do break end", version));
        assert_has_errors(&parse("function f() break end", version));
        // Function bodies reset the loop context.
        assert_has_errors(&parse(
            "while true do local f = function() break end end",
            version,
        ));
        assert_no_errors(&parse("while true do break end", version));
        assert_no_errors(&parse("repeat break until x", version));
        assert_no_errors(&parse("for i = 1, 2 do break end", version));
        assert_no_errors(&parse("while true do do break end end", version));
    }
}

#[test]
fn continue_outside_loop_rejected() {
    assert_has_errors(&parse("continue", LuaVersion::Luau));
    assert_has_errors(&parse(
        "while true do local f = function() continue end end",
        LuaVersion::Luau,
    ));
    assert_no_errors(&parse("while true do continue end", LuaVersion::Luau));
}

#[test]
fn vararg_outside_vararg_function_rejected() {
    for version in [LuaVersion::Lua51, LuaVersion::Lua54, LuaVersion::Luau] {
        // Main chunks are vararg.
        assert_no_errors(&parse("return ...", version));
        assert_no_errors(&parse("local function f(...) return ... end", version));
        assert_has_errors(&parse("local function f() return ... end", version));
        // Non-vararg function nested in a vararg one.
        assert_has_errors(&parse(
            "local function f(...) local g = function(x) return ... end end",
            version,
        ));
    }
}

#[test]
fn unknown_lua_attribute_rejected() {
    for version in [LuaVersion::Lua54, LuaVersion::Lua55] {
        assert_no_errors(&parse("local x <const> = 1", version));
        assert_no_errors(&parse("local x <close> = nil", version));
        assert_has_errors(&parse("local x <foo> = 1", version));
    }
}

#[test]
fn multiple_close_attributes_rejected() {
    // 5.4 §3.3.7: at most one to-be-closed variable per list.
    assert_has_errors(&parse(
        "local x <close>, y <close> = a, b",
        LuaVersion::Lua54,
    ));
    assert_no_errors(&parse(
        "local x <close>, y <const> = a, 1",
        LuaVersion::Lua54,
    ));
}

#[test]
fn close_attribute_on_globals_rejected() {
    // 5.5 §3.3.7: only local variables can have the close attribute.
    assert_has_errors(&parse("global x <close>", LuaVersion::Lua55));
    assert_has_errors(&parse("global <close> x", LuaVersion::Lua55));
    assert_has_errors(&parse("global <close> *", LuaVersion::Lua55));
    assert_no_errors(&parse("global x <const>", LuaVersion::Lua55));
    assert_no_errors(&parse("global <const> *", LuaVersion::Lua55));
}

#[test]
fn empty_statement_gated_by_version() {
    // 5.1 and Luau have no empty statement: `;` only follows a stat.
    for version in [LuaVersion::Lua51, LuaVersion::Luau] {
        assert_has_errors(&parse(";", version));
        assert_has_errors(&parse(";local x = 1", version));
        assert_has_errors(&parse("local x = 1;;", version));
        assert_no_errors(&parse("local x = 1;", version));
        assert_no_errors(&parse("local x = 1; local y = 2;", version));
    }
    for version in [LuaVersion::Lua52, LuaVersion::Lua54] {
        assert_no_errors(&parse(";", version));
        assert_no_errors(&parse(";;local x = 1;;", version));
    }
}

#[test]
fn ambiguous_newline_call_gated_by_version() {
    // 5.1 and Luau hard-error on a call whose `(` opens a new line.
    let source = "a = b\n(f)(g)";
    assert_has_errors(&parse(source, LuaVersion::Lua51));
    assert_has_errors(&parse(source, LuaVersion::Luau));
    assert_no_errors(&parse(source, LuaVersion::Lua54));
    // Same line: fine everywhere.
    assert_no_errors(&parse("a = b(f)(g)", LuaVersion::Lua51));
    // Non-paren call args never trigger the check.
    assert_no_errors(&parse("a = b\n{ x = 1 }", LuaVersion::Lua51));
}

#[test]
fn luau_function_attribute_names_validated() {
    assert_no_errors(&parse("@native function f() end", LuaVersion::Luau));
    assert_no_errors(&parse(
        "@checked @native function f() end",
        LuaVersion::Luau,
    ));
    assert_has_errors(&parse("@totallymadeup function f() end", LuaVersion::Luau));
    assert_has_errors(&parse("@native @native function f() end", LuaVersion::Luau));
}
