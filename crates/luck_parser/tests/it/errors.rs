use luck_ast::Statement;
use luck_token::LuaVersion;

use luck_parser::ParseResult;

fn parse(source: &str) -> ParseResult {
    luck_parser::parse(source, LuaVersion::Lua54)
}

#[test]
fn missing_variable_name_in_local() {
    let result = parse("local = 5");
    assert!(
        !result.errors.is_empty(),
        "should produce at least one error"
    );
}

#[test]
fn missing_then_in_if() {
    let result = parse("if true end");
    assert!(
        !result.errors.is_empty(),
        "should produce an error for missing 'then'"
    );
}

#[test]
fn recovery_parses_subsequent_statements() {
    let result = parse("local = 5\nlocal y = 10");
    assert!(
        !result.errors.is_empty(),
        "should have errors from first statement"
    );
    let has_valid_local = result.block.stmts.iter().any(|s| {
        matches!(s, Statement::LocalAssignment(la) if {
            la.names.last_item().is_some_and(|name| {
                matches!(&name.name.kind, luck_token::TokenKind::Identifier(n) if n == "y")
            })
        })
    });
    assert!(
        has_valid_local,
        "second statement should parse correctly after error recovery"
    );
}

#[test]
fn unclosed_paren_in_function() {
    let result = parse("function f( end");
    assert!(
        !result.errors.is_empty(),
        "should produce error for unclosed paren"
    );
}

#[test]
fn multiple_errors_collected() {
    let result = parse("local = 5\nlocal = 10\nlocal z = 15");
    assert!(
        result.errors.len() >= 2,
        "should collect multiple errors, got: {:?}",
        result.errors
    );
}

#[test]
fn unexpected_token_produces_error_node() {
    let result = parse(") local x = 1");
    assert!(!result.errors.is_empty());
    let has_error_node = result
        .block
        .stmts
        .iter()
        .any(|s| matches!(s, Statement::Error(_)));
    assert!(has_error_node, "should have an Error statement node");
}

#[test]
fn error_in_expression_position() {
    let result = parse("local x = )");
    assert!(
        !result.errors.is_empty(),
        "should error on invalid expression"
    );
}

#[test]
fn leading_comma_in_table() {
    let result = parse("local x = {,1}");
    assert!(
        !result.errors.is_empty(),
        "leading comma in table constructor should be an error"
    );
}
