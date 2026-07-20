//! Luau type grammar tests (real type AST, not span scanning).

use luck_ast::stmt::TypeDeclarationValue;
use luck_ast::types::{Type, TypeField};
use luck_ast::{Expression, Statement};
use luck_token::LuaVersion;

use crate::common::{assert_no_errors, parse_luau};

/// Extract the alias type from `type X = ...` in a one-statement program.
fn alias_type(source: &str) -> Type {
    let result = parse_luau(source);
    assert_no_errors(&result);
    match &result.block.stmts[0] {
        Statement::TypeDeclaration(type_decl) => match &type_decl.type_value {
            TypeDeclarationValue::Alias(alias) => alias.clone(),
            TypeDeclarationValue::TypeFunction(_) => panic!("expected alias form"),
        },
        other => panic!("expected TypeDeclaration, got {other:?}"),
    }
}

#[test]
fn named_type() {
    assert!(matches!(alias_type("type X = number"), Type::Named(_)));
}

#[test]
fn qualified_named_type() {
    let Type::Named(named) = alias_type("type X = module.Thing") else {
        panic!("expected Named");
    };
    assert!(named.prefix.is_some());
}

#[test]
fn generic_args() {
    let Type::Named(named) = alias_type("type X = Map<string, number>") else {
        panic!("expected Named");
    };
    assert_eq!(named.generics.expect("generics").args.len(), 2);
}

#[test]
fn nested_generics_shift_right_split() {
    let Type::Named(named) = alias_type("type X = Map<string, Array<number>>") else {
        panic!("expected Named");
    };
    let args = named.generics.expect("generics");
    assert_eq!(args.args.len(), 2);
    let inner = args.args.get(1).expect("second arg");
    assert!(matches!(inner, Type::Named(inner) if inner.generics.is_some()));
}

#[test]
fn union_type() {
    let Type::Union(union) = alias_type("type X = string | number | nil") else {
        panic!("expected Union");
    };
    assert_eq!(union.types.len(), 3);
    assert!(!union.has_leading_pipe);
}

#[test]
fn union_with_leading_pipe() {
    let Type::Union(union) = alias_type("type X = | string | number") else {
        panic!("expected Union");
    };
    assert_eq!(union.types.len(), 2);
    assert!(union.has_leading_pipe);
}

#[test]
fn intersection_type() {
    let Type::Intersection(intersection) = alias_type("type X = A & B") else {
        panic!("expected Intersection");
    };
    assert_eq!(intersection.types.len(), 2);
}

#[test]
fn intersection_binds_tighter_than_union() {
    // Real Luau's suffix chain is flat: mixing `&` and `|` without
    // parentheses is a parse error, not a precedence question.
    let result = crate::common::parse_luau("type X = A & B | C");
    assert!(!result.errors.is_empty(), "mixed union/intersection errors");
    let Type::Union(union) = alias_type("type X = (A & B) | C") else {
        panic!("expected Union at top level");
    };
    assert_eq!(union.types.len(), 2);
    assert!(matches!(union.types.first(), Some(Type::Parenthesized(_))));
}

#[test]
fn optional_type() {
    assert!(matches!(alias_type("type X = number?"), Type::Optional(_)));
}

#[test]
fn optional_in_union() {
    let Type::Union(union) = alias_type("type X = number? | string") else {
        panic!("expected Union");
    };
    assert!(matches!(union.types.first(), Some(Type::Optional(_))));
}

#[test]
fn string_singleton() {
    let Type::Union(union) = alias_type(r#"type X = "on" | "off""#) else {
        panic!("expected Union");
    };
    assert!(matches!(union.types.first(), Some(Type::Singleton(_))));
}

#[test]
fn boolean_and_nil_singletons() {
    assert!(matches!(alias_type("type X = true"), Type::Singleton(_)));
    assert!(matches!(alias_type("type X = false"), Type::Singleton(_)));
    assert!(matches!(alias_type("type X = nil"), Type::Singleton(_)));
}

#[test]
fn table_type_named_fields() {
    let Type::Table(table) = alias_type("type X = { name: string, age: number }") else {
        panic!("expected Table");
    };
    assert_eq!(table.fields.len(), 2);
    assert!(matches!(&table.fields.items[0], TypeField::Named { .. }));
}

#[test]
fn table_type_indexer() {
    let Type::Table(table) = alias_type("type X = { [string]: number }") else {
        panic!("expected Table");
    };
    assert!(matches!(&table.fields.items[0], TypeField::Indexer { .. }));
}

#[test]
fn table_type_array_shorthand() {
    let Type::Table(table) = alias_type("type X = { number }") else {
        panic!("expected Table");
    };
    assert!(matches!(&table.fields.items[0], TypeField::Array { .. }));
}

