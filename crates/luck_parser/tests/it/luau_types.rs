use core::ops::Range;

use luck_ast::{Expression, Statement, Type, TypeDeclarationValue, TypeField};
use luck_token::LuaVersion;

use crate::common::{assert_no_errors, parse_luau};

fn alias_type(source: &str) -> Type {
    let result = parse_luau(source);
    assert_no_errors(&result);
    let stmt = &result.block.stmts[0];
    let Statement::TypeDeclaration(type_declaration) = stmt else {
        panic!("expected TypeDeclaration, got {stmt:?}");
    };
    match &type_declaration.type_value {
        TypeDeclarationValue::Alias(alias) => alias.clone(),
        TypeDeclarationValue::TypeFunction(_) => panic!("expected alias form"),
    }
}

#[test]
fn named_type() {
    for expected in ["number", "LongTypeNameThatExceedsInlineStringStorage"] {
        let source = format!("type X = {expected}");
        let Type::Name { name, span } = alias_type(&source) else {
            panic!("expected inline Name");
        };
        assert_eq!(name, expected);
        assert_eq!(source.get(Range::<usize>::from(span)), Some(expected));
    }
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
fn nested_generics_split_the_closing_shift_right() {
    let Type::Named(named) = alias_type("type X = Map<string, Array<number>>") else {
        panic!("expected Named");
    };
    let args = named.generics.expect("generics");
    assert_eq!(args.args.len(), 2);
    let inner = args.args.get(1).expect("second arg");
    assert!(matches!(inner, Type::Named(nested) if nested.generics.is_some()));
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
fn parenthesized_intersection_is_one_union_member() {
    // Luau's suffix chain is flat: mixing `&` and `|` without parentheses
    // is a parse error, not a precedence question.
    let result = parse_luau("type X = A & B | C");
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
fn negation_syntax_is_an_error_in_luau() {
    for source in [
        "return ~1",
        "type X = ~nil",
        "type X = ~nil?",
        "type X = ~~nil",
        "type X = ~string | nil",
        "type X = ~(string | nil)",
    ] {
        let result = parse_luau(source);
        assert!(!result.errors.is_empty(), "{source}");
    }
    let result = parse_luau("return 1 ~= 2");
    assert!(result.errors.is_empty(), "{:?}", result.errors);
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
        assert!(matches!(field, TypeField::Named { access: Some(_), .. }));
    }
}

#[test]
fn table_field_literally_named_read() {
    let Type::Table(table) = alias_type("type X = { read: number }") else {
        panic!("expected Table");
    };
    assert!(matches!(&table.fields.items[0], TypeField::Named { access: None, .. }));
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
    assert!(function_type.params.iter().all(|param| param.name.is_some()));
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
fn parenthesized_type_is_not_a_pack() {
    assert!(matches!(alias_type("type X = (number)"), Type::Parenthesized(_)));
}

#[test]
fn parenthesized_types_preserve_pack_and_function_shapes() {
    // Packs stand alone only where one is expected, such as a return.
    for (source, n) in [("type X = () -> ()", 0), ("type X = () -> (number, string)", 2)] {
        let Type::Function(function) = alias_type(source) else {
            panic!("expected function for {source}");
        };
        let Type::Pack(pack) = function.return_type else {
            panic!("expected pack return for {source}");
        };
        assert_eq!(pack.types.len(), n);
    }
    for source in ["type X = (number) -> string", "type X = (value: number) -> string"] {
        let Type::Function(function) = alias_type(source) else {
            panic!("expected function for {source}");
        };
        assert_eq!(function.params.len(), 1);
    }
    let Type::Parenthesized(outer) = alias_type("type X = ((number))") else {
        panic!("expected nested parentheses");
    };
    assert!(matches!(outer.type_value, Type::Parenthesized(_)));
}

#[test]
fn named_parameter_outside_a_function_type_is_an_error() {
    let parsed = parse_luau("type X = (value: number)");
    assert_eq!(parsed.errors.len(), 1, "{:?}", parsed.errors);
    assert_eq!(parsed.errors[0].span, luck_token::Span::new(10, 15));
    assert_eq!(parsed.errors[0].message, "named parameters are only valid in function types");
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
    let Statement::TypeDeclaration(type_declaration) = &result.block.stmts[0] else {
        panic!("expected TypeDeclaration");
    };
    let generics = type_declaration.generics.as_ref().expect("generics");
    assert_eq!(generics.params.len(), 2);
    assert!(generics.params.iter().all(|param| param.default.is_some()));
    assert!(generics.params.last().expect("pack param").is_pack);
}

#[test]
fn local_annotation_preserved() {
    let result = parse_luau("local x: number = 1");
    assert_no_errors(&result);
    let Statement::LocalAssignment(local) = &result.block.stmts[0] else {
        panic!("expected LocalAssignment");
    };
    let name = local.names.first().expect("name");
    assert!(name.type_annotation.is_some());
}

#[test]
fn multi_local_annotations_preserved() {
    let result = parse_luau("local a: number, b, c: string? = 1, 2, \"x\"");
    assert_no_errors(&result);
    let Statement::LocalAssignment(local) = &result.block.stmts[0] else {
        panic!("expected LocalAssignment");
    };
    let annotations: Vec<bool> =
        local.names.iter().map(|name| name.type_annotation.is_some()).collect();
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
    assert!(generic_for.names.iter().all(|binding| binding.type_annotation.is_some()));
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
    assert!(local_function.body.params.iter().all(|param| param.type_annotation.is_some()));
}

#[test]
fn cast_with_complex_type() {
    let result = parse_luau("local x = (y :: { [string]: number }?)");
    assert_no_errors(&result);
}

#[test]
fn annotation_not_confused_with_method_call() {
    // The `:` in `obj:method()` must not start a type annotation.
    let result = parse_luau("obj:method(1)");
    assert_no_errors(&result);
}

#[test]
fn type_annotations_rejected_outside_luau() {
    let result = luck_parser::parse("local x: number = 1", LuaVersion::Lua54);
    assert_ne!(result.errors, Vec::<luck_token::SourceError>::new());
}

#[test]
fn mixed_union_intersection_rejected() {
    for source in ["type X = A | B & C", "type X = A & B | C", "type X = A & B?", "type X = A? & B"]
    {
        let result = parse_luau(source);
        assert!(!result.errors.is_empty(), "must reject mixing: {source}");
    }
    for source in
        ["type X = (A & B) | C", "type X = A & (B?)", "type X = A? | B", "type X = A | B | C?"]
    {
        let result = parse_luau(source);
        assert!(result.errors.is_empty(), "parenthesized forms parse: {source}");
    }
}

#[test]
fn table_type_shape_rules() {
    // TableType ::= '{' Type '}' | '{' [PropList] '}'
    let invalid_sources = [
        "type T = {number, x: string}",
        "type T = {x: string, number}",
        "type T = {number, string}",
        "type T = {[string]: number, [number]: string}",
    ];
    for source in invalid_sources {
        let result = parse_luau(source);
        assert!(!result.errors.is_empty(), "must reject: {source}");
    }
    let valid_sources = [
        "type T = {number}",
        "type T = {x: string, y: number}",
        "type T = {[string]: number, x: boolean}",
        "type T = {x: boolean, [string]: number}",
    ];
    for source in valid_sources {
        let result = parse_luau(source);
        assert!(result.errors.is_empty(), "must accept: {source}");
    }
}

#[test]
fn generic_type_list_ordering() {
    // Packs come last, a plain parameter after a defaulted one needs a
    // default, a default must match its parameter's kind, and only aliases
    // take defaults.
    let invalid_sources = [
        "type X<T..., U> = U",
        "type X<T = number, U> = U",
        "type X<T... = number> = number",
        "type X<T = U...> = number",
        "local f = function<T = number>(x: T) return x end",
    ];
    for source in invalid_sources {
        let result = parse_luau(source);
        assert!(!result.errors.is_empty(), "must reject: {source}");
    }
    let valid_sources = [
        "type X<T, U...> = T",
        "type X<T, U = number> = U",
        "type X<T... = ...number> = number",
        "type X<T... = (string, number)> = number",
    ];
    for source in valid_sources {
        let result = parse_luau(source);
        assert!(result.errors.is_empty(), "must accept: {source}");
    }
}

fn assert_rejected(sources: &[&str]) {
    for source in sources {
        let result = parse_luau(source);
        assert!(!result.errors.is_empty(), "luau rejects {source}");
    }
}

fn assert_accepted(sources: &[&str]) {
    for source in sources {
        let result = parse_luau(source);
        assert!(result.errors.is_empty(), "luau accepts {source}: {:?}", result.errors);
    }
}

#[test]
fn bare_type_pack_is_rejected_in_single_type_position() {
    assert_rejected(&[
        "type X = ()",
        "type X = (number, string)",
        "local x: () = 1",
        "function f(x: ()) end",
        "x = a :: (number, string)",
        "type X = {a: ()}",
    ]);
}

#[test]
fn variadic_and_generic_packs_are_rejected_as_single_types() {
    assert_rejected(&[
        "type X = ...number",
        "type X = T...",
        "local x: T... = 1",
        "function f(a: ...number) end",
        "type X = number | T...",
        "x = a :: T...",
        "type X = (a: T...) -> ()",
        "function f(...: ...number) end",
    ]);
}

#[test]
fn type_pack_takes_no_union_suffix() {
    assert_rejected(&[
        "function f(): (number, string)? end",
        "function f(): (number, string) | string end",
        "type X = Foo<(A, B)?>",
        "type X = Foo<T...?>",
        "type X = (T...?) -> ()",
        "function f(): T...? end",
    ]);
}

#[test]
fn return_pack_folds_a_suffix_over_the_type_before_its_variadic() {
    assert_accepted(&[
        "function f(): (A, ...B)? end",
        "function f(): (A, ...B) | C end",
        "function f(): (A, T...)? end",
    ]);
    assert_rejected(&["local x: (A, ...B)? = y", "type X = Foo<(A, ...B)?>"]);
}

#[test]
fn string_table_type_key_rejects_an_embedded_nul() {
    assert_rejected(&["type X = { [\"a\\0b\"]: number }"]);
}

#[test]
fn trailing_comma_is_rejected_in_function_type_params() {
    assert_rejected(&["type X = (number,) -> ()", "type X = (a: number,) -> ()"]);
}

#[test]
fn trailing_comma_is_rejected_in_type_arguments() {
    assert_rejected(&["type X = Foo<number,>", "x = f<<T,>>()"]);
}

#[test]
fn trailing_comma_is_rejected_in_generic_lists() {
    assert_rejected(&[
        "type X<T,> = T",
        "type X<T...,> = number",
        "function f<T,>() end",
        "type X = <T,>() -> ()",
    ]);
}

#[test]
fn empty_generic_list_is_rejected() {
    assert_rejected(&[
        "type X<> = number",
        "function f<>() end",
        "local function f<>() end",
        "return function<>() end",
        "type X = <>() -> ()",
    ]);
}

#[test]
fn type_pack_must_be_the_last_function_type_parameter() {
    assert_rejected(&[
        "type X = (...number, string) -> ()",
        "type X = (T..., string) -> ()",
        "type X = (number, ...string, number) -> ()",
    ]);
}

#[test]
fn typeof_requires_a_parenthesized_expression() {
    assert_rejected(&["type X = typeof", "local x: typeof = 1", "type X = typeof<T>"]);
}

#[test]
fn array_table_type_takes_no_trailing_separator() {
    assert_rejected(&["type X = { number, }", "type X = { number; }"]);
}

#[test]
fn access_modifier_applies_to_an_array_element_type() {
    assert_accepted(&[
        "type X = { read number }",
        "type X = { write number }",
        "type X = { read {number} }",
        "type X = { read (number) -> () }",
    ]);
}

#[test]
fn bare_read_or_write_is_a_modifier_missing_its_type() {
    assert_rejected(&[
        "type X = { read }",
        "type X = { write }",
        "type X = { read, }",
        "type X = { read? }",
        "type X = { read.x }",
    ]);
}

#[test]
fn string_keyed_table_type_fields_are_not_indexers() {
    assert_accepted(&[
        "type X = { [\"a\"]: number, [\"b\"]: string }",
        "type X = { [string]: number, [\"a\"]: string }",
        "type X = { [\"a\"]: number, [string]: string }",
    ]);
}

#[test]
fn type_pack_after_a_defaulted_parameter_needs_a_default() {
    assert_rejected(&["type X<T = number, U...> = T", "type X<T... = ...number, U...> = T"]);
}

#[test]
fn parenthesized_single_type_is_a_valid_pack_default() {
    assert_accepted(&["type X<T... = (number)> = number"]);
}
