use luck_allocator::Allocator;
use luck_ast::Statement;
use luck_parser::ParseResult;
use luck_token::LuaVersion;

use crate::common::assert_no_errors;

fn parse_lua55<'ast>(allocator: &'ast Allocator, source: &'ast str) -> ParseResult<'ast> {
    luck_parser::parse(allocator, source.as_bytes(), LuaVersion::Lua55)
}

#[test]
fn global_stays_a_name_outside_a_declaration() {
    let allocator = Allocator::new();
    for source in [
        "local global = 1",
        "global = 1",
        "global.x = 1",
        "global()",
        "global \"x\"",
        "global {}",
        "function global() end",
        "return global",
    ] {
        let result = parse_lua55(&allocator, source);
        assert_no_errors(&result);
        assert!(
            !matches!(
                result.chunk.block.stmts.first(),
                Some(
                    Statement::GlobalDeclaration(_)
                        | Statement::GlobalFunction(_)
                        | Statement::GlobalStar(_)
                )
            ),
            "{source}: {:?}",
            result.chunk.block.stmts
        );
    }
}

#[test]
fn global_declares_before_a_name_attribute_star_or_function() {
    let allocator = Allocator::new();
    for source in
        ["global x", "global global", "global <const> x", "global *", "global function f() end"]
    {
        let result = parse_lua55(&allocator, source);
        assert_no_errors(&result);
        assert!(
            matches!(
                result.chunk.block.stmts.first(),
                Some(
                    Statement::GlobalDeclaration(_)
                        | Statement::GlobalFunction(_)
                        | Statement::GlobalStar(_)
                )
            ),
            "{source}: {:?}",
            result.chunk.block.stmts
        );
    }
}

#[test]
fn global_simple() {
    let allocator = Allocator::new();
    let result = parse_lua55(&allocator, "global x");
    assert_no_errors(&result);
    assert_eq!(result.chunk.block.stmts.len(), 1);
    assert!(
        matches!(&result.chunk.block.stmts[0], Statement::GlobalDeclaration(_)),
        "expected GlobalDeclaration, got {:?}",
        result.chunk.block.stmts[0]
    );
}

#[test]
fn global_multiple_names() {
    let allocator = Allocator::new();
    let result = parse_lua55(&allocator, "global x, y");
    assert_no_errors(&result);
    if let Statement::GlobalDeclaration(global) = &result.chunk.block.stmts[0] {
        let n = global.names.len();
        assert_eq!(n, 2);
    } else {
        panic!("expected GlobalDeclaration");
    }
}

#[test]
fn global_function() {
    let allocator = Allocator::new();
    let result = parse_lua55(&allocator, "global function f() end");
    assert_no_errors(&result);
    assert_eq!(result.chunk.block.stmts.len(), 1);
    assert!(
        matches!(&result.chunk.block.stmts[0], Statement::GlobalFunction(_)),
        "expected GlobalFunction, got {:?}",
        result.chunk.block.stmts[0]
    );
}

#[test]
fn global_star() {
    let allocator = Allocator::new();
    let result = parse_lua55(&allocator, "global *");
    assert_no_errors(&result);
    assert_eq!(result.chunk.block.stmts.len(), 1);
    if let Statement::GlobalStar(global_star) = &result.chunk.block.stmts[0] {
        assert!(global_star.attribute.is_none());
    } else {
        panic!("expected GlobalStar, got {:?}", result.chunk.block.stmts[0]);
    }
}

#[test]
fn global_star_with_attribute() {
    let allocator = Allocator::new();
    let result = parse_lua55(&allocator, "global <const> *");
    assert_no_errors(&result);
    assert_eq!(result.chunk.block.stmts.len(), 1);
    if let Statement::GlobalStar(global_star) = &result.chunk.block.stmts[0] {
        let attribute = global_star.attribute.as_ref().expect("expected attribute");
        assert_eq!(attribute.name.text, "const");
    } else {
        panic!("expected GlobalStar, got {:?}", result.chunk.block.stmts[0]);
    }
}

#[test]
fn local_attribute_before_first_name() {
    let allocator = Allocator::new();
    let result = parse_lua55(&allocator, "local <close> f = io.open(\"file\")");
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
        panic!("expected LocalAssignment, got {:?}", result.chunk.block.stmts[0]);
    }
}

