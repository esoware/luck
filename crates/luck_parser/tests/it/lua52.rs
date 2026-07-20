use luck_ast::{LastStatement, Statement};
use luck_token::LuaVersion;

use crate::common::assert_no_errors;
use luck_parser::ParseResult;

fn parse_lua52(source: &str) -> ParseResult {
    luck_parser::parse(source, LuaVersion::Lua52)
}

#[test]
fn goto_statement() {
    // A goto must resolve to a visible label (undefined -> compile error).
    let result = parse_lua52("goto label_name ::label_name::");
    assert_no_errors(&result);
    assert_eq!(result.block.stmts.len(), 2);
    if let Statement::Goto(goto) = &result.block.stmts[0] {
        assert!(matches!(
            &goto.name.kind,
            luck_token::TokenKind::Identifier(name) if name == "label_name"
        ));
    } else {
        panic!("expected Goto, got {:?}", result.block.stmts[0]);
    }
}

#[test]
fn label_statement() {
    let result = parse_lua52("::my_label::");
    assert_no_errors(&result);
    assert_eq!(result.block.stmts.len(), 1);
    if let Statement::Label(label) = &result.block.stmts[0] {
        assert!(matches!(
            &label.name.kind,
            luck_token::TokenKind::Identifier(name) if name == "my_label"
        ));
    } else {
        panic!("expected Label, got {:?}", result.block.stmts[0]);
    }
}

#[test]
fn goto_and_label_in_same_block() {
    let result = parse_lua52("::start:: goto start");
    assert_no_errors(&result);
    assert_eq!(result.block.stmts.len(), 2);
    assert!(matches!(&result.block.stmts[0], Statement::Label(_)));
    assert!(matches!(&result.block.stmts[1], Statement::Goto(_)));
}

#[test]
fn three_empty_statements() {
    let result = parse_lua52(";;;");
    assert_no_errors(&result);
    assert_eq!(result.block.stmts.len(), 3);
    for stmt in &result.block.stmts {
        assert!(
            matches!(stmt, Statement::EmptyStatement(_)),
            "expected EmptyStatement, got {:?}",
            stmt
        );
    }
}

#[test]
fn break_in_middle_of_block() {
    let result = parse_lua52("while true do break local x = 1 end");
    assert_no_errors(&result);
    if let Statement::WhileLoop(w) = &result.block.stmts[0] {
        assert!(
            w.block.stmts.len() >= 2,
            "expected at least 2 statements in while body, got {}",
            w.block.stmts.len()
        );
        assert!(
            matches!(&w.block.stmts[0], Statement::Break(_)),
            "expected Break, got {:?}",
            w.block.stmts[0]
        );
        assert!(
            matches!(&w.block.stmts[1], Statement::LocalAssignment(_)),
            "expected LocalAssignment, got {:?}",
            w.block.stmts[1]
        );
    } else {
        panic!("expected WhileLoop");
    }
}

#[test]
fn return_with_trailing_semicolon() {
    let result = parse_lua52("return 1;");
    assert_no_errors(&result);
    if let Some(last) = &result.block.last_stmt {
        if let LastStatement::Return(ret) = &**last {
            assert!(ret.exprs.len() == 1);
        } else {
            panic!("expected Return");
        }
    } else {
        panic!("expected last statement");
    }
}

#[test]
fn break_still_works_as_last_statement() {
    let result = parse_lua52("while true do break end");
    assert_no_errors(&result);
    if let Statement::WhileLoop(w) = &result.block.stmts[0] {
        // In 5.2+, break is parsed as a regular statement, not last statement
        assert!(
            !w.block.stmts.is_empty() || w.block.last_stmt.is_some(),
            "expected break somewhere in the block"
        );
    } else {
        panic!("expected WhileLoop");
    }
}
