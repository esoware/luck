use luck_allocator::Allocator;
use luck_ast::Statement;
use luck_parser::ParseResult;
use luck_token::LuaVersion;

use crate::common::assert_no_errors;

fn parse_lua52<'ast>(allocator: &'ast Allocator, source: &'ast str) -> ParseResult<'ast> {
    luck_parser::parse(allocator, source.as_bytes(), LuaVersion::Lua52)
}

#[test]
fn goto_stmt() {
    let allocator = Allocator::new();
    // The label is there because Lua rejects a goto with no visible label.
    let result = parse_lua52(&allocator, "goto label_name ::label_name::");
    assert_no_errors(&result);
    assert_eq!(result.chunk.block.stmts.len(), 2);
    if let Statement::Goto(goto) = &result.chunk.block.stmts[0] {
        assert_eq!(goto.name.text, "label_name");
    } else {
        panic!("expected Goto, got {:?}", result.chunk.block.stmts[0]);
    }
}

#[test]
fn label_stmt() {
    let allocator = Allocator::new();
    let result = parse_lua52(&allocator, "::my_label::");
    assert_no_errors(&result);
    assert_eq!(result.chunk.block.stmts.len(), 1);
    if let Statement::Label(label) = &result.chunk.block.stmts[0] {
        assert_eq!(label.name.text, "my_label");
    } else {
        panic!("expected Label, got {:?}", result.chunk.block.stmts[0]);
    }
}

#[test]
fn goto_and_label_in_same_block() {
    let allocator = Allocator::new();
    let result = parse_lua52(&allocator, "::start:: goto start");
    assert_no_errors(&result);
    assert_eq!(result.chunk.block.stmts.len(), 2);
    assert!(matches!(&result.chunk.block.stmts[0], Statement::Label(_)));
    assert!(matches!(&result.chunk.block.stmts[1], Statement::Goto(_)));
}

#[test]
fn empty_stmts_leave_no_node() {
    let allocator = Allocator::new();
    let result = parse_lua52(&allocator, ";; local x = 1;; ;");
    assert_no_errors(&result);
    assert!(matches!(*result.chunk.block.stmts, [Statement::LocalAssignment(_)]));
}

#[test]
fn break_can_precede_other_stmts() {
    let allocator = Allocator::new();
    let result = parse_lua52(&allocator, "while true do break local x = 1 end");
    assert_no_errors(&result);
    if let Statement::WhileLoop(while_loop) = &result.chunk.block.stmts[0] {
        assert!(
            while_loop.block.stmts.len() >= 2,
            "expected at least 2 statements in while body, got {}",
            while_loop.block.stmts.len()
        );
        assert!(
            matches!(&while_loop.block.stmts[0], Statement::Break(_)),
            "expected Break, got {:?}",
            while_loop.block.stmts[0]
        );
        assert!(
            matches!(&while_loop.block.stmts[1], Statement::LocalAssignment(_)),
            "expected LocalAssignment, got {:?}",
            while_loop.block.stmts[1]
        );
    } else {
        panic!("expected WhileLoop");
    }
}

#[test]
fn return_takes_a_trailing_semicolon() {
    let allocator = Allocator::new();
    let result = parse_lua52(&allocator, "return 1;");
    assert_no_errors(&result);
    let [Statement::Return(return_stmt)] = &*result.chunk.block.stmts else {
        panic!("expected Return, got {:?}", result.chunk.block.stmts);
    };
    assert_eq!(return_stmt.exprs.len(), 1);
    assert_eq!(return_stmt.span.end, 9, "the `;` is the return's");
}

#[test]
fn break_takes_any_semicolons() {
    let allocator = Allocator::new();
    let result = parse_lua52(&allocator, "while true do break;; end");
    assert_no_errors(&result);
    if let Statement::WhileLoop(while_loop) = &result.chunk.block.stmts[0] {
        assert!(
            matches!(&*while_loop.block.stmts, [Statement::Break(_)]),
            "expected a lone Break statement, got {:?}",
            while_loop.block.stmts
        );
    } else {
        panic!("expected WhileLoop");
    }
}
