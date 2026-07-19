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
        assert!(
            la.equal_and_exprs.is_none(),
            "should not reach = in Lua 5.3"
        );
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
