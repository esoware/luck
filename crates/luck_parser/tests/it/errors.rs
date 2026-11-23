use core::ops::Range;

use luck_allocator::Allocator;
use luck_ast::{Expression, FunctionArgs, Statement};
use luck_parser::ParseResult;
use luck_token::LuaVersion;

fn parse<'ast>(allocator: &'ast Allocator, source: &'ast str) -> ParseResult<'ast> {
    luck_parser::parse(allocator, source.as_bytes(), LuaVersion::Lua54)
}

#[test]
fn expectation_errors_have_exact_message_and_span() {
    let allocator = Allocator::new();
    for (source, span, message) in [
        ("local = 5", luck_token::Span::new(6, 7), "expected identifier, found `=`"),
        ("if true end", luck_token::Span::new(8, 11), "missing `then` in if-statement"),
        (
            "function f()",
            luck_token::Span::new(12, 12),
            "expected `end` to close `function`, found end of file",
        ),
    ] {
        let result = parse(&allocator, source);
        assert!(
            result.errors.iter().any(|e| e.span == Some(span) && e.message == message),
            "{source}: {:?}",
            result.errors
        );
    }
}

#[test]
fn recovery_parses_subsequent_stmts() {
    let allocator = Allocator::new();
    let result = parse(
        &allocator,
        "\
local = 5
local y = 10",
    );
    assert!(!result.errors.is_empty(), "should have errors from first statement");
    let has_valid_local = result.chunk.block.stmts.iter().any(|stmt| {
        matches!(stmt, Statement::LocalAssignment(local) if {
            local.names.last().is_some_and(|attributed| attributed.name.text == "y")
        })
    });
    assert!(has_valid_local, "second statement should parse correctly after error recovery");
}

#[test]
fn unclosed_parameter_list_is_an_error() {
    let allocator = Allocator::new();
    let result = parse(&allocator, "function f( end");
    assert!(!result.errors.is_empty(), "should produce error for unclosed paren");
}

#[test]
fn errors_in_separate_stmts_are_all_reported() {
    let allocator = Allocator::new();
    let result = parse(
        &allocator,
        "\
local = 5
local = 10
local z = 15",
    );
    assert!(result.errors.len() >= 2, "should collect multiple errors, got: {:?}", result.errors);
}

#[test]
fn unexpected_token_becomes_an_error_stmt() {
    let allocator = Allocator::new();
    let result = parse(&allocator, ") local x = 1");
    assert_ne!(result.errors, Vec::<luck_diagnostics::Diagnostic>::new());
    let has_error_node =
        result.chunk.block.stmts.iter().any(|stmt| matches!(stmt, Statement::Error(_)));
    assert!(has_error_node, "should have an Error statement node");
}

#[test]
fn unmatched_paren_in_expr_position_is_an_error() {
    let allocator = Allocator::new();
    let result = parse(&allocator, "local x = )");
    assert!(!result.errors.is_empty(), "should error on invalid expression");
}

#[test]
fn stmt_after_top_level_return_is_an_error() {
    let allocator = Allocator::new();
    // Accepting it would silently drop the tail from the AST.
    let result = parse(
        &allocator,
        "\
return 1
local x = 2",
    );
    assert!(!result.errors.is_empty(), "statements after a top-level return must be an error");
}

