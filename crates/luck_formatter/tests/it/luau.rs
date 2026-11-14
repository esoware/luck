use luck_formatter::FormatOptions;
use luck_token::LuaVersion;

use crate::common::assert_format_with;

fn assert_luau(input: &str, expected: &str) {
    assert_format_with(input, expected, LuaVersion::Luau, &FormatOptions::default());
}

#[test]
fn long_if_expression_starts_a_line_at_then_and_else() {
    let options = FormatOptions { line_width: 40, ..FormatOptions::default() };
    assert_format_with(
        "local scale = if someLongFlagName() then firstValue else secondValue\n",
        "local scale = if someLongFlagName()\n\tthen firstValue\n\telse secondValue\n",
        LuaVersion::Luau,
        &options,
    );
}

#[test]
fn long_if_expression_keeps_an_elseif_and_its_then_together() {
    let options = FormatOptions { line_width: 40, ..FormatOptions::default() };
    assert_format_with(
        "local scale = if firstFlag() then 1 elseif secondFlag() then 0.5 else 2\n",
        "local scale = if firstFlag()\n\tthen 1\n\telseif secondFlag() then 0.5\n\telse 2\n",
        LuaVersion::Luau,
        &options,
    );
}

#[test]
fn if_expression_branches_hug_their_tables() {
    let source = "local thing = makeSomething(\"Foo\", {\n\tOneChild = if someFlag()\n\t\tthen makeSomething(\"Bar\", {\n\t\t\tscale = 1,\n\t\t})\n\t\telse makeSomething(\"Bar\", {\n\t\t\tscale = 2,\n\t\t}),\n\tTwoChild = makeSomething(\"Baz\"),\n})\n";
    assert_luau(source, source);
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
    assert_luau("local function f(x: number)\nend\n", "local function f(x: number) end\n");
}

#[test]
fn param_type_annotation_multiple() {
    assert_luau(
        "local function f(x: number, y: string)\nend\n",
        "local function f(x: number, y: string) end\n",
    );
}

#[test]
fn return_type_annotation() {
    assert_luau(
        "local function f(x: number): string\nend\n",
        "local function f(x: number): string end\n",
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
    assert_luau("local function f(x: number?)\nend\n", "local function f(x: number?) end\n");
}

#[test]
fn type_declaration_qualified() {
    assert_luau("type Alias = module.Type\n", "type Alias = module.Type\n");
}

#[test]
fn vararg_type_annotation() {
    assert_luau("local function f(...: number)\nend\n", "local function f(...: number) end\n");
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
    assert_format_with(
        "local function f(aaa: number, bbb: string, ccc: boolean)\nend\n",
        "local function f(\n\taaa: number,\n\tbbb: string,\n\tccc: boolean\n) end\n",
        LuaVersion::Luau,
        &options,
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
    // Official Luau rejects a method call whose `(` is on a later line than
    // its name.
    let options = FormatOptions { line_width: 30, ..FormatOptions::default() };
    assert_format_with(
        "local x = a:b<<Aaaaaaaaaaaaaa, Bbbbbbbbbbbbbbb>>(c)\n",
        "local x = a:b<<Aaaaaaaaaaaaaa, Bbbbbbbbbbbbbbb>>(\n\tc\n)\n",
        LuaVersion::Luau,
        &options,
    );
}

#[test]
fn string_singleton_type_follows_the_quote_style() {
    assert_luau("type A = 'b'\n", "type A = \"b\"\n");
}

#[test]
fn a_lone_leading_separator_is_dropped() {
    assert_luau("type A = | B\n", "type A = B\n");
    assert_luau("type A = & B\n", "type A = B\n");
}

#[test]
fn an_array_field_keeps_the_leading_operator_that_keeps_read_from_opening_it() {
    // A bare `read` or `write` opening a table type field is its access
    // modifier, so `{ write }` reads back as a field with no type.
    for (input, expected) in [
        ("type A = { | write }\n", "type A = { | write }\n"),
        ("type A = { | read | Other }\n", "type A = { | read | Other }\n"),
        ("type A = { & write & Other }\n", "type A = { & write & Other }\n"),
        ("type A = { read | read }\n", "type A = { read read }\n"),
        ("type A = { | Other | read }\n", "type A = { Other | read }\n"),
    ] {
        assert_luau(input, expected);
    }
}
