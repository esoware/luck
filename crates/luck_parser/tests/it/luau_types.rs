use std::ops::Range;

use luck_allocator::{Allocator, Box};
use luck_ast::{Expression, GetSpan as _, Statement, Type, TypeDeclarationValue, TypeField};
use luck_token::{LuaVersion, Span};

use crate::common::{assert_no_errors, parse_luau};

fn alias_type<'ast>(allocator: &'ast Allocator, source: &'ast str) -> Type<'ast> {
    let result = parse_luau(allocator, source);
    assert_no_errors(&result);
    let stmt = result.chunk.block.stmts.into_iter().next().expect("a statement");
    let Statement::TypeDeclaration(type_declaration) = stmt else {
        panic!("expected TypeDeclaration, got {stmt:?}");
    };
    match Box::unbox(type_declaration).value {
        TypeDeclarationValue::Alias(alias) => alias,
        TypeDeclarationValue::TypeFunction(_) => panic!("expected alias form"),
    }
}

#[test]
fn bare_name_is_a_name_type() {
    let allocator = Allocator::new();
    for expected in ["number", "LongTypeNameThatExceedsInlineStringStorage"] {
        let source = format!("type X = {expected}");
        let Type::Name(name) = alias_type(&allocator, &source) else {
            panic!("expected inline Name");
        };
        assert_eq!(name.text, expected);
        assert_eq!(&source[Range::<usize>::from(name.span)], expected);
    }
}

#[test]
fn module_qualified_type_keeps_its_prefix() {
    let allocator = Allocator::new();
    let Type::Named(named) = alias_type(&allocator, "type X = module.Thing") else {
        panic!("expected Named");
    };
    assert!(named.prefix.is_some());
}

#[test]
fn named_type_keeps_its_generic_arguments() {
    let allocator = Allocator::new();
    let Type::Named(named) = alias_type(&allocator, "type X = Map<string, number>") else {
        panic!("expected Named");
    };
    assert_eq!(named.generics.as_ref().expect("generics").args.len(), 2);
}

#[test]
fn nested_generics_split_the_closing_shift_right() {
    let allocator = Allocator::new();
    let Type::Named(named) = alias_type(&allocator, "type X = Map<string, Array<number>>") else {
        panic!("expected Named");
    };
    let args = named.generics.as_ref().expect("generics");
    assert_eq!(args.args.len(), 2);
    let inner = &args.args[1];
    assert!(matches!(inner, Type::Named(nested) if nested.generics.is_some()));
}

#[test]
fn union_keeps_each_member() {
    let allocator = Allocator::new();
    let Type::Union(union) = alias_type(&allocator, "type X = string | number | nil") else {
        panic!("expected Union");
    };
    assert_eq!(union.types.len(), 3);
    assert!(!union.has_leading_pipe);
}

#[test]
fn union_keeps_its_leading_pipe() {
    let allocator = Allocator::new();
    let Type::Union(union) = alias_type(&allocator, "type X = | string | number") else {
        panic!("expected Union");
    };
    assert_eq!(union.types.len(), 2);
    assert!(union.has_leading_pipe);
}

#[test]
fn intersection_keeps_each_member() {
    let allocator = Allocator::new();
    let Type::Intersection(intersection) = alias_type(&allocator, "type X = A & B") else {
        panic!("expected Intersection");
    };
    assert_eq!(intersection.types.len(), 2);
}

#[test]
fn parenthesized_intersection_is_one_union_member() {
    let allocator = Allocator::new();
    // Luau's suffix chain is flat: mixing `&` and `|` without parentheses
    // is a parse error, not a precedence question.
    let result = parse_luau(&allocator, "type X = A & B | C");
    assert!(!result.errors.is_empty(), "mixed union/intersection errors");
    let Type::Union(union) = alias_type(&allocator, "type X = (A & B) | C") else {
        panic!("expected Union at top level");
    };
    assert_eq!(union.types.len(), 2);
    assert!(matches!(union.types.first(), Some(Type::Parenthesized(_))));
}

#[test]
fn question_mark_makes_an_optional() {
    let allocator = Allocator::new();
    assert!(matches!(alias_type(&allocator, "type X = number?"), Type::Optional(_)));
}

#[test]
fn a_run_of_question_marks_is_one_optional() {
    let allocator = Allocator::new();
    for (source, question_count) in
        [("type X = number?", 1), ("type X = number??", 2), ("type X = number? ? ?", 3)]
    {
        let alias = alias_type(&allocator, source);
        assert_eq!(&source[Range::<usize>::from(alias.span())], &source[9..], "{source}");
        let Type::Optional(optional) = alias else {
            panic!("expected Optional, got {alias:?}");
        };
        assert_eq!(optional.question_count, question_count, "{source}");
        assert!(matches!(optional.ty, Type::Name(_)), "{source}");
    }
}

