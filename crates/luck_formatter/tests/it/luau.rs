use crate::common::assert_format_with;
use luck_formatter::FormatOptions;
use luck_token::LuaVersion;

fn assert_luau(input: &str, expected: &str) {
    assert_format_with(input, expected, LuaVersion::Luau, &FormatOptions::default());
}

#[test]
fn param_type_annotation_simple() {
    assert_luau(
        "local function f(x: number)\nend\n",
        "local function f(x: number)\nend\n",
    );
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
fn type_decl_table() {
    assert_luau(
        "type Foo = { name: string, age: number }\n",
        "type Foo = { name: string, age: number }\n",
    );
}

#[test]
fn type_decl_table_normalizes_whitespace() {
    assert_luau(
        "type Foo = {name:string,age:number}\n",
        "type Foo = { name: string, age: number }\n",
    );
}

#[test]
fn type_decl_function_type() {
    assert_luau(
        "type Callback = (number, string) -> boolean\n",
        "type Callback = (number, string) -> boolean\n",
    );
}

#[test]
fn type_decl_union() {
    assert_luau(
        "type StringOrNumber = string | number\n",
        "type StringOrNumber = string | number\n",
    );
}

#[test]
fn type_decl_union_normalizes_whitespace() {
    assert_luau(
        "type StringOrNumber = string|number\n",
        "type StringOrNumber = string | number\n",
    );
}

#[test]
fn type_decl_intersection() {
    assert_luau(
        "type Both = Readable & Writable\n",
        "type Both = Readable & Writable\n",
    );
}

#[test]
fn type_decl_generic() {
    assert_luau(
        "type Map = Map<string, number>\n",
        "type Map = Map<string, number>\n",
    );
}

#[test]
fn type_decl_nested_generic() {
    assert_luau(
        "type Nested = Map<string, Array<number>>\n",
        "type Nested = Map<string, Array<number>>\n",
    );
}

#[test]
fn type_annotation_optional() {
    assert_luau(
        "local function f(x: number?)\nend\n",
        "local function f(x: number?)\nend\n",
    );
}

#[test]
fn type_decl_qualified() {
    assert_luau("type Alias = module.Type\n", "type Alias = module.Type\n");
}

#[test]
fn vararg_type_annotation() {
    assert_luau(
        "local function f(...: number)\nend\n",
        "local function f(...: number)\nend\n",
    );
}

#[test]
fn export_type_table() {
    assert_luau(
        "export type Config = { width: number, height: number }\n",
        "export type Config = { width: number, height: number }\n",
    );
}

#[test]
fn type_decl_with_generics() {
    assert_luau(
        "type Container<T> = { value: T }\n",
        "type Container<T> = { value: T }\n",
    );
}

// Luau's parlist grammar forbids trailing commas in parameter lists
// (`bindinglist [',' '...'] | '...'`), so broken params must never gain
// one - the output has to re-parse (hard invariant 8).
#[test]
fn luau_params_break_without_trailing_comma() {
    let options = FormatOptions {
        line_width: 30,
        ..FormatOptions::default()
    };
    let input = "local function f(aaa: number, bbb: string, ccc: boolean)\nend\n";
    let result = luck_formatter::format(input, LuaVersion::Luau, &options);
    assert!(result.errors.is_empty());
    assert!(
        !result.output.contains("boolean,"),
        "params must not gain a trailing comma: {}",
        result.output,
    );
    let reparse = luck_parser::parse(&result.output, LuaVersion::Luau);
    assert!(
        reparse.errors.is_empty(),
        "formatted params failed to re-parse: {}\nerrors: {:?}",
        result.output,
        reparse.errors,
    );
}

#[test]
fn interp_expr_starting_with_table_gets_space() {
    // `{{` is a parse error in Luau; the formatter separates the
    // interpolation opener from a leading table constructor.
    assert_luau(
        "return `list { {1, 2} } tail`\n",
        "return `list { { 1, 2 } } tail`\n",
    );
}

#[test]
fn merged_rfc_syntax_roundtrips() {
    assert_luau(
        "@native\nexport function convert<T>(value: T): T\nreturn value\nend\n\
         export local seen: integer = 0i\n\
         export const MAX: integer = 0XFF_FFi\n\
         type NonNil = ~nil\n\
         type Closed = ~(string | nil)\n\
         local partial = convert<<integer>>\n\
         local value = convert<<integer>>(MAX)\n\
         local mapped = values:map<<integer, string>>(convert)\n",
        "@native\nexport function convert<T>(value: T): T\n\treturn value\nend\n\
         export local seen: integer = 0i\n\
         export const MAX: integer = 0xFF_FFi\n\
         type NonNil = ~nil\n\
         type Closed = ~(string | nil)\n\
         local partial = convert<<integer>>\n\
         local value = convert<<integer>>(MAX)\n\
         local mapped = values:map<<integer, string>>(convert)\n",
    );
}
