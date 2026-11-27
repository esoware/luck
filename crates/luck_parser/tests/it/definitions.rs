use luck_allocator::Allocator;
use luck_ast::{
    Declaration, DeclaredExternType, DeclaredFunction, Expression, ExternTypeMember,
    MAX_NESTING_DEPTH, Statement, Type, TypeField, TypeFieldAccess, block_nesting_depth,
};
use luck_parser::{ParseResult, parse, parse_definitions, parse_definitions_with_tokens};
use luck_testgen::{definition_fixtures, nested, with_the_smallest_stack};
use luck_token::Span;

use crate::common::{assert_no_errors, chain_shape};

fn parse_str<'ast>(allocator: &'ast Allocator, source: &'ast str) -> ParseResult<'ast> {
    parse_definitions(allocator, source.as_bytes())
}

/// The only statement of a definition file that parsed cleanly.
fn only_stmt<'tree, 'ast>(result: &'tree ParseResult<'ast>) -> &'tree Statement<'ast> {
    assert_no_errors(result);
    let [stmt] = &*result.chunk.stmts else {
        panic!("expected one statement, got {:?}", result.chunk.stmts);
    };
    stmt
}

/// The only statement of a definition file that parsed cleanly, a `declare`.
fn only_declaration<'tree, 'ast>(result: &'tree ParseResult<'ast>) -> &'tree Declaration<'ast> {
    let Statement::Declaration(declaration) = only_stmt(result) else {
        panic!("expected a declaration, got {:?}", result.chunk.stmts);
    };
    declaration
}

fn declared_function<'tree, 'ast>(
    result: &'tree ParseResult<'ast>,
) -> &'tree DeclaredFunction<'ast> {
    let Declaration::Function(function) = only_declaration(result) else {
        panic!("expected a declared function, got {:?}", result.chunk.stmts);
    };
    function
}

fn declared_extern_type<'tree, 'ast>(
    result: &'tree ParseResult<'ast>,
) -> &'tree DeclaredExternType<'ast> {
    let Declaration::ExternType(extern_type) = only_declaration(result) else {
        panic!("expected a declared extern type, got {:?}", result.chunk.stmts);
    };
    extern_type
}

/// The type a lone `declare name: T` gives.
fn declared_type<'tree, 'ast>(result: &'tree ParseResult<'ast>) -> &'tree Type<'ast> {
    let Declaration::Global(global) = only_declaration(result) else {
        panic!("expected a declared global, got {:?}", result.chunk.stmts);
    };
    &global.ty
}

fn attribute_names<'ast>(function: &DeclaredFunction<'ast>) -> Vec<&'ast str> {
    function.attributes.iter().map(|attribute| attribute.name.text.as_str()).collect()
}

/// The names of the attributes on `ty`, parentheses that must hold some.
fn attributed<'ast>(ty: &Type<'ast>) -> Vec<&'ast str> {
    let Type::Parenthesized(parenthesized) = ty else {
        panic!("expected attributed parentheses, got {ty:?}");
    };
    assert!(!parenthesized.attributes.is_empty(), "no attributes on {ty:?}");
    parenthesized.attributes.iter().map(|attribute| attribute.name.text.as_str()).collect()
}

fn text(source: &str, span: Span) -> &str {
    &source[span.start as usize..span.end as usize]
}

#[test]
fn every_definition_fixture_parses_cleanly_only_as_a_definition_file() {
    let fixtures = definition_fixtures();
    assert!(!fixtures.is_empty(), "no definition fixture");
    for fixture in fixtures {
        let allocator = Allocator::new();
        let source = fixture.source.as_bytes();
        let name = fixture.relative_path.display();
        let definitions = parse_definitions(&allocator, source);
        assert!(definitions.errors.is_empty(), "{name}: {:?}", definitions.errors);
        assert!(!parse(&allocator, source).errors.is_empty(), "{name} parses as code");
        let tokens = parse_definitions_with_tokens(&allocator, source).tokens;
        assert_eq!(tokens, luck_lexer::lex(source).tokens, "{name}");
        let built = block_nesting_depth(&definitions.chunk);
        assert_eq!(definitions.nesting_depth, built, "{name}");
    }
}