/// `parseReturnType` folds a suffix after `(T, ...U)` onto `T`.
#[test]
fn question_marks_folded_into_a_return_pack_join_its_first_elements_run() {
    let allocator = Allocator::new();
    let result = parse_luau(&allocator, "local function f(): (T?, ...U)?? end");
    assert_no_errors(&result);
    let Statement::LocalFunction(function) = &result.chunk.block.stmts[0] else {
        panic!("expected a local function");
    };
    let Some(Type::Pack(pack)) = function.body.return_type.as_deref() else {
        panic!("expected a pack");
    };
    let Some(Type::Optional(optional)) = pack.types.first() else {
        panic!("expected an optional first element, got {:?}", pack.types.first());
    };
    assert_eq!(optional.question_count, 3);
    assert!(matches!(optional.ty, Type::Name(_)));
}

#[test]
fn negation_syntax_is_an_error_in_luau() {
    let allocator = Allocator::new();
    for source in [
        "return ~1",
        "type X = ~nil",
        "type X = ~nil?",
        "type X = ~~nil",
        "type X = ~string | nil",
        "type X = ~(string | nil)",
    ] {
        let result = parse_luau(&allocator, source);
        assert!(!result.errors.is_empty(), "{source}");
    }
    let result = parse_luau(&allocator, "return 1 ~= 2");
    assert!(result.errors.is_empty(), "{:?}", result.errors);
}

#[test]
fn optional_is_a_union_member() {
    let allocator = Allocator::new();
    let Type::Union(union) = alias_type(&allocator, "type X = number? | string") else {
        panic!("expected Union");
    };
    assert!(matches!(union.types.first(), Some(Type::Optional(_))));
}

#[test]
fn string_singleton_is_a_string_literal_type() {
    let allocator = Allocator::new();
    let Type::Union(union) = alias_type(&allocator, r#"type X = "on" | "off""#) else {
        panic!("expected Union");
    };
    assert!(matches!(union.types.first(), Some(Type::StringLiteral(_))));
}

#[test]
fn true_false_and_nil_are_singleton_types() {
    let allocator = Allocator::new();
    assert!(matches!(alias_type(&allocator, "type X = true"), Type::True(_)));
    assert!(matches!(alias_type(&allocator, "type X = false"), Type::False(_)));
    assert!(matches!(alias_type(&allocator, "type X = nil"), Type::Nil(_)));
}

#[test]
fn table_type_keeps_named_fields() {
    let allocator = Allocator::new();
    let Type::Table(table) = alias_type(&allocator, "type X = { name: string, age: number }")
    else {
        panic!("expected Table");
    };
    assert_eq!(table.fields.len(), 2);
    assert!(matches!(&table.fields[0], TypeField::Named { .. }));
}

#[test]
fn table_type_keeps_its_indexer() {
    let allocator = Allocator::new();
    let Type::Table(table) = alias_type(&allocator, "type X = { [string]: number }") else {
        panic!("expected Table");
    };
    assert!(matches!(&table.fields[0], TypeField::Indexer { .. }));
}

#[test]
fn table_type_of_one_type_is_an_array() {
    let allocator = Allocator::new();
    let Type::Table(table) = alias_type(&allocator, "type X = { number }") else {
        panic!("expected Table");
    };
    assert!(matches!(&table.fields[0], TypeField::Array { .. }));
}

#[test]
fn table_type_fields_keep_their_access() {
    let allocator = Allocator::new();
    let Type::Table(table) =
        alias_type(&allocator, "type X = { read name: string, write count: number }")
    else {
        panic!("expected Table");
    };
    for field in &table.fields {
        assert!(matches!(field, TypeField::Named { access: Some(_), .. }));
    }
}

#[test]
fn read_before_a_colon_is_a_field_name() {
    let allocator = Allocator::new();
    let Type::Table(table) = alias_type(&allocator, "type X = { read: number }") else {
        panic!("expected Table");
    };
    assert!(matches!(&table.fields[0], TypeField::Named { access: None, .. }));
}

#[test]
fn function_type_keeps_its_params() {
    let allocator = Allocator::new();
    let Type::Function(function_type) =
        alias_type(&allocator, "type F = (number, string) -> boolean")
    else {
        panic!("expected Function");
    };
    assert_eq!(function_type.params.len(), 2);
}

#[test]
fn function_type_keeps_its_param_names() {
    let allocator = Allocator::new();
    let Type::Function(function_type) =
        alias_type(&allocator, "type F = (x: number, y: number) -> number")
    else {
        panic!("expected Function");
    };
    assert!(function_type.params.iter().all(|param| param.name.is_some()));
}

#[test]
fn function_type_keeps_its_generics() {
    let allocator = Allocator::new();
    let Type::Function(function_type) = alias_type(&allocator, "type F = <T>(T) -> T") else {
        panic!("expected Function");
    };
    assert!(function_type.generics.is_some());
}

