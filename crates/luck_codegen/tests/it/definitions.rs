use luck_allocator::Allocator;
use luck_codegen::LiteralSpelling;
use luck_testgen::definition_fixtures;

use crate::common::assert_reprints_as;

/// `source`, a definition file, compacts to `expected`, which reads back as
/// one.
fn assert_compacts_to(source: &str, expected: &str) {
    let allocator = Allocator::new();
    let parsed = luck_parser::parse_definitions(&allocator, source.as_bytes());
    assert!(parsed.errors.is_empty(), "parse errors for {source:?}: {:?}", parsed.errors);
    let output = luck_codegen::compact(&parsed.chunk, LiteralSpelling::AsWritten);
    assert_eq!(String::from_utf8(output).unwrap(), expected, "input: {source:?}");
    assert_reprints_as(&parsed.chunk, luck_parser::parse_definitions);
}

#[test]
fn every_definition_fixture_compacts_to_a_definition_file_that_reads_back() {
    let fixtures = definition_fixtures();
    assert!(!fixtures.is_empty(), "no definition fixture");
    for fixture in fixtures {
        let allocator = Allocator::new();
        let parsed = luck_parser::parse_definitions(&allocator, fixture.source.as_bytes());
        let name = fixture.relative_path.display();
        assert!(parsed.errors.is_empty(), "{name}: {:?}", parsed.errors);
        assert_reprints_as(&parsed.chunk, luck_parser::parse_definitions);
    }
}

#[test]
fn declarations_compact_to_minimal_whitespace() {
    for (source, expected) in [
        ("declare  foo :  number", "declare foo:number"),
        ("declare class: { [string]: number }", "declare class:{[string]:number}"),
        ("declare function f ( )", "declare function f()"),
        (
            "declare function f< T , U... >( x : T , ... : U... ) : ( T , U... )",
            "declare function f<T,U...>(x:T,...:U...):(T,U...)",
        ),
        (
            "@checked  @[deprecated {use = 'g'}]  declare function f(x: number): number",
            "@checked@[deprecated{use='g'}]declare function f(x:number):number",
        ),
        (
            "declare bit32: { band: @checked (...number) -> number, n: number }",
            "declare bit32:{band:@checked(...number)->number,n:number}",
        ),
        ("declare id: @native <T>(T) -> T", "declare id:@native<T>(T)->T"),
        ("declare extern type Empty with end", "declare extern type Empty with end"),
        ("declare f: @checked ( number ) | string", "declare f:@checked(number)|string"),
        ("declare t: { a: @native (number)? }", "declare t:{a:@native(number)?}"),
        ("@checked declare . field function f()", "@checked declare.field function f()"),
        (
            "@native declare ( 1 ) [ k ] : m ( ) 's' { } <<T>> function f()",
            "@native declare(1)[k]:m()'s'{}<<T>>function f()",
        ),
    ] {
        assert_compacts_to(source, expected);
    }
}

#[test]
fn an_extern_type_s_members_compact_one_after_another() {
    assert_compacts_to(
        "declare extern type Part extends Instance with
    read Name: string
    write Size: number
    read: boolean
    [\"Parent Name\"]: string
    [string]: any
    @checked function IsA(self, className: string): boolean
    function Destroy(self)
    function Fire(self, ...: any): ...any
end",
        "declare extern type Part extends Instance with read Name:string write Size:number read:boolean[\"Parent Name\"]:string[string]:any@checked function IsA(self,className:string):boolean function Destroy(self)function Fire(self,...:any):...any end",
    );
}

#[test]
fn a_declaration_that_ends_in_a_type_takes_a_semicolon_before_a_paren() {
    for (source, expected) in [
        ("declare x: T (a or b)()", "declare x:T;(a or b)()"),
        ("declare function f(): T (a or b)()", "declare function f():T;(a or b)()"),
        ("declare function f() (a or b)()", "declare function f()(a or b)()"),
        ("declare extern type A with end (a or b)()", "declare extern type A with end(a or b)()"),
    ] {
        assert_compacts_to(source, expected);
    }
}