#[test]
fn a_declared_global_holds_its_name_and_type() {
    let allocator = Allocator::new();
    let source = "declare foo: number";
    let result = parse_str(&allocator, source);
    let Declaration::Global(global) = only_declaration(&result) else {
        panic!("expected a declared global");
    };
    assert_eq!(global.name.text, "foo");
    assert_eq!(text(source, global.name.span), "foo");
    assert!(matches!(&global.ty, Type::Name(name) if name.text == "number"));
    assert_eq!(text(source, global.span), source);
}

#[test]
fn a_global_may_take_a_contextual_keyword_s_name() {
    let allocator = Allocator::new();
    for name in ["class", "type", "typeof", "export", "read", "continue", "extends", "with"] {
        let source = format!("declare {name}: {{ x: number }}");
        let result = parse_definitions(&allocator, source.as_bytes());
        let Declaration::Global(global) = only_declaration(&result) else {
            panic!("expected a declared global in {source:?}");
        };
        assert_eq!(global.name.text, name);
        assert!(matches!(global.ty, Type::Table(_)), "{source}");
    }
}

#[test]
fn a_declared_function_holds_its_signature() {
    let allocator = Allocator::new();
    let source = "declare function bar(x: number, y: string?): string";
    let result = parse_str(&allocator, source);
    let function = declared_function(&result);
    assert_eq!(function.name.text, "bar");
    let params: Vec<_> = function.params.iter().map(|param| param.name.text.as_str()).collect();
    assert_eq!(params, ["x", "y"]);
    assert!(function.params.iter().all(|param| param.type_annotation.is_some()));
    assert!(function.vararg.is_none() && function.generics.is_none());
    assert!(
        matches!(function.return_type.as_deref(), Some(Type::Name(name)) if name.text == "string")
    );
    assert_eq!(text(source, function.span), source);

    let variadic_result = parse_str(&allocator, "declare function var(...: any)");
    let variadic = declared_function(&variadic_result);
    assert!(variadic.params.is_empty() && variadic.return_type.is_none());
    let vararg = variadic.vararg.as_deref().expect("a vararg");
    assert!(
        matches!(vararg.type_annotation.as_deref(), Some(Type::Name(name)) if name.text == "any")
    );
}

#[test]
fn a_declared_function_takes_generics_and_packs() {
    let allocator = Allocator::new();
    let result =
        parse_str(&allocator, "declare function f<a, b, c...>(x: a, ...: c...): (b, c...)");
    let function = declared_function(&result);
    let generics = function.generics.as_deref().expect("generics");
    let packs: Vec<_> =
        generics.params.iter().map(|param| (param.name.text.as_str(), param.is_pack)).collect();
    assert_eq!(packs, [("a", false), ("b", false), ("c", true)]);
    let vararg = function.vararg.as_deref().expect("a vararg");
    assert!(matches!(vararg.type_annotation.as_deref(), Some(Type::GenericPack(_))));
    assert!(matches!(function.return_type.as_deref(), Some(Type::Pack(_))));
}

#[test]
fn attributes_before_a_declared_function_are_its_own() {
    let allocator = Allocator::new();
    let source = "@checked declare function abs(n: number): number";
    let result = parse_str(&allocator, source);
    let function = declared_function(&result);
    assert_eq!(attribute_names(function), ["checked"]);
    assert_eq!(text(source, function.attributes[0].span), "@checked");
    assert_eq!(text(source, function.span), source);

    let listed = "@native @[deprecated {use = \"new\", reason = \"old\"}] declare function f()";
    let listed_result = parse_str(&allocator, listed);
    let deprecated = declared_function(&listed_result);
    assert_eq!(attribute_names(deprecated), ["native", "deprecated"]);
    let args = deprecated.attributes[1].args.as_deref().expect("arguments");
    assert!(matches!(args, [Expression::TableConstructor(table)] if table.fields.len() == 2));
}

#[test]
fn a_function_type_in_a_declared_type_takes_attributes() {
    let allocator = Allocator::new();
    for (source, generic_count) in [
        ("declare abs: @checked (n: number) -> number", 0),
        ("declare id: @native @checked <T>(T) -> T", 1),
        ("declare f: @[deprecated {use = \"g\"}] () -> ()", 0),
    ] {
        let result = parse_str(&allocator, source);
        let Type::Function(function) = declared_type(&result) else {
            panic!("expected a function type in {source:?}");
        };
        assert!(!function.attributes.is_empty(), "{source}");
        assert_eq!(function.generics.as_ref().map_or(0, |list| list.params.len()), generic_count);
        assert_eq!(text(source, function.span), &source[source.find('@').unwrap()..]);
    }
}

