use luck_ast::Statement;
use luck_token::LuaVersion;

use crate::common::assert_no_errors;
use luck_parser::ParseResult;

fn parse_lua55(source: &str) -> ParseResult {
    luck_parser::parse(source, LuaVersion::Lua55)
}

#[test]
fn global_simple() {
    let result = parse_lua55("global x");
    assert_no_errors(&result);
    assert_eq!(result.block.stmts.len(), 1);
    assert!(
        matches!(&result.block.stmts[0], Statement::GlobalDeclaration(_)),
        "expected GlobalDeclaration, got {:?}",
        result.block.stmts[0]
    );
}

#[test]
fn global_multiple_names() {
    let result = parse_lua55("global x, y");
    assert_no_errors(&result);
    if let Statement::GlobalDeclaration(gd) = &result.block.stmts[0] {
        let count = gd.names.len();
        assert_eq!(count, 2);
    } else {
        panic!("expected GlobalDeclaration");
    }
}

#[test]
fn global_function() {
    let result = parse_lua55("global function f() end");
    assert_no_errors(&result);
    assert_eq!(result.block.stmts.len(), 1);
    assert!(
        matches!(&result.block.stmts[0], Statement::GlobalFunction(_)),
        "expected GlobalFunction, got {:?}",
        result.block.stmts[0]
    );
}

#[test]
fn global_star() {
    let result = parse_lua55("global *");
    assert_no_errors(&result);
    assert_eq!(result.block.stmts.len(), 1);
    if let Statement::GlobalStar(gs) = &result.block.stmts[0] {
        assert!(gs.attrib.is_none());
    } else {
        panic!("expected GlobalStar, got {:?}", result.block.stmts[0]);
    }
}

#[test]
fn global_star_with_attribute() {
    let result = parse_lua55("global <const> *");
    assert_no_errors(&result);
    assert_eq!(result.block.stmts.len(), 1);
    if let Statement::GlobalStar(gs) = &result.block.stmts[0] {
        let attrib = gs.attrib.as_ref().expect("expected attribute");
        assert!(matches!(
            &attrib.name.kind,
            luck_token::TokenKind::Identifier(name) if name == "const"
        ));
    } else {
        panic!("expected GlobalStar, got {:?}", result.block.stmts[0]);
    }
}

#[test]
fn local_attribute_before_first_name() {
    let result = parse_lua55("local <close> f = io.open(\"file\")");
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
        panic!("expected LocalAssignment, got {:?}", result.block.stmts[0]);
    }
}

#[test]
fn named_vararg() {
    let result = parse_lua55("function f(...args) return args end");
    assert_no_errors(&result);
    if let Statement::FunctionDecl(f) = &result.block.stmts[0] {
        let vararg = f.body.vararg.as_ref().expect("expected vararg");
        let vararg_name = vararg.name.as_ref().expect("expected named vararg");
        assert!(matches!(
            &vararg_name.kind,
            luck_token::TokenKind::Identifier(name) if name == "args"
        ));
    } else {
        panic!("expected FunctionDecl, got {:?}", result.block.stmts[0]);
    }
}

#[test]
fn named_vararg_after_params() {
    let result = parse_lua55("function f(a, b, ...rest) end");
    assert_no_errors(&result);
    if let Statement::FunctionDecl(f) = &result.block.stmts[0] {
        let param_count = f.body.params.len();
        assert_eq!(param_count, 2);
        let vararg = f.body.vararg.as_ref().expect("expected vararg");
        let vararg_name = vararg.name.as_ref().expect("expected named vararg");
        assert!(matches!(
            &vararg_name.kind,
            luck_token::TokenKind::Identifier(name) if name == "rest"
        ));
    } else {
        panic!("expected FunctionDecl");
    }
}
