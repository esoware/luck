use luck_ast::Statement;
use luck_parser::ParseResult;
use luck_token::LuaVersion;

use crate::common::assert_no_errors;

fn parse_lua54(source: &str) -> ParseResult {
    luck_parser::parse(source, LuaVersion::Lua54)
}

#[test]
fn local_const_attribute() {
    let result = parse_lua54("local x <const> = 5");
    assert_no_errors(&result);
    if let Statement::LocalAssignment(local) = &result.block.stmts[0] {
        assert_eq!(local.names.len(), 1);
        let attribute = local
            .names
            .get(0)
            .expect("declared name")
            .attribute
            .as_ref()
            .expect("expected attribute on x");
        assert!(matches!(
            &attribute.name.kind,
            luck_token::TokenKind::Identifier(name) if name == "const"
        ));
    } else {
        panic!("expected LocalAssignment, got {:?}", result.block.stmts[0]);
    }
}

#[test]
fn local_close_attribute() {
    let result = parse_lua54("local f <close> = io.open(\"file\")");
    assert_no_errors(&result);
    if let Statement::LocalAssignment(local) = &result.block.stmts[0] {
        assert_eq!(local.names.len(), 1);
        let attribute = local
            .names
            .get(0)
            .expect("declared name")
            .attribute
            .as_ref()
            .expect("expected attribute on f");
        assert!(matches!(
            &attribute.name.kind,
            luck_token::TokenKind::Identifier(name) if name == "close"
        ));
    } else {
        panic!("expected LocalAssignment");
    }
}

#[test]
fn local_mixed_attributes_only_second() {
    // In `local a, b <const> = 1, 2` only `b` carries the attribute.
    let result = parse_lua54("local a, b <const> = 1, 2");
    assert_no_errors(&result);
    if let Statement::LocalAssignment(local) = &result.block.stmts[0] {
        assert_eq!(local.names.len(), 2);
        assert!(
            local.names.get(0).expect("declared name").attribute.is_none(),
            "a should have no attribute"
        );
        assert!(
            local.names.get(1).expect("declared name").attribute.is_some(),
            "b should have an attribute"
        );
    } else {
        panic!("expected LocalAssignment");
    }
}

#[test]
fn local_both_have_attributes() {
    let result = parse_lua54("local a <const>, b <close> = 1, 2");
    assert_no_errors(&result);
    if let Statement::LocalAssignment(local) = &result.block.stmts[0] {
        assert_eq!(local.names.len(), 2);
        let attribute_a = local
            .names
            .get(0)
            .expect("declared name")
            .attribute
            .as_ref()
            .expect("a should have attribute");
        assert!(matches!(
            &attribute_a.name.kind,
            luck_token::TokenKind::Identifier(name) if name == "const"
        ));
        let attribute_b = local
            .names
            .get(1)
            .expect("declared name")
            .attribute
            .as_ref()
            .expect("b should have attribute");
        assert!(matches!(
            &attribute_b.name.kind,
            luck_token::TokenKind::Identifier(name) if name == "close"
        ));
    } else {
        panic!("expected LocalAssignment");
    }
}
