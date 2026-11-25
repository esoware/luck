use luck_allocator::Allocator;
use luck_ast::Statement;
use luck_parser::ParseResult;
use luck_token::LuaVersion;

use crate::common::assert_no_errors;

fn parse_lua54<'ast>(allocator: &'ast Allocator, source: &'ast str) -> ParseResult<'ast> {
    luck_parser::parse(allocator, source.as_bytes(), LuaVersion::Lua54)
}

#[test]
fn local_keeps_its_const_attribute() {
    let allocator = Allocator::new();
    let result = parse_lua54(&allocator, "local x <const> = 5");
    assert_no_errors(&result);
    if let Statement::LocalAssignment(local) = &result.chunk.block.stmts[0] {
        assert_eq!(local.names.len(), 1);
        let attribute = local
            .names
            .first()
            .expect("declared name")
            .attribute
            .as_ref()
            .expect("expected attribute on x");
        assert_eq!(attribute.name.text, "const");
    } else {
        panic!("expected LocalAssignment, got {:?}", result.chunk.block.stmts[0]);
    }
}

#[test]
fn local_keeps_its_close_attribute() {
    let allocator = Allocator::new();
    let result = parse_lua54(&allocator, "local f <close> = io.open(\"file\")");
    assert_no_errors(&result);
    if let Statement::LocalAssignment(local) = &result.chunk.block.stmts[0] {
        assert_eq!(local.names.len(), 1);
        let attribute = local
            .names
            .first()
            .expect("declared name")
            .attribute
            .as_ref()
            .expect("expected attribute on f");
        assert_eq!(attribute.name.text, "close");
    } else {
        panic!("expected LocalAssignment");
    }
}

#[test]
fn trailing_attribute_applies_only_to_its_name() {
    let allocator = Allocator::new();
    // In `local a, b <const> = 1, 2` only `b` carries the attribute.
    let result = parse_lua54(&allocator, "local a, b <const> = 1, 2");
    assert_no_errors(&result);
    if let Statement::LocalAssignment(local) = &result.chunk.block.stmts[0] {
        assert_eq!(local.names.len(), 2);
        assert!(local.names[0].attribute.is_none(), "a should have no attribute");
        assert!(local.names[1].attribute.is_some(), "b should have an attribute");
    } else {
        panic!("expected LocalAssignment");
    }
}

#[test]
fn each_name_keeps_its_own_attribute() {
    let allocator = Allocator::new();
    let result = parse_lua54(&allocator, "local a <const>, b <close> = 1, 2");
    assert_no_errors(&result);
    if let Statement::LocalAssignment(local) = &result.chunk.block.stmts[0] {
        assert_eq!(local.names.len(), 2);
        let attribute_a = local.names[0].attribute.as_ref().expect("a should have attribute");
        assert_eq!(attribute_a.name.text, "const");
        let attribute_b = local.names[1].attribute.as_ref().expect("b should have attribute");
        assert_eq!(attribute_b.name.text, "close");
    } else {
        panic!("expected LocalAssignment");
    }
}