#[test]
fn function_type_ends_in_a_variadic_param() {
    let allocator = Allocator::new();
    let Type::Function(function_type) =
        alias_type(&allocator, "type F = (number, ...string) -> ()")
    else {
        panic!("expected Function");
    };
    let last = function_type.params.last().expect("params");
    assert!(matches!(last.ty, Type::Variadic(_)));
    assert!(matches!(function_type.return_type, Type::Pack(_)));
}

#[test]
fn function_type_returns_a_pack() {
    let allocator = Allocator::new();
    let Type::Function(function_type) = alias_type(&allocator, "type F = () -> (number, string)")
    else {
        panic!("expected Function");
    };
    let Type::Pack(pack) = &function_type.return_type else {
        panic!("expected Pack return");
    };
    assert_eq!(pack.types.len(), 2);
}

#[test]
fn parenthesized_type_is_not_a_pack() {
    let allocator = Allocator::new();
    assert!(matches!(alias_type(&allocator, "type X = (number)"), Type::Parenthesized(_)));
}

#[test]
fn parenthesized_types_preserve_pack_and_function_shapes() {
    let allocator = Allocator::new();
    // Packs stand alone only where one is expected, such as a return.
    for (source, n) in [("type X = () -> ()", 0), ("type X = () -> (number, string)", 2)] {
        let Type::Function(function) = alias_type(&allocator, source) else {
            panic!("expected function for {source}");
        };
        let Type::Pack(pack) = &function.return_type else {
            panic!("expected pack return for {source}");
        };
        assert_eq!(pack.types.len(), n);
    }
    for source in ["type X = (number) -> string", "type X = (value: number) -> string"] {
        let Type::Function(function) = alias_type(&allocator, source) else {
            panic!("expected function for {source}");
        };
        assert_eq!(function.params.len(), 1);
    }
    let Type::Parenthesized(outer) = alias_type(&allocator, "type X = ((number))") else {
        panic!("expected nested parentheses");
    };
    assert!(matches!(outer.ty, Type::Parenthesized(_)));
}

#[test]
fn named_parameter_outside_a_function_type_is_an_error() {
    let allocator = Allocator::new();
    let parsed = parse_luau(&allocator, "type X = (value: number)");
    assert_eq!(parsed.errors.len(), 1, "{:?}", parsed.errors);
    assert_eq!(parsed.errors[0].span, Some(Span::new(10, 15)));
    assert_eq!(parsed.errors[0].message, "named parameters are only valid in function types");
}

#[test]
fn typeof_type_keeps_its_expression() {
    let allocator = Allocator::new();
    let Type::Typeof(typeof_type) = alias_type(&allocator, "type X = typeof(game.Workspace)")
    else {
        panic!("expected Typeof");
    };
    assert!(matches!(typeof_type.expr, Expression::Suffixed(_)));
}

#[test]
fn generic_pack_is_a_return_type() {
    let allocator = Allocator::new();
    let Type::Function(function_type) = alias_type(&allocator, "type F = <T...>(T...) -> T...")
    else {
        panic!("expected Function");
    };
    assert!(matches!(function_type.return_type, Type::GenericPack(_)));
}

#[test]
fn type_alias_generics_keep_their_defaults() {
    let allocator = Allocator::new();
    let result = parse_luau(&allocator, "type Foo<T = string, S... = ...string> = T");
    assert_no_errors(&result);
    let Statement::TypeDeclaration(type_declaration) = &result.chunk.block.stmts[0] else {
        panic!("expected TypeDeclaration");
    };
    let generics = type_declaration.generics.as_ref().expect("generics");
    assert_eq!(generics.params.len(), 2);
    assert!(generics.params.iter().all(|param| param.default.is_some()));
    assert!(generics.params.last().expect("pack param").is_pack);
}

#[test]
fn local_keeps_its_annotation() {
    let allocator = Allocator::new();
    let result = parse_luau(&allocator, "local x: number = 1");
    assert_no_errors(&result);
    let Statement::LocalAssignment(local) = &result.chunk.block.stmts[0] else {
        panic!("expected LocalAssignment");
    };
    let name = local.names.first().expect("name");
    assert!(name.type_annotation.is_some());
}

#[test]
fn each_local_keeps_its_own_annotation() {
    let allocator = Allocator::new();
    let result = parse_luau(&allocator, "local a: number, b, c: string? = 1, 2, \"x\"");
    assert_no_errors(&result);
    let Statement::LocalAssignment(local) = &result.chunk.block.stmts[0] else {
        panic!("expected LocalAssignment");
    };
    let annotations: Vec<bool> =
        local.names.iter().map(|name| name.type_annotation.is_some()).collect();
    assert_eq!(annotations, vec![true, false, true]);
}