#[test]
fn table_type_read_write_access() {
    let Type::Table(table) = alias_type("type X = { read name: string, write count: number }")
    else {
        panic!("expected Table");
    };
    for field in table.fields.iter() {
        assert!(matches!(
            field,
            TypeField::Named {
                access: Some(_),
                ..
            }
        ));
    }
}

#[test]
fn table_field_literally_named_read() {
    let Type::Table(table) = alias_type("type X = { read: number }") else {
        panic!("expected Table");
    };
    assert!(matches!(
        &table.fields.items[0],
        TypeField::Named { access: None, .. }
    ));
}

#[test]
fn function_type() {
    let Type::Function(function_type) = alias_type("type F = (number, string) -> boolean") else {
        panic!("expected Function");
    };
    assert_eq!(function_type.params.len(), 2);
}

#[test]
fn function_type_named_params() {
    let Type::Function(function_type) = alias_type("type F = (x: number, y: number) -> number")
    else {
        panic!("expected Function");
    };
    assert!(
        function_type
            .params
            .iter()
            .all(|param| param.name.is_some())
    );
}

#[test]
fn function_type_generic() {
    let Type::Function(function_type) = alias_type("type F = <T>(T) -> T") else {
        panic!("expected Function");
    };
    assert!(function_type.generics.is_some());
}

#[test]
fn function_type_variadic_param() {
    let Type::Function(function_type) = alias_type("type F = (number, ...string) -> ()") else {
        panic!("expected Function");
    };
    let last = function_type.params.last().expect("params");
    assert!(matches!(last.type_value, Type::Variadic(_)));
    assert!(matches!(function_type.return_type, Type::Pack(_)));
}

#[test]
fn function_type_pack_return() {
    let Type::Function(function_type) = alias_type("type F = () -> (number, string)") else {
        panic!("expected Function");
    };
    let Type::Pack(pack) = &function_type.return_type else {
        panic!("expected Pack return");
    };
    assert_eq!(pack.types.len(), 2);
}

#[test]
fn paren_type_is_not_pack() {
    assert!(matches!(
        alias_type("type X = (number)"),
        Type::Parenthesized(_)
    ));
}

#[test]
fn typeof_type() {
    let Type::Typeof(typeof_type) = alias_type("type X = typeof(game.Workspace)") else {
        panic!("expected Typeof");
    };
    assert!(matches!(typeof_type.expr, Expression::Var(_)));
}

#[test]
fn generic_pack_reference() {
    let Type::Function(function_type) = alias_type("type F = <T...>(T...) -> T...") else {
        panic!("expected Function");
    };
    assert!(matches!(function_type.return_type, Type::GenericPack(_)));
}

#[test]
fn type_declaration_generic_defaults() {
    let result = parse_luau("type Foo<T = string, S... = ...string> = T");
    assert_no_errors(&result);
    let Statement::TypeDeclaration(type_decl) = &result.block.stmts[0] else {
        panic!("expected TypeDeclaration");
    };
    let generics = type_decl.generics.as_ref().expect("generics");
    assert_eq!(generics.params.len(), 2);
    assert!(generics.params.iter().all(|param| param.default.is_some()));
    assert!(generics.params.last().expect("pack param").is_pack);
}

#[test]
fn export_type_declaration() {
    let result = parse_luau("export type Point = { x: number, y: number }");
    assert_no_errors(&result);
    let Statement::TypeDeclaration(type_decl) = &result.block.stmts[0] else {
        panic!("expected TypeDeclaration");
    };
    assert!(type_decl.is_exported);
}

#[test]
fn type_function_declaration_keeps_body() {
    let result = parse_luau("type function Id(t)\n\treturn t\nend");
    assert_no_errors(&result);
    let Statement::TypeDeclaration(type_decl) = &result.block.stmts[0] else {
        panic!("expected TypeDeclaration");
    };
    assert!(matches!(
        type_decl.type_value,
        TypeDeclarationValue::TypeFunction(_)
    ));
}

#[test]
fn local_annotation_preserved() {
    let result = parse_luau("local x: number = 1");
    assert_no_errors(&result);
    let Statement::LocalAssignment(local_assign) = &result.block.stmts[0] else {
        panic!("expected LocalAssignment");
    };
    let name = local_assign.names.first().expect("name");
    assert!(name.type_annotation.is_some());
}

#[test]
fn multi_local_annotations_preserved() {
    let result = parse_luau("local a: number, b, c: string? = 1, 2, \"x\"");
    assert_no_errors(&result);
    let Statement::LocalAssignment(local_assign) = &result.block.stmts[0] else {
        panic!("expected LocalAssignment");
    };
    let annotations: Vec<bool> = local_assign
        .names
        .iter()
        .map(|name| name.type_annotation.is_some())
        .collect();
    assert_eq!(annotations, vec![true, false, true]);
}

