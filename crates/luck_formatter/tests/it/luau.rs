use luck_formatter::FormatOptions;
use luck_token::LuaVersion;

use crate::common::{assert_format_with, format_once};

fn assert_luau(input: &str, expected: &str) {
    assert_format_with(input, expected, LuaVersion::Luau, &FormatOptions::default());
}

#[test]
fn paren_type_instantiation_stmt_keeps_guard() {
    // Without the `;` this is a parse error: Luau rejects a call whose `(`
    // opens a new line.
    assert_luau("f()\n;(g or h)<<T>>()\n", "f()\n;(g or h)<<T>>()\n");
    assert_luau("f()\n;(g or h)<<T>>.x = 1\n", "f()\n;(g or h)<<T>>.x = 1\n");
}

#[test]
fn param_type_annotation_simple() {
    assert_luau("local function f(x: number)\nend\n", "local function f(x: number)\nend\n");
}

#[test]
fn param_type_annotation_multiple() {
    assert_luau(
        "local function f(x: number, y: string)\nend\n",
        "local function f(x: number, y: string)\nend\n",
    );
}

#[test]
fn return_type_annotation() {
    assert_luau(
        "local function f(x: number): string\nend\n",
        "local function f(x: number): string\nend\n",
    );
}

#[test]
fn type_cast() {
    assert_luau("local x = y :: number\n", "local x = y :: number\n");
}

#[test]
fn type_declaration_table() {
    assert_luau(
        "type Foo = { name: string, age: number }\n",
        "type Foo = { name: string, age: number }\n",
    );
}

#[test]
fn type_declaration_table_normalizes_whitespace() {
    assert_luau(
        "type Foo = {name:string,age:number}\n",
        "type Foo = { name: string, age: number }\n",
    );
}

#[test]
fn type_declaration_function_type() {
    assert_luau(
        "type Callback = (number, string) -> boolean\n",
        "type Callback = (number, string) -> boolean\n",
    );
}

#[test]
fn type_declaration_union() {
    assert_luau(
        "type StringOrNumber = string | number\n",
        "type StringOrNumber = string | number\n",
    );
}

#[test]
fn type_declaration_union_normalizes_whitespace() {
    assert_luau("type StringOrNumber = string|number\n", "type StringOrNumber = string | number\n");
}

#[test]
fn type_declaration_intersection() {
    assert_luau("type Both = Readable & Writable\n", "type Both = Readable & Writable\n");
}

#[test]
fn type_declaration_generic() {
    assert_luau("type Map = Map<string, number>\n", "type Map = Map<string, number>\n");
}

#[test]
fn type_declaration_nested_generic() {
    assert_luau(
        "type Nested = Map<string, Array<number>>\n",
        "type Nested = Map<string, Array<number>>\n",
    );
}

#[test]
fn type_annotation_optional() {
    assert_luau("local function f(x: number?)\nend\n", "local function f(x: number?)\nend\n");
}

#[test]
fn type_declaration_qualified() {
    assert_luau("type Alias = module.Type\n", "type Alias = module.Type\n");
}

#[test]
fn vararg_type_annotation() {
    assert_luau("local function f(...: number)\nend\n", "local function f(...: number)\nend\n");
}

#[test]
fn export_type_table() {
    assert_luau(
        "export type Config = { width: number, height: number }\n",
        "export type Config = { width: number, height: number }\n",
    );
}

#[test]
fn type_declaration_with_generics() {
    assert_luau("type Container<T> = { value: T }\n", "type Container<T> = { value: T }\n");
}

// Luau's parlist grammar (`bindinglist [',' '...'] | '...'`) has no trailing
// comma, so a broken parameter list must not gain one.
#[test]
fn luau_params_break_without_trailing_comma() {
    let options = FormatOptions { line_width: 30, ..FormatOptions::default() };
    let input = "local function f(aaa: number, bbb: string, ccc: boolean)\nend\n";
    let result = luck_formatter::format(input, LuaVersion::Luau, &options);
    assert_eq!(result.errors, Vec::<luck_token::SourceError>::new());
    assert!(
        !result.output.contains("boolean,"),
        "params must not gain a trailing comma: {}",
        result.output,
    );
    let reparsed = luck_parser::parse(&result.output, LuaVersion::Luau);
    assert!(
        reparsed.errors.is_empty(),
        "formatted params failed to re-parse: {}\nerrors: {:?}",
        result.output,
        reparsed.errors,
    );
}

#[test]
fn interpolated_expr_starting_with_table_gets_space() {
    // `{{` is a parse error in Luau; the formatter separates the
    // interpolation opener from a leading table constructor.
    assert_luau("return `list { {1, 2} } tail`\n", "return `list { { 1, 2 } } tail`\n");
}

#[test]
fn merged_rfc_syntax_roundtrips() {
    assert_luau(
        "@native\nexport function convert<T>(value: T): T\nreturn value\nend\n\
         export local seen: integer = 0i\n\
         export const MAX: integer = 0XFF_FFi\n\
         type NonNil = number | string\n\
         type Closed = (string | nil)\n\
         local partial = convert<<integer>>\n\
         local value = convert<<integer>>(MAX)\n\
         local mapped = values:map<<integer, string>>(convert)\n",
        "@native\nexport function convert<T>(value: T): T\n\treturn value\nend\n\
         export local seen: integer = 0i\n\
         export const MAX: integer = 0xFF_FFi\n\
         type NonNil = number | string\n\
         type Closed = (string | nil)\n\
         local partial = convert<<integer>>\n\
         local value = convert<<integer>>(MAX)\n\
         local mapped = values:map<<integer, string>>(convert)\n",
    );
}

#[test]
fn broken_method_type_args_keep_the_call_paren_on_the_name_line() {
    let options = FormatOptions { line_width: 30, ..FormatOptions::default() };
    let output = format_once(
        "local x = a:b<<Aaaaaaaaaaaaaa, Bbbbbbbbbbbbbbb>>(c)\n",
        LuaVersion::Luau,
        &options,
    );
    let call_line = output.lines().find(|line| line.contains(">>(")).expect("a call line");
    assert!(
        call_line.contains("a:b<<"),
        "official Luau rejects a method call whose `(` is on a later line than its name: {output:?}"
    );
}

#[test]
fn string_singleton_type_follows_the_quote_style() {
    assert_eq!(
        format_once("type A = 'b'\n", LuaVersion::Luau, &FormatOptions::default()),
        "type A = \"b\"\n",
        "`quote_style` must apply to a string singleton type as it does to a string literal"
    );
}

#[test]
fn verify_accepts_dropping_a_lone_leading_separator() {
    for input in ["type A = | B\n", "type A = & B\n"] {
        let result =
            luck_formatter::format_and_verify(input, LuaVersion::Luau, &FormatOptions::default());
        assert!(
            result.is_ok(),
            "{input:?} means the same as the `type A = B` it formats to: {:?}",
            result.err().map(|(_, diff)| diff)
        );
    }
}
