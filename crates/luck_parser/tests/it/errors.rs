use core::ops::Range;

use luck_ast::{LastStatement, Statement};
use luck_parser::ParseResult;
use luck_token::LuaVersion;

fn parse(source: &str) -> ParseResult {
    luck_parser::parse(source, LuaVersion::Lua54)
}

#[test]
fn expectation_errors_have_exact_message_and_span() {
    for (source, span, message) in [
        ("local = 5", luck_token::Span::new(6, 7), "expected identifier, found `=`"),
        ("if true end", luck_token::Span::new(8, 11), "missing `then` in if-statement"),
        ("function f()", luck_token::Span::new(12, 12), "expected `end`, found end of file"),
    ] {
        let result = parse(source);
        assert!(
            result.errors.iter().any(|e| e.span == span && e.message == message),
            "{source}: {:?}",
            result.errors
        );
    }
}

#[test]
fn recovery_parses_subsequent_stmts() {
    let result = parse(
        "\
local = 5
local y = 10",
    );
    assert!(!result.errors.is_empty(), "should have errors from first statement");
    let has_valid_local = result.block.stmts.iter().any(|stmt| {
        matches!(stmt, Statement::LocalAssignment(local) if {
            local.names.last().is_some_and(|attributed| attributed.name.identifier() == Some("y"))
        })
    });
    assert!(has_valid_local, "second statement should parse correctly after error recovery");
}

#[test]
fn unclosed_parameter_list_is_an_error() {
    let result = parse("function f( end");
    assert!(!result.errors.is_empty(), "should produce error for unclosed paren");
}

#[test]
fn errors_in_separate_stmts_are_all_reported() {
    let result = parse(
        "\
local = 5
local = 10
local z = 15",
    );
    assert!(result.errors.len() >= 2, "should collect multiple errors, got: {:?}", result.errors);
}

#[test]
fn unexpected_token_becomes_an_error_stmt() {
    let result = parse(") local x = 1");
    assert_ne!(result.errors, Vec::<luck_token::SourceError>::new());
    let has_error_node = result.block.stmts.iter().any(|stmt| matches!(stmt, Statement::Error(_)));
    assert!(has_error_node, "should have an Error statement node");
}

#[test]
fn unmatched_paren_in_expr_position_is_an_error() {
    let result = parse("local x = )");
    assert!(!result.errors.is_empty(), "should error on invalid expression");
}

#[test]
fn stmt_after_top_level_return_is_an_error() {
    // Accepting it would silently drop the tail from the AST.
    let result = parse(
        "\
return 1
local x = 2",
    );
    assert!(!result.errors.is_empty(), "statements after a top-level return must be an error");
}

#[test]
fn stray_end_at_top_level_is_an_error() {
    let result = parse(
        "\
local x = 1
end",
    );
    assert!(!result.errors.is_empty(), "a stray end at top level must be an error");
}

/// Each broken source is its one error, with that text, and the statement
/// after it parses as it would have.
fn assert_one_error_then_the_rest(cases: &[(LuaVersion, &str, &str, &str)]) {
    for &(version, broken, message, text) in cases {
        let source = format!(
            "\
{broken}
local after = 1"
        );
        let result = luck_parser::parse(source.as_str(), version);
        let errors: Vec<(&str, &str)> =
            result.errors.iter().map(|e| (e.message.as_str(), e.span.text(&source))).collect();
        assert_eq!(errors, [(message, text)], "{broken:?}");
        if message.starts_with("unterminated long") {
            continue;
        }
        let Some(Statement::LocalAssignment(after)) = result.block.stmts.last() else {
            panic!("{broken:?}: {:?}", result.block.stmts.last());
        };
        assert_eq!(after.names.first().and_then(|name| name.name.identifier()), Some("after"));
    }
}

/// A clean recovery reports the one error, where it is, and the statement
/// after the broken one parses as it would have.
#[test]
fn a_broken_construct_is_one_error_and_the_rest_parses() {
    let lua = LuaVersion::Lua54;
    let luau = LuaVersion::Luau;
    assert_one_error_then_the_rest(&[
        (lua, "x = f(g(h(", "unexpected token `local` in an expression", "local"),
        (lua, "x = (a]", "expected `)`, found `]`", "]"),
        (lua, "x = t[f(a]", "expected `)`, found `]`", "]"),
        (lua, "x = {a, b)", "expected `}`, found `)`", ")"),
        (
            lua,
            "\
x = 1
end",
            "unexpected token `end` (extra `end` without matching block)",
            "end",
        ),
        (
            lua,
            "\
x = 1
) ) )",
            "unexpected token `)` (unmatched `)`)",
            ")",
        ),
        (lua, "return 1", "`return` must be the last statement in its block", "local"),
        (
            lua,
            "function f() return 1 x = 2 end",
            "`return` must be the last statement in its block",
            "x",
        ),
        (
            LuaVersion::Lua51,
            "while a do break x() end",
            "`break` must be the last statement in its block",
            "x",
        ),
        (
            luau,
            "\
while a do continue
x() end",
            "`continue` must be the last statement in its block",
            "x",
        ),
    ]);
}