#[test]
fn a_named_field_of_a_declared_table_type_takes_attributes_however_deep() {
    let allocator = Allocator::new();
    let source = "\
declare bit32: {
    band: @checked (...number) -> number,
    read nested: { deeper: { f: @checked () -> () } },
}";
    let result = parse_str(&allocator, source);
    let Type::Table(table) = declared_type(&result) else {
        panic!("expected a table type");
    };
    let [TypeField::Named { value: band, .. }, TypeField::Named { access, value: nested, .. }] =
        &*table.fields
    else {
        panic!("expected two named fields, got {:?}", table.fields);
    };
    assert!(matches!(band, Type::Function(function) if function.attributes.len() == 1));
    assert_eq!(*access, Some(TypeFieldAccess::Read));
    let Type::Table(nested) = nested else {
        panic!("expected a table type");
    };
    let [TypeField::Named { value: Type::Table(deeper), .. }] = &*nested.fields else {
        panic!("expected a nested table type");
    };
    let [TypeField::Named { value: Type::Function(function), .. }] = &*deeper.fields else {
        panic!("expected a function type");
    };
    assert_eq!(function.attributes[0].name.text, "checked");
}

#[test]
fn an_extern_type_holds_its_members() {
    let allocator = Allocator::new();
    let source = "\
declare extern type Part extends Instance with
    Name: string
    read ClassName: string
    write Archivable: boolean
    read: number
    [\"Parent Name\"]: string
    [string]: any
    function GetChildren(self): { Instance }
    @checked function FindFirstChild(self, name: string, recursive: boolean?): Instance?
    function IsA(self, className: string): boolean
    function IsA(self, className: \"Part\"): true
    function Fire(self, ...: any)
end";
    let result = parse_str(&allocator, source);
    let extern_type = declared_extern_type(&result);
    assert_eq!(extern_type.name.text, "Part");
    assert_eq!(extern_type.supertype.map(|supertype| supertype.text.as_str()), Some("Instance"));
    assert_eq!(text(source, extern_type.span), source);
    let mut fields = Vec::new();
    let mut methods = Vec::new();
    for member in &extern_type.members {
        match member {
            ExternTypeMember::Field(TypeField::Named { access, name, .. }) => {
                fields.push(format!("{access:?} {}", name.text));
            }
            ExternTypeMember::Field(TypeField::Indexer { access: None, key, .. }) => {
                fields.push(format!("[{}]", text(source, luck_ast::GetSpan::span(key))));
            }
            ExternTypeMember::Field(field) => panic!("unexpected field {field:?}"),
            ExternTypeMember::Method(method) => {
                assert_eq!(method.params[0].name.text, "self");
                methods.push(format!("{} {:?}", method.name.text, attribute_names(method)));
            }
        }
    }
    assert_eq!(
        fields,
        [
            "None Name",
            "Some(Read) ClassName",
            "Some(Write) Archivable",
            "None read",
            "[\"Parent Name\"]",
            "[string]",
        ]
    );
    assert_eq!(
        methods,
        ["GetChildren []", "FindFirstChild [\"checked\"]", "IsA []", "IsA []", "Fire []",]
    );
}

#[test]
fn an_extern_type_may_hold_nothing() {
    let allocator = Allocator::new();
    let spread = "\
declare extern type Empty with
end";
    for source in ["declare extern type Empty with end", spread] {
        let result = parse_str(&allocator, source);
        let extern_type = declared_extern_type(&result);
        assert!(extern_type.members.is_empty() && extern_type.supertype.is_none());
    }
}

#[test]
fn a_definition_file_takes_ordinary_statements_too() {
    let allocator = Allocator::new();
    let result = parse_str(
        &allocator,
        "\
export type Callback = (number) -> ()
type Pair<T> = { T }
local declare = 1
declare = 2
declare.field = 3
declare += 4
declare(5)
declare \"six\"
declare { 7 }
declare:method()
do declare nested: number end
function f() declare inner: string end",
    );
    assert_no_errors(&result);
    let kinds: Vec<_> = result
        .chunk
        .stmts
        .iter()
        .map(|stmt| format!("{stmt:?}").split('(').next().unwrap_or_default().to_owned())
        .collect();
    assert_eq!(
        kinds,
        [
            "TypeDeclaration",
            "TypeDeclaration",
            "LocalAssignment",
            "Assignment",
            "Assignment",
            "CompoundAssignment",
            "FunctionCall",
            "FunctionCall",
            "FunctionCall",
            "FunctionCall",
            "DoBlock",
            "FunctionDecl",
        ]
    );
    let nested = match &result.chunk.stmts[10..] {
        [Statement::DoBlock(block), Statement::FunctionDecl(function)] => {
            [&block.block.stmts[..], &function.body.block.stmts[..]]
        }
        stmts => panic!("expected a `do` and a function, got {stmts:?}"),
    };
    let is_global = |stmts: &&[Statement<'_>]| matches!(stmts, [Statement::Declaration(declaration)] if matches!(**declaration, Declaration::Global(_)));
    assert!(nested.iter().all(is_global));
}

#[test]
fn declaration_syntax_is_an_error_in_code() {
    let allocator = Allocator::new();
    for source in [
        "declare foo: number",
        "declare function f(): ()",
        "@checked declare function f(): ()",
        "declare extern type Foo with end",
    ] {
        let result = parse(&allocator, source.as_bytes());
        assert!(!result.errors.is_empty(), "{source} parses as code");
    }
    let result = parse(&allocator, b"local declare = 1 declare = 2 declare(3)");
    assert_no_errors(&result);
}

/// Each source holds one mistake, which Luau's parser rejects too, and the
/// first error luck reports for it.
#[test]
fn what_luau_rejects_in_a_definition_file_is_an_error() {
    for (source, message) in [
        ("declare function foo(x)", "all declaration parameters must be annotated"),
        ("declare function foo(...)", "all declaration parameters must be annotated"),
        ("declare function a.b(): ()", "expected `(`, found `.`"),
        (
            "declare function f<T = number>(x: T)",
            "default type parameters are only allowed in type alias declarations",
        ),
        ("declare foo", "expected `:`, found end of file"),
        ("declare class Foo prop: number end", "expected `:`, found identifier `Foo`"),
        (
            "declare 5",
            "expected a name, `function`, or `extern type` after `declare`, found number `5`",
        ),
        (
            "declare",
            "expected a name, `function`, or `extern type` after `declare`, found end of file",
        ),
        (
            "@checked declare foo: number",
            "expected `function` after attributes, found identifier `foo`",
        ),
        (
            "@checked declare extern type Foo with end",
            "expected `function` after attributes, found identifier `extern`",
        ),
        ("@cool declare function f()", "invalid attribute `@cool`"),
        ("@checked @checked declare function f()", "duplicate attribute `@checked`"),
        ("declare extern Foo with end", "expected `type` after `extern`, found identifier `Foo`"),
        ("declare extern: number", "expected `type` after `extern`, found `:`"),
        (
            "declare extern type Foo prop: number end",
            "expected `with` before the extern type's members, found identifier `prop`",
        ),
        (
            "declare extern type Foo<T> with end",
            "expected `with` before the extern type's members, found `<`",
        ),
        (
            "declare extern type Foo with function method(foo: number) end",
            "`self` must be present as the unannotated first parameter",
        ),
        (
            "declare extern type Foo with function method() end",
            "`self` must be present as the unannotated first parameter",
        ),
        (
            "declare extern type Foo with function method(self: Foo) end",
            "`self` must be present as the unannotated first parameter",
        ),
        (
            "declare extern type Foo with function method(self, foo) end",
            "all declaration parameters aside from `self` must be annotated",
        ),
        (
            "declare extern type Foo with function a(self, ...) end",
            "all declaration parameters aside from `self` must be annotated",
        ),
        ("declare extern type Foo with function m<T>(self) end", "expected `(`, found `<`"),
        (
            "declare extern type Foo with [string]: number [number]: number end",
            "cannot have more than one indexer on an extern type",
        ),
        (
            "declare extern type Foo with [\"a\\0\"]: number end",
            "string literal contains malformed escape sequence or `\\0`",
        ),
        (
            "declare extern type Foo with a: number, end",
            "expected a property, an indexer, or a method, found `,`",
        ),
        (
            "declare extern type Foo with a: number; end",
            "expected a property, an indexer, or a method, found `;`",
        ),
        (
            "declare extern type Foo with @checked prop: number end",
            "expected `function` after attributes, found identifier `prop`",
        ),
        (
            "declare extern type Foo with foo x: number end",
            "a property's access must be `read` or `write`",
        ),
        (
            "declare extern type Foo with read [string]: number end",
            "expected a property, an indexer, or a method, found `[`",
        ),
        (
            "declare extern type Foo with x: number",
            "expected `end` to close `declare`, found end of file",
        ),
        ("declare extern type Foo with f: @checked () -> () end", "expected type, found `@`"),
        (
            "declare bit32: { band: @checked number }",
            "expected `(` after attributes, found identifier `number`",
        ),
        ("declare f: @checked (number, string)", "expected `->`, found end of file"),
        ("declare f: @checked ()", "expected `->` after `()`, did you mean `nil`?"),
        ("declare f: @checked (...number)", "expected `->`, found end of file"),
        ("declare f: @checked (x: number)", "named parameters are only valid in function types"),
        ("declare f: @checked <T>(T)", "expected `->` after generic function type parameters"),
        ("declare f: @checked (number): string", "unexpected token `:`"),
        ("@checked declare + 1 function f()", "expected `function` after attributes, found `+`"),
        ("@checked declare.x = 1", "expected `function` after attributes, found `=`"),
        ("declare f: number | @checked () -> ()", "expected type, found `@`"),
        ("declare f: (@checked () -> ())", "expected type, found `@`"),
        ("declare f: { [string]: @checked () -> () }", "expected type, found `@`"),
        ("declare f: { @checked () -> () }", "expected type, found `@`"),
        ("declare f: () -> @checked () -> ()", "expected type, found `@`"),
        ("type T = { f: @checked () -> () }", "expected type, found `@`"),
        ("local f: @checked () -> ()", "expected type, found `@`"),
    ] {
        let allocator = Allocator::new();
        let result = parse_str(&allocator, source);
        let first = result.errors.first().map(|error| &*error.message);
        assert_eq!(first, Some(message), "{source}: {:?}", result.errors);
        assert!(result.errors.iter().all(|error| error.code == "E008"), "{source}");
    }
}

#[test]
fn a_second_indexer_points_at_the_first() {
    let allocator = Allocator::new();
    let source = "declare extern type Foo with [string]: number [number]: number end";
    let result = parse_str(&allocator, source);
    let [error] = &*result.errors else {
        panic!("expected one error, got {:?}", result.errors);
    };
    assert_eq!(text(source, error.span.expect("a span")), "[number]: number");
    assert_eq!(text(source, error.labels[0].span), "[string]: number");
}

#[test]
fn a_bad_member_reports_once_and_the_parse_goes_on() {
    let allocator = Allocator::new();
    let result = parse_str(
        &allocator,
        "\
declare extern type Foo
    a: number,
    b: string
end
declare extern type Bar
    c: number
end
declare after: number",
    );
    let messages: Vec<_> = result.errors.iter().map(|error| &*error.message).collect();
    assert_eq!(
        messages,
        [
            "expected `with` before the extern type's members, found identifier `a`",
            "expected `with` before the extern type's members, found identifier `c`",
        ]
    );
    assert!(matches!(result.chunk.stmts.last(), Some(Statement::Declaration(_))));
}

#[test]
fn counts_the_depth_of_each_declaration_as_the_tree_measures_it() {
    for source in [
        "declare f: { a: @[deprecated {use = 'b'}] ({c}) -> ({d}) }?",
        "@[deprecated {use = 'a'}] declare function f<T>(a: {T}, ...: {T}): ({T}, ...{T})",
        "declare extern type A with b: {c} [{d}]: {e} @[deprecated {use = 'f'}] function g(self, h: {i}): {j} end",
        "do declare extern type A with function b(self) end end",
        "declare f: { a: @[deprecated {use = 'b'}] ({c})? }",
        "@native declare.a[{b}]({c}) function f()",
    ] {
        let allocator = Allocator::new();
        let result = parse_str(&allocator, source);
        assert_no_errors(&result);
        assert_eq!(result.nesting_depth, block_nesting_depth(&result.chunk), "{source}");
    }
}

#[test]
fn a_declared_type_nests_to_the_cap_on_the_smallest_stack() {
    // The chunk's block and the innermost function type take a level each.
    let fits = MAX_NESTING_DEPTH - 2;
    for (tables, is_past_the_cap) in [(fits, false), (fits + 1, true)] {
        let source =
            format!("declare x: {}", nested("{ a: ", tables, "@checked () -> number", " }"));
        with_the_smallest_stack(|| {
            let allocator = Allocator::new();
            let result = parse_definitions(&allocator, source.as_bytes());
            if is_past_the_cap {
                let [error] = &*result.errors else {
                    panic!("expected one error, got {:?}", result.errors.first());
                };
                assert!(error.message.contains("resource limit"), "{error:?}");
            } else {
                assert_no_errors(&result);
                assert_eq!(block_nesting_depth(&result.chunk), MAX_NESTING_DEPTH);
            }
        });
    }
}

#[test]
fn attributes_before_a_lone_parenthesized_type_stay_on_it() {
    let allocator = Allocator::new();
    let source = "declare f: @native @checked ((boolean))";
    let result = parse_str(&allocator, source);
    let ty = declared_type(&result);
    assert_eq!(attributed(ty), ["native", "checked"]);
    assert_eq!(text(source, luck_ast::GetSpan::span(ty)), "@native @checked ((boolean))");
    let Type::Parenthesized(outer) = ty else {
        panic!("expected parentheses");
    };
    assert!(matches!(&outer.ty, Type::Parenthesized(inner) if inner.attributes.is_empty()));

    let optional_result = parse_str(&allocator, "declare f: @checked (string)?");
    let Type::Optional(optional) = declared_type(&optional_result) else {
        panic!("expected an optional type");
    };
    assert_eq!(attributed(&optional.ty), ["checked"]);

    let union_result = parse_str(&allocator, "declare f: @checked (number) | string");
    let Type::Union(union) = declared_type(&union_result) else {
        panic!("expected a union");
    };
    assert_eq!(attributed(&union.types[0]), ["checked"]);

    let table_source = "declare t: { a: @checked (number), b: { c: @deprecated (boolean) } }";
    let table_result = parse_str(&allocator, table_source);
    let Type::Table(table) = declared_type(&table_result) else {
        panic!("expected a table type");
    };
    let [TypeField::Named { value: a, .. }, TypeField::Named { value: Type::Table(b), .. }] =
        &*table.fields
    else {
        panic!("expected two named fields, got {:?}", table.fields);
    };
    assert_eq!(attributed(a), ["checked"]);
    let [TypeField::Named { value: c, .. }] = &*b.fields else {
        panic!("expected one named field, got {:?}", b.fields);
    };
    assert_eq!(attributed(c), ["deprecated"]);
}

#[test]
fn an_attributed_declare_keeps_the_chain_luau_reads_and_drops() {
    let allocator = Allocator::new();
    for (source, shape) in [
        ("@checked declare.field function f()", "declare.field"),
        ("@checked declare:method() function f()", "declare:method()"),
        ("@checked declare(1) function f()", "declare()"),
        ("@checked declare[key] function f()", "declare[]"),
        ("@checked declare \"s\" function f()", "declare()"),
        ("@checked declare {} function f()", "declare()"),
        ("@checked declare<<number>> function f()", "declare<<>>"),
        ("@native declare.a[1]:b() \"c\" { d } function f(): ()", "declare.a[]:b()()()"),
    ] {
        let result = parse_str(&allocator, source);
        let function = declared_function(&result);
        let chain = function.declare_chain.as_deref().expect("a chain");
        assert_eq!(chain_shape(chain), shape, "{source}");
        assert_eq!(function.name.text, "f", "{source}");
        assert_eq!(text(source, function.span), source);
    }
    let result = parse_str(&allocator, "@checked declare function f()");
    assert!(declared_function(&result).declare_chain.is_none());
}

#[test]
fn without_attributes_a_suffix_makes_declare_an_ordinary_name() {
    let allocator = Allocator::new();
    for source in [
        "declare.x function f()",
        "declare:m() function f()",
        "declare(1) function f(): ()",
        "declare[1] x: number",
        "declare \"s\" function f()",
    ] {
        let result = parse_str(&allocator, source);
        assert!(!result.errors.is_empty(), "{source} parses");
        let is_declaration = |stmt: &Statement<'_>| matches!(stmt, Statement::Declaration(_));
        assert!(!result.chunk.stmts.iter().any(is_declaration), "{source}");
    }
    // Luau's `getIdentifier` names a local too, so a local `declare` still
    // opens a declaration.
    let result = parse_str(
        &allocator,
        "local declare = 1
declare x: number",
    );
    assert_no_errors(&result);
    assert!(matches!(
        &result.chunk.stmts[..],
        [Statement::LocalAssignment(_), Statement::Declaration(_)]
    ));
}