#[test]
fn numeric_for_annotation_preserved() {
    let result = parse_luau("for i: number = 1, 10 do end");
    assert_no_errors(&result);
    let Statement::NumericFor(numeric_for) = &result.block.stmts[0] else {
        panic!("expected NumericFor");
    };
    assert!(numeric_for.type_annotation.is_some());
}

#[test]
fn generic_for_annotations_preserved() {
    let result = parse_luau("for k: string, v: number in pairs(t) do end");
    assert_no_errors(&result);
    let Statement::GenericFor(generic_for) = &result.block.stmts[0] else {
        panic!("expected GenericFor");
    };
    assert!(
        generic_for
            .names
            .iter()
            .all(|binding| binding.type_annotation.is_some())
    );
}

#[test]
fn generic_function_declaration() {
    let result =
        parse_luau("local function map<T, U>(items: {T}, fn: (T) -> U): {U}\n\treturn {}\nend");
    assert_no_errors(&result);
    let Statement::LocalFunction(local_function) = &result.block.stmts[0] else {
        panic!("expected LocalFunction");
    };
    assert!(local_function.body.generics.is_some());
    assert!(local_function.body.return_type.is_some());
    assert!(
        local_function
            .body
            .params
            .iter()
            .all(|param| param.type_annotation.is_some())
    );
}

#[test]
fn cast_with_complex_type() {
    let result = parse_luau("local x = (y :: { [string]: number }?)");
    assert_no_errors(&result);
}

#[test]
fn chained_casts() {
    // One cast per simpleexp; real Luau requires parens to chain.
    let result = parse_luau("local x = y :: any :: number");
    assert!(!result.errors.is_empty(), "chained casts must error");
    let result = parse_luau("local x = (y :: any) :: number");
    assert_no_errors(&result);
}

#[test]
fn annotation_not_confused_with_method_call() {
    // `:` in `obj:method()` must not trigger type parsing outside
    // annotation positions
    let result = parse_luau("obj:method(1)");
    assert_no_errors(&result);
}

#[test]
fn type_annotations_rejected_outside_luau() {
    let result = luck_parser::parse("local x: number = 1", LuaVersion::Lua54);
    assert!(!result.errors.is_empty());
}

#[test]
fn mixed_union_intersection_rejected() {
    for source in [
        "type X = A | B & C",
        "type X = A & B | C",
        "type X = A & B?",
        "type X = A? & B",
    ] {
        let result = crate::common::parse_luau(source);
        assert!(!result.errors.is_empty(), "must reject mixing: {source}");
    }
    for source in [
        "type X = (A & B) | C",
        "type X = A & (B?)",
        "type X = A? | B",
        "type X = A | B | C?",
    ] {
        let result = crate::common::parse_luau(source);
        assert!(
            result.errors.is_empty(),
            "parenthesized forms parse: {source}"
        );
    }
}

#[test]
fn table_type_shape_rules() {
    // TableType ::= '{' Type '}' | '{' [PropList] '}'
    let bad = [
        "type T = {number, x: string}",
        "type T = {x: string, number}",
        "type T = {number, string}",
        "type T = {[string]: number, [number]: string}",
    ];
    for source in bad {
        let result = crate::common::parse_luau(source);
        assert!(!result.errors.is_empty(), "must reject: {source}");
    }
    let good = [
        "type T = {number}",
        "type T = {x: string, y: number}",
        "type T = {[string]: number, x: boolean}",
        "type T = {x: boolean, [string]: number}",
    ];
    for source in good {
        let result = crate::common::parse_luau(source);
        assert!(result.errors.is_empty(), "must accept: {source}");
    }
}

#[test]
fn generic_type_list_ordering() {
    // Packs come last; a defaulted param forces defaults on the rest;
    // pack defaults must be packs; defaults are alias-only.
    let bad = [
        "type X<T..., U> = U",
        "type X<T = number, U> = U",
        "type X<T... = number> = number",
        "type X<T = U...> = number",
        "local f = function<T = number>(x: T) return x end",
    ];
    for source in bad {
        let result = crate::common::parse_luau(source);
        assert!(!result.errors.is_empty(), "must reject: {source}");
    }
    let good = [
        "type X<T, U...> = T",
        "type X<T, U = number> = U",
        "type X<T... = ...number> = number",
        "type X<T... = (string, number)> = number",
    ];
    for source in good {
        let result = crate::common::parse_luau(source);
        assert!(result.errors.is_empty(), "must accept: {source}");
    }
}