#[test]
fn numeric_for_keeps_its_annotation() {
    let allocator = Allocator::new();
    let result = parse_luau(&allocator, "for i: number = 1, 10 do end");
    assert_no_errors(&result);
    let Statement::NumericFor(numeric_for) = &result.chunk.block.stmts[0] else {
        panic!("expected NumericFor");
    };
    assert!(numeric_for.type_annotation.is_some());
}

#[test]
fn generic_for_keeps_each_annotation() {
    let allocator = Allocator::new();
    let result = parse_luau(&allocator, "for k: string, v: number in pairs(t) do end");
    assert_no_errors(&result);
    let Statement::GenericFor(generic_for) = &result.chunk.block.stmts[0] else {
        panic!("expected GenericFor");
    };
    assert!(generic_for.names.iter().all(|binding| binding.type_annotation.is_some()));
}

#[test]
fn generic_function_keeps_its_generics_and_annotations() {
    let allocator = Allocator::new();
    let result = parse_luau(
        &allocator,
        "\
local function map<T, U>(items: {T}, fn: (T) -> U): {U}
\treturn {}
end",
    );
    assert_no_errors(&result);
    let Statement::LocalFunction(local_function) = &result.chunk.block.stmts[0] else {
        panic!("expected LocalFunction");
    };
    assert!(local_function.body.generics.is_some());
    assert!(local_function.body.return_type.is_some());
    assert!(local_function.body.params.iter().all(|param| param.type_annotation.is_some()));
}

#[test]
fn cast_takes_an_optional_table_type() {
    let allocator = Allocator::new();
    let result = parse_luau(&allocator, "local x = (y :: { [string]: number }?)");
    assert_no_errors(&result);
}

#[test]
fn method_call_colon_starts_no_annotation() {
    let allocator = Allocator::new();
    // The `:` in `obj:method()` must not start a type annotation.
    let result = parse_luau(&allocator, "obj:method(1)");
    assert_no_errors(&result);
}

#[test]
fn type_annotations_rejected_outside_luau() {
    let allocator = Allocator::new();
    let result = luck_parser::parse(&allocator, b"local x: number = 1", LuaVersion::Lua54);
    assert_ne!(result.errors, Vec::<luck_diagnostics::Diagnostic>::new());
}

#[test]
fn mixed_union_intersection_rejected() {
    let allocator = Allocator::new();
    for source in ["type X = A | B & C", "type X = A & B | C", "type X = A & B?", "type X = A? & B"]
    {
        let result = parse_luau(&allocator, source);
        assert!(!result.errors.is_empty(), "must reject mixing: {source}");
    }
    for source in
        ["type X = (A & B) | C", "type X = A & (B?)", "type X = A? | B", "type X = A | B | C?"]
    {
        let result = parse_luau(&allocator, source);
        assert!(result.errors.is_empty(), "parenthesized forms parse: {source}");
    }
}

#[test]
fn table_type_holds_one_array_type_or_at_most_one_indexer() {
    let allocator = Allocator::new();
    // TableType ::= '{' Type '}' | '{' [PropList] '}'
    let invalid_sources = [
        "type T = {number, x: string}",
        "type T = {x: string, number}",
        "type T = {number, string}",
        "type T = {[string]: number, [number]: string}",
    ];
    for source in invalid_sources {
        let result = parse_luau(&allocator, source);
        assert!(!result.errors.is_empty(), "must reject: {source}");
    }
    let valid_sources = [
        "type T = {number}",
        "type T = {x: string, y: number}",
        "type T = {[string]: number, x: boolean}",
        "type T = {x: boolean, [string]: number}",
    ];
    for source in valid_sources {
        let result = parse_luau(&allocator, source);
        assert!(result.errors.is_empty(), "must accept: {source}");
    }
}

#[test]
fn generic_lists_follow_luaus_ordering_and_default_rules() {
    let allocator = Allocator::new();
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
        let result = parse_luau(&allocator, source);
        assert!(!result.errors.is_empty(), "must reject: {source}");
    }
    let valid_sources = [
        "type X<T, U...> = T",
        "type X<T, U = number> = U",
        "type X<T... = ...number> = number",
        "type X<T... = (string, number)> = number",
    ];
    for source in valid_sources {
        let result = parse_luau(&allocator, source);
        assert!(result.errors.is_empty(), "must accept: {source}");
    }
}

fn assert_rejected(sources: &[&str]) {
    for source in sources {
        let allocator = Allocator::new();
        let result = parse_luau(&allocator, source);
        assert!(!result.errors.is_empty(), "luau rejects {source}");
    }
}

fn assert_accepted(sources: &[&str]) {
    for source in sources {
        let allocator = Allocator::new();
        let result = parse_luau(&allocator, source);
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
