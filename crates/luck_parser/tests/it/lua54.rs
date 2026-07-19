use luck_ast::Statement;
use luck_token::LuaVersion;

use crate::common::assert_no_errors;
use luck_parser::ParseResult;

fn parse_lua54(source: &str) -> ParseResult {
    luck_parser::parse(source, LuaVersion::Lua54)
}

#[test]
fn local_const_attribute() {
    let result = parse_lua54("local x <const> = 5");
    assert_no_errors(&result);
    if let Statement::LocalAssignment(la) = &result.block.stmts[0] {
        assert_eq!(la.names.len(), 1);
        let attrib = la
            .names
            .get(0)
            .expect("declared name")
            .attrib
            .as_ref()
            .expect("expected attribute on x");
        assert!(matches!(
            &attrib.name.kind,
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
    if let Statement::LocalAssignment(la) = &result.block.stmts[0] {
        assert_eq!(la.names.len(), 1);
        let attrib = la
            .names
            .get(0)
            .expect("declared name")
            .attrib
            .as_ref()
            .expect("expected attribute on f");
        assert!(matches!(
            &attrib.name.kind,
            luck_token::TokenKind::Identifier(name) if name == "close"
        ));
    } else {
        panic!("expected LocalAssignment");
    }
}

#[test]
fn local_mixed_attributes_only_second() {
    // `local a, b <const> = 1, 2` - only b has attribute
    let result = parse_lua54("local a, b <const> = 1, 2");
    assert_no_errors(&result);
    if let Statement::LocalAssignment(la) = &result.block.stmts[0] {
        assert_eq!(la.names.len(), 2);
        assert!(
            la.names.get(0).expect("declared name").attrib.is_none(),
            "a should have no attribute"
        );
        assert!(
            la.names.get(1).expect("declared name").attrib.is_some(),
            "b should have an attribute"
        );
    } else {
        panic!("expected LocalAssignment");
    }
}

#[test]
fn local_both_have_attributes() {
    // `local a <const>, b <close> = 1, 2`
    let result = parse_lua54("local a <const>, b <close> = 1, 2");
    assert_no_errors(&result);
    if let Statement::LocalAssignment(la) = &result.block.stmts[0] {
        assert_eq!(la.names.len(), 2);
        let attr_a = la
            .names
            .get(0)
            .expect("declared name")
            .attrib
            .as_ref()
            .expect("a should have attribute");
        assert!(matches!(
            &attr_a.name.kind,
            luck_token::TokenKind::Identifier(name) if name == "const"
        ));
        let attr_b = la
            .names
            .get(1)
            .expect("declared name")
            .attrib
            .as_ref()
            .expect("b should have attribute");
        assert!(matches!(
            &attr_b.name.kind,
            luck_token::TokenKind::Identifier(name) if name == "close"
        ));
    } else {
        panic!("expected LocalAssignment");
    }
}
