use luck_ast::Statement;
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
    let result = parse("local = 5\nlocal y = 10");
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
    let result = parse("local = 5\nlocal = 10\nlocal z = 15");
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
    let result = parse("return 1\nlocal x = 2");
    assert!(!result.errors.is_empty(), "statements after a top-level return must be an error");
}

#[test]
fn stray_end_at_top_level_is_an_error() {
    let result = parse("local x = 1\nend");
    assert!(!result.errors.is_empty(), "a stray end at top level must be an error");
}