#[test]
fn named_vararg() {
    let allocator = Allocator::new();
    let result = parse_lua55(&allocator, "function f(...args) return args end");
    assert_no_errors(&result);
    if let Statement::FunctionDecl(function_decl) = &result.chunk.block.stmts[0] {
        let vararg = function_decl.body.vararg.as_ref().expect("expected vararg");
        let vararg_name = vararg.name.as_ref().expect("expected named vararg");
        assert_eq!(vararg_name.text, "args");
    } else {
        panic!("expected FunctionDecl, got {:?}", result.chunk.block.stmts[0]);
    }
}

#[test]
fn named_vararg_after_params() {
    let allocator = Allocator::new();
    let result = parse_lua55(&allocator, "function f(a, b, ...rest) end");
    assert_no_errors(&result);
    if let Statement::FunctionDecl(function_decl) = &result.chunk.block.stmts[0] {
        let param_count = function_decl.body.params.len();
        assert_eq!(param_count, 2);
        let vararg = function_decl.body.vararg.as_ref().expect("expected vararg");
        let vararg_name = vararg.name.as_ref().expect("expected named vararg");
        assert_eq!(vararg_name.text, "rest");
    } else {
        panic!("expected FunctionDecl");
    }
}

#[test]
fn global_with_initializer() {
    let allocator = Allocator::new();
    // 5.5 manual section 3.3.7: stat ::= global attnamelist ['=' explist]
    let single = parse_lua55(&allocator, "global x = 1");
    assert_no_errors(&single);
    if let Statement::GlobalDeclaration(global) = &single.chunk.block.stmts[0] {
        assert_eq!(global.exprs.len(), 1);
    } else {
        panic!("expected GlobalDeclaration, got {:?}", single.chunk.block.stmts[0]);
    }

    let multiple = parse_lua55(&allocator, "global a, b = 1, 2");
    assert_no_errors(&multiple);
    if let Statement::GlobalDeclaration(global) = &multiple.chunk.block.stmts[0] {
        assert_eq!(global.names.len(), 2);
        assert_eq!(global.exprs.len(), 2);
    } else {
        panic!("expected GlobalDeclaration, got {:?}", multiple.chunk.block.stmts[0]);
    }
}

#[test]
fn global_attributed_with_initializer() {
    let allocator = Allocator::new();
    let result = parse_lua55(&allocator, "global x <const> = 1");
    assert_no_errors(&result);
    if let Statement::GlobalDeclaration(global) = &result.chunk.block.stmts[0] {
        assert_eq!(global.exprs.len(), 1);
    } else {
        panic!("expected GlobalDeclaration, got {:?}", result.chunk.block.stmts[0]);
    }
}

#[test]
fn leading_attribute_distributes_to_all_names() {
    let allocator = Allocator::new();
    // 5.5 manual section 3.3.7: "A prefixed attribute applies to all names in
    // the list."
    let result = parse_lua55(&allocator, "local <const> x, y = 1, 2");
    assert_no_errors(&result);
    if let Statement::LocalAssignment(local) = &result.chunk.block.stmts[0] {
        let attribute_names: Vec<_> = local
            .names
            .iter()
            .map(|attributed| attributed.attribute.as_ref().map(|attribute| attribute.name.text))
            .collect();
        assert_eq!(attribute_names.len(), 2);
        assert!(
            attribute_names.iter().all(Option::is_some),
            "leading attrib must apply to every name: {attribute_names:?}"
        );
    } else {
        panic!("expected LocalAssignment, got {:?}", result.chunk.block.stmts[0]);
    }
}

#[test]
fn trailing_attribute_replaces_the_leading_one_on_its_name() {
    let allocator = Allocator::new();
    // 5.5 `localstat`: `kind = getvarattribute(ls, defkind)`.
    let result = parse_lua55(&allocator, "local <const> x, y <close> = 1, 2");
    assert_no_errors(&result);
    let Statement::LocalAssignment(local) = &result.chunk.block.stmts[0] else {
        panic!("expected LocalAssignment, got {:?}", result.chunk.block.stmts[0]);
    };
    let attribute_names: Vec<_> = local
        .names
        .iter()
        .map(|attributed| Some(attributed.attribute.as_ref()?.name.text.as_str()))
        .collect();
    assert_eq!(attribute_names, [Some("const"), Some("close")]);
}

#[test]
fn postfixed_attribute_overrides_prefixed_attribute() {
    let allocator = Allocator::new();
    // 5.5 `localstat`: `kind = getvarattribute(ls, defkind)`.
    for source in ["local <const> a <close> = nil", "global <const> x <const>"] {
        let result = parse_lua55(&allocator, source);
        assert!(result.errors.is_empty(), "{source} is valid in Lua 5.5: {:?}", result.errors);
    }
}
