use luck_formatter::FormatOptions;

use crate::common::{assert_format, assert_format_with};

#[test]
fn paren_type_instantiation_stmt_keeps_guard() {
    // Without the `;` this is a parse error, since a call's `(` can't open a
    // new line.
    assert_format(
        "\
f()
;(g or h)<<T>>()
",
        "\
f()
;(g or h)<<T>>()
",
    );
    assert_format(
        "\
f()
;(g or h)<<T>>.x = 1
",
        "\
f()
;(g or h)<<T>>.x = 1
",
    );
}

#[test]
fn param_type_annotation_simple() {
    assert_format(
        "\
local function f(x: number)
end
",
        "local function f(x: number) end\n",
    );
}

#[test]
fn param_type_annotation_multiple() {
    assert_format(
        "\
local function f(x: number, y: string)
end
",
        "local function f(x: number, y: string) end\n",
    );
}

#[test]
fn return_type_annotation() {
    assert_format(
        "\
local function f(x: number): string
end
",
        "local function f(x: number): string end\n",
    );
}

#[test]
fn params_break_when_the_return_type_overflows_the_line() {
    // The parameters fit up to `)`, and `: number` takes the line to 44
    // columns.
    let options = FormatOptions { line_width: 40, ..FormatOptions::default() };
    assert_format_with(
        "function M.add(a: number, b: number): number return a + b end\n",
        "\
function M.add(
\ta: number,
\tb: number
): number
\treturn a + b
end
",
        &options,
    );
}

#[test]
fn type_cast() {
    assert_format("local x = y :: number\n", "local x = y :: number\n");
}

#[test]
fn type_declaration_table() {
    assert_format(
        "type Foo = { name: string, age: number }\n",
        "type Foo = { name: string, age: number }\n",
    );
}

#[test]
fn type_declaration_table_normalizes_whitespace() {
    assert_format(
        "type Foo = {name:string,age:number}\n",
        "type Foo = { name: string, age: number }\n",
    );
}

#[test]
fn type_declaration_function_type() {
    assert_format(
        "type Callback = (number, string) -> boolean\n",
        "type Callback = (number, string) -> boolean\n",
    );
}

#[test]
fn type_declaration_union() {
    assert_format(
        "type StringOrNumber = string | number\n",
        "type StringOrNumber = string | number\n",
    );
}

#[test]
fn type_declaration_union_normalizes_whitespace() {
    assert_format(
        "type StringOrNumber = string|number\n",
        "type StringOrNumber = string | number\n",
    );
}

#[test]
fn type_declaration_intersection() {
    assert_format("type Both = Readable & Writable\n", "type Both = Readable & Writable\n");
}

#[test]
fn type_declaration_generic() {
    assert_format("type Map = Map<string, number>\n", "type Map = Map<string, number>\n");
}

#[test]
fn type_declaration_nested_generic() {
    assert_format(
        "type Nested = Map<string, Array<number>>\n",
        "type Nested = Map<string, Array<number>>\n",
    );
}

#[test]
fn type_annotation_optional() {
    assert_format(
        "\
local function f(x: number?)
end
",
        "local function f(x: number?) end\n",
    );
}

#[test]
fn a_run_of_question_marks_keeps_every_one() {
    assert_format(
        "type T = { number ? ? } ?? ?
",
        "type T = { number?? }???
",
    );
}

#[test]
fn type_declaration_qualified() {
    assert_format("type Alias = module.Type\n", "type Alias = module.Type\n");
}

#[test]
fn vararg_type_annotation() {
    assert_format(
        "\
local function f(...: number)
end
",
        "local function f(...: number) end\n",
    );
}

#[test]
fn export_type_table() {
    assert_format(
        "export type Config = { width: number, height: number }\n",
        "export type Config = { width: number, height: number }\n",
    );
}

#[test]
fn type_declaration_with_generics() {
    assert_format("type Container<T> = { value: T }\n", "type Container<T> = { value: T }\n");
}

// The parlist grammar (`bindinglist [',' '...'] | '...'`) has no trailing
// comma, so a broken parameter list must not gain one.
#[test]
fn typed_params_break_without_trailing_comma() {
    let options = FormatOptions { line_width: 30, ..FormatOptions::default() };
    assert_format_with(
        "\
local function f(aaa: number, bbb: string, ccc: boolean)
end
",
        "\
local function f(
\taaa: number,
\tbbb: string,
\tccc: boolean
) end
",
        &options,
    );
}

#[test]
fn merged_rfc_syntax_roundtrips() {
    assert_format(
        "\
@native
export function convert<T>(value: T): T
return value
end
export local seen: number = 0
export const MAX: number = 0XFF_FF
type NonNil = number | string
type Closed = (string | nil)
local partial = convert<<number>>
local value = convert<<number>>(MAX)
local mapped = values:map<<number, string>>(convert)
",
        "\
@native
export function convert<T>(value: T): T
\treturn value
end
export local seen: number = 0
export const MAX: number = 0xFF_FF
type NonNil = number | string
type Closed = (string | nil)
local partial = convert<<number>>
local value = convert<<number>>(MAX)
local mapped = values:map<<number, string>>(convert)
",
    );
}