#[test]
fn stray_end_at_top_level_is_an_error() {
    let allocator = Allocator::new();
    let result = parse(
        &allocator,
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
        let allocator = Allocator::new();
        let result = luck_parser::parse(&allocator, source.as_bytes(), version);
        let errors: Vec<(&str, &[u8])> = result
            .errors
            .iter()
            .map(|e| (&*e.message, e.span.unwrap().text(source.as_bytes())))
            .collect();
        assert_eq!(errors, [(message, text.as_bytes())], "{broken:?}");
        if message.starts_with("unterminated long") {
            continue;
        }
        let Some(Statement::LocalAssignment(after)) = result.chunk.block.stmts.last() else {
            panic!("{broken:?}: {:?}", result.chunk.block.stmts.last());
        };
        assert_eq!(after.names.first().map(|name| name.name.text.as_str()), Some("after"));
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
        (lua, "x = (a]", "expected `)` to close `(`, found `]`", "]"),
        (lua, "x = t[f(a]", "expected `)` to close `(`, found `]`", "]"),
        (lua, "x = {a, b)", "expected `}` to close `{`, found `)`", ")"),
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

/// Lua 5.1 and Luau take one `;` after a statement, a last one included, and
/// 5.2+ any number but after a `return`, whose own `;` ends its block.
#[test]
fn semicolons_follow_each_grammar() {
    let allocator = Allocator::new();
    for (version, source, is_valid) in [
        (LuaVersion::Lua51, "x = 1; y = 2;", true),
        (LuaVersion::Lua51, "x = 1;;", false),
        (LuaVersion::Lua51, "do ; end", false),
        (LuaVersion::Lua51, "while a do break; end", true),
        (LuaVersion::Lua51, "while a do break;; end", false),
        (LuaVersion::Luau, "while a do continue; end", true),
        (LuaVersion::Luau, "while a do continue;; end", false),
        (LuaVersion::Lua54, "do ;; end x = 1;;", true),
        (LuaVersion::Lua54, "while a do break;; end", true),
        (LuaVersion::Lua54, "return;", true),
        (LuaVersion::Lua54, "return 1;;", false),
    ] {
        let result = luck_parser::parse(&allocator, source.as_bytes(), version);
        assert_eq!(result.errors.is_empty(), is_valid, "{version:?} {source}: {:?}", result.errors);
    }
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
            "expected `end` to close `function`, found `)`",
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
            "expected `)` to close `(`, found `else`",
            "else",
        ),
        (lua, "repeat x() end", "expected `until` to close `repeat`, found `end`", "end"),
        (lua, "local t = {1, 2 3}", "expected `}` to close `{`, found number `3`", "3"),
        (lua, "print(a, b c, d)", "expected `)` to close `(`, found identifier `c`", "c"),
        (
            lua,
            "x = = 1",
            "unexpected token `=` in an expression (use `==` for comparison, not `=`)",
            "=",
        ),
        (lua, "local a = 1 + * 2", "unexpected token `*` in an expression", "*"),
        (lua, "function f(a b) end", "expected `)` to close `(`, found identifier `b`", "b"),
        (luau, "local x: A<B = 1", "expected `>` to close `<`, found `=`", "="),
        (luau, "type X = {a: number", "expected `}` to close `{`, found `local`", "local"),
        (luau, "local s = `a{b c}`", "expected `}` to close `{`, found identifier `c`", "c"),
    ]);
}