/// A string cut off at its line is its lexer error alone, and a broken
/// expression or type is one error where it breaks.
#[test]
fn a_broken_string_expression_or_type_is_one_error_and_the_rest_parses() {
    let lua = LuaVersion::Lua54;
    let luau = LuaVersion::Luau;
    assert_one_error_then_the_rest(&[
        (lua, "f(\"abc)", "unterminated string", "\"abc)"),
        (lua, r#"x = "a\qb" .. c"#, r"invalid escape sequence `\q`", r"\q"),
        (
            lua,
            "x = [[abc",
            "unterminated long bracket string",
            "\
[[abc
local after = 1",
        ),
        (luau, "local s = `{function() + }tail`", "unexpected token `+`", "+"),
        (luau, "local s = `{x}tail", "unterminated interpolated string", "}tail"),
        (luau, "local s = `tail", "unterminated interpolated string", "`tail"),
        (
            lua,
            "\
f(function()
  local y = 1
)",
            "expected `end`, found `)`",
            ")",
        ),
        (
            lua,
            "\
if a then
  f(x
else
  g()
end",
            "expected `)`, found `else`",
            "else",
        ),
        (lua, "repeat x() end", "missing `until` in repeat-loop", "end"),
        (lua, "local t = {1, 2 3}", "expected `}`, found number `3`", "3"),
        (lua, "print(a, b c, d)", "expected `)`, found identifier `c`", "c"),
        (
            lua,
            "x = = 1",
            "unexpected token `=` in an expression (use `==` for comparison, not `=`)",
            "=",
        ),
        (lua, "local a = 1 + * 2", "unexpected token `*` in an expression", "*"),
        (lua, "function f(a b) end", "expected `)`, found identifier `b`", "b"),
        (luau, "local x: A<B = 1", "expected `>` to close generics, found `=`", "="),
        (luau, "type X = {a: number", "expected `}`, found `local`", "local"),
    ]);
}

/// Each construct left open at the end of the file is the one error there.
#[test]
fn constructs_left_open_at_the_end_are_one_error() {
    for (source, message) in [
        ("x = f(g(h(", "unexpected token end of file in an expression"),
        ("x = {{{", "expected `}`, found end of file"),
        ("do do while a do", "missing `end` in while-loop"),
        ("if a then repeat function f()", "expected `end`, found end of file"),
    ] {
        let result = parse(source);
        let errors: Vec<(&str, usize)> = result
            .errors
            .iter()
            .map(|e| (e.message.as_str(), Range::<usize>::from(e.span).start))
            .collect();
        assert_eq!(errors, [(message, source.len())], "{source:?}");
    }
}

/// Statements after a misplaced `return` stay in the tree, after it, as they
/// would be had it been written `do return end`.
#[test]
fn a_misplaced_return_keeps_what_follows_it() {
    let result = parse(
        "\
local a = 1
return a
local b = 2",
    );
    let [Statement::LocalAssignment(_), Statement::DoBlock(wrapped), Statement::LocalAssignment(b)] =
        result.block.stmts.as_slice()
    else {
        panic!("{:?}", result.block.stmts);
    };
    assert_eq!(wrapped.block.stmts, []);
    assert!(matches!(wrapped.block.last_stmt.as_deref(), Some(LastStatement::Return(_))));
    assert_eq!(b.names.first().and_then(|name| name.name.identifier()), Some("b"));
    assert!(result.block.last_stmt.is_none());
}

/// An error in one statement doesn't hide one in the next.
#[test]
fn an_error_on_each_line_is_reported_on_each_line() {
    let source = "\
local = 1
x = = 2
f(a b)
local ok = 3";
    let result = parse(source);
    let texts: Vec<&str> = result.errors.iter().map(|e| e.span.text(source)).collect();
    assert_eq!(texts, ["=", "=", "b"], "{:?}", result.errors);
}