#[test]
fn broken_method_type_args_keep_the_call_paren_on_the_name_line() {
    // A method call whose `(` is on a later line than its name doesn't parse.
    let options = FormatOptions { line_width: 30, ..FormatOptions::default() };
    assert_format_with(
        "local x = a:b<<Aaaaaaaaaaaaaa, Bbbbbbbbbbbbbbb>>(c)\n",
        "\
local x = a:b<<Aaaaaaaaaaaaaa, Bbbbbbbbbbbbbbb>>(
\tc
)
",
        &options,
    );
}

#[test]
fn string_singleton_type_follows_the_quote_style() {
    assert_format("type A = 'b'\n", "type A = \"b\"\n");
}

#[test]
fn a_lone_leading_separator_is_dropped() {
    assert_format("type A = | B\n", "type A = B\n");
    assert_format("type A = & B\n", "type A = B\n");
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
        assert_format(input, expected);
    }
}

/// A table that breaks opens a line for its array field, so a union or an
/// intersection that breaks there starts on that line, at its indent, where
/// one after an access modifier moves to the next line.
#[test]
fn a_union_breaks_on_the_line_its_table_opens_for_it() {
    let options = FormatOptions { line_width: 30, ..FormatOptions::default() };
    for (input, expected) in [
        (
            "type T = { \"aaaaaaaaaa\" | \"bbbbbbbbbb\" | \"cccccccccc\" }\n",
            "\
type T = {
\t| \"aaaaaaaaaa\"
\t| \"bbbbbbbbbb\"
\t| \"cccccccccc\"
}
",
        ),
        (
            "type T = { Aaaaaaaaaaaa & Bbbbbbbbbbbb & Cccccccccccc }\n",
            "\
type T = {
\t& Aaaaaaaaaaaa
\t& Bbbbbbbbbbbb
\t& Cccccccccccc
}
",
        ),
        (
            "type T = { \"aaaaaaaa\" | \"bbbbbbbb\" }\n",
            "\
type T = {
\t\"aaaaaaaa\" | \"bbbbbbbb\"
}
",
        ),
        (
            "type T = { | read | \"aaaaaaaaaa\" | \"bbbbbbbbbb\" }\n",
            "\
type T = {
\t| read
\t| \"aaaaaaaaaa\"
\t| \"bbbbbbbbbb\"
}
",
        ),
        (
            "type T = { read \"aaaaaaaaaa\" | \"bbbbbbbbbb\" }\n",
            "\
type T = {
\tread
\t\t| \"aaaaaaaaaa\"
\t\t| \"bbbbbbbbbb\"
}
",
        ),
    ] {
        assert_format_with(input, expected, &options);
    }
}

/// A type list that breaks opens a line for each item, so a union that
/// breaks as an unnamed item starts on that line, at its indent, where one
/// after `name:` moves to the next line.
#[test]
fn a_union_breaks_on_the_line_its_list_opens_for_it() {
    let options = FormatOptions { line_width: 30, ..FormatOptions::default() };
    for (input, expected) in [
        (
            "type T = Map<\"aaaaaaaaaa\" | \"bbbbbbbbbb\" | \"cccccccccc\", number>\n",
            "\
type T = Map<
\t| \"aaaaaaaaaa\"
\t| \"bbbbbbbbbb\"
\t| \"cccccccccc\",
\tnumber
>
",
        ),
        (
            "type T = (\"aaaaaaaaaa\" | \"bbbbbbbbbb\", n: \"aaaaaaaaaa\" | \"bbbbbbbbbb\") -> ()\n",
            "\
type T = (
\t| \"aaaaaaaaaa\"
\t| \"bbbbbbbbbb\",
\tn:
\t\t| \"aaaaaaaaaa\"
\t\t| \"bbbbbbbbbb\"
) -> ()
",
        ),
        (
            "local function f(): (\"aaaaaaaaaa\" | \"bbbbbbbbbb\", ...Cccccccc) end\n",
            "\
local function f(): (
\t| \"aaaaaaaaaa\"
\t| \"bbbbbbbbbb\",
\t...Cccccccc
) end
",
        ),
        (
            "local v = f<<\"aaaaaaaaaa\" | \"bbbbbbbbbb\" | \"cccccccccc\">>()\n",
            "\
local v = f<<
\t| \"aaaaaaaaaa\"
\t| \"bbbbbbbbbb\"
\t| \"cccccccccc\"
>>()
",
        ),
    ] {
        assert_format_with(input, expected, &options);
    }
    let narrowest = FormatOptions { line_width: 1, ..FormatOptions::default() };
    assert_format_with(
        "type T = Map<\"a\" | \"b\", { A & B }>\n",
        "\
type T = Map<
\t| \"a\"
\t| \"b\",
\t{
\t\t& A
\t\t& B
\t}
>
",
        &narrowest,
    );
}