/// A block comment with no closer takes the rest of the source, closers
/// included, so it's the one error and the comment the tree keeps.
#[test]
fn an_unterminated_block_comment_is_its_one_error() {
    let allocator = Allocator::new();
    for source in [
        "\
local t = {
  1, --[[ note
}",
        "\
if x then --[[ note
end",
        "local x = --[[ note",
        "\
f(a, --[==[ note ]]
)",
        "\
do
  --[[ note
end",
    ] {
        let result = parse(&allocator, source);
        let cut_off =
            source.find("--").and_then(|start| source.get(start..)).expect("a comment opener");
        let errors: Vec<(&str, &[u8])> = result
            .errors
            .iter()
            .map(|e| (&*e.message, e.span.unwrap().text(source.as_bytes())))
            .collect();
        assert_eq!(errors, [("unterminated block comment", cut_off.as_bytes())], "{source:?}");
        let comments: Vec<&[u8]> =
            result.comments.iter().map(|comment| comment.span.text(source.as_bytes())).collect();
        assert_eq!(comments, [cut_off.as_bytes()], "{source:?}");
    }
}

/// A method call with nothing to call it with has error arguments, at the
/// token found in their place.
#[test]
fn a_method_call_without_arguments_has_error_arguments() {
    let allocator = Allocator::new();
    let source = "\
x = a:b
local after = 1";
    let result = parse(&allocator, source);
    let errors: Vec<(&str, &[u8])> = result
        .errors
        .iter()
        .map(|e| (&*e.message, e.span.unwrap().text(source.as_bytes())))
        .collect();
    assert_eq!(errors, [("expected function arguments", &b"local"[..])]);
    let Some(Statement::Assignment(assignment)) = result.chunk.block.stmts.first() else {
        panic!("{:?}", result.chunk.block.stmts);
    };
    let Some(Expression::Suffixed(chain)) = assignment.values.first() else {
        panic!("{:?}", assignment.values);
    };
    let call = chain.last_call().expect("the chain ends in the call");
    let FunctionArgs::Error(span) = call.args else {
        panic!("{:?}", call.args);
    };
    assert_eq!(span.text(source.as_bytes()), b"local");
}

/// Each construct left open at the end of the file is the one error there.
#[test]
fn constructs_left_open_at_the_end_are_one_error() {
    let allocator = Allocator::new();
    for (source, message) in [
        ("x = f(g(h(", "unexpected token end of file in an expression"),
        ("x = {{{", "expected `}` to close `{`, found end of file"),
        ("do do while a do", "expected `end` to close `while`, found end of file"),
        ("if a then repeat function f()", "expected `end` to close `function`, found end of file"),
    ] {
        let result = parse(&allocator, source);
        let errors: Vec<(&str, usize)> = result
            .errors
            .iter()
            .map(|e| (&*e.message, Range::<usize>::from(e.span.unwrap()).start))
            .collect();
        assert_eq!(errors, [(message, source.len())], "{source:?}");
    }
}

/// What a construct left open is missing points at the token that opened it.
#[test]
fn an_unclosed_construct_is_labeled_where_it_opened() {
    let allocator = Allocator::new();
    for (version, source, opener) in [
        (LuaVersion::Lua54, "local function f() return 1", "function"),
        (LuaVersion::Lua54, "x = function() end + function()", "function"),
        (LuaVersion::Lua54, "if a then b() else", "if"),
        (LuaVersion::Lua54, "while a do", "while"),
        (LuaVersion::Lua54, "for i = 1, 2 do", "for"),
        (LuaVersion::Lua54, "for k in pairs(t) do", "for"),
        (LuaVersion::Lua54, "do", "do"),
        (LuaVersion::Lua54, "repeat", "repeat"),
        (LuaVersion::Lua54, "x = f(a", "("),
        (LuaVersion::Lua54, "x = {1,", "{"),
        (LuaVersion::Lua54, "x = t[1", "["),
        (LuaVersion::Luau, "local x: A<B", "<"),
        (LuaVersion::Luau, "local s = `a{b c}`", "{"),
    ] {
        let result = luck_parser::parse(&allocator, source.as_bytes(), version);
        let [error] = result.errors.as_slice() else {
            panic!("{source:?}: {:?}", result.errors);
        };
        let labels: Vec<(&[u8], &str)> = error
            .labels
            .iter()
            .map(|label| (label.span.text(source.as_bytes()), &*label.message))
            .collect();
        let unclosed = format!("unclosed `{opener}`");
        assert_eq!(labels, [(opener.as_bytes(), unclosed.as_str())], "{source:?}");
    }
}

/// The labels of the one error in `source`, by the text each points at.
fn labels_of(source: &str) -> Vec<(&[u8], String)> {
    let allocator = Allocator::new();
    let result = parse(&allocator, source);
    let [error] = result.errors.as_slice() else {
        panic!("{source:?}: {:?}", result.errors);
    };
    error
        .labels
        .iter()
        .map(|label| (label.span.text(source.as_bytes()), label.message.to_string()))
        .collect()
}

/// A missing `end` also points at the latest construct inside the one left
/// open whose `end` sits at another indentation than its first line.
///
/// Luau guesses so, since that `end` was likely written for the construct
/// around it.
#[test]
fn a_missing_end_points_at_the_misindented_construct_that_took_it() {
    for (source, suspect) in [
        (
            "\
function f()
  if x then
    y()
end
",
            ("if", "did you forget to close this `if`?"),
        ),
        (
            "\
do
  local function g()
    return 1
end
",
            ("function", "did you forget to close this `function`?"),
        ),
        (
            "\
do
  repeat
    x()
until done
",
            ("repeat", "did you forget to close this `repeat`?"),
        ),
        (
            "\
while a do
  for i = 1, 2 do
    if b then
      c()
  end
end
",
            ("if", "did you forget to close this `if`?"),
        ),
    ] {
        let labels = labels_of(source);
        assert_eq!(
            labels.get(1),
            Some(&(suspect.0.as_bytes(), suspect.1.to_owned())),
            "{source:?} {labels:?}"
        );
    }
}

/// With every `end` at its construct's indentation, nothing suggests which
/// one is missing.
#[test]
fn a_missing_end_guesses_nothing_when_every_end_lines_up() {
    for source in [
        "\
function f()
  if x then
    y()
  end
",
        "\
do
  local t = setmetatable({}, { __index = function() return 1 end })
",
        "if a then b() end do",
    ] {
        assert_eq!(labels_of(source).len(), 1, "{source:?}");
    }
}

/// Statements after a misplaced `return` stay in the tree, after it, as they
/// would be had it been written `do return end`.
#[test]
fn a_misplaced_return_keeps_what_follows_it() {
    let allocator = Allocator::new();
    let result = parse(
        &allocator,
        "\
local a = 1
return a
local b = 2",
    );
    let [Statement::LocalAssignment(_), Statement::Return(_), Statement::LocalAssignment(b)] =
        &*result.chunk.block.stmts
    else {
        panic!("{:?}", result.chunk.block.stmts);
    };
    assert_eq!(b.names.first().map(|name| name.name.text.as_str()), Some("b"));
}

/// An error in one statement doesn't hide one in the next.
#[test]
fn an_error_on_each_line_is_reported_on_each_line() {
    let allocator = Allocator::new();
    let source = "\
local = 1
x = = 2
f(a b)
local ok = 3";
    let result = parse(&allocator, source);
    let texts: Vec<&[u8]> =
        result.errors.iter().map(|e| e.span.unwrap().text(source.as_bytes())).collect();
    assert_eq!(texts, [b"=", b"=", b"b"], "{:?}", result.errors);
}
