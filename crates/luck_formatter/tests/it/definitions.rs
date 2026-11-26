use luck_allocator::Allocator;
use luck_formatter::{Comments, FormatOptions, format_chunk};
use luck_testgen::tree_shape;

use crate::common::formatted_as;

/// `input`, a definition file, formats to `expected` under `options`, which
/// reads back as the same tree and formats to itself.
fn assert_format_with(input: &str, expected: &str, options: &FormatOptions) {
    let output =
        formatted_as(luck_parser::parse_definitions_with_tokens, input.as_bytes(), options);
    assert_eq!(String::from_utf8_lossy(&output), expected);
    let allocator = Allocator::new();
    let parsed = luck_parser::parse_definitions(&allocator, input.as_bytes());
    let formatted = luck_parser::parse_definitions(&allocator, &output);
    assert_eq!(
        tree_shape(&formatted.chunk),
        tree_shape(&parsed.chunk),
        "formatting changed the tree"
    );
}

fn assert_format(input: &str, expected: &str) {
    assert_format_with(input, expected, &FormatOptions::default());
}

#[test]
fn a_declared_global_formats_as_a_typed_name() {
    assert_format("declare   foo :number", "declare foo: number\n");
    assert_format(
        "declare vector: {zero: vector, one: vector}",
        "declare vector: { zero: vector, one: vector }\n",
    );
}

#[test]
fn a_declared_function_s_attributes_take_lines_of_their_own() {
    assert_format(
        "@checked   declare function abs( n :number ):number",
        "\
@checked
declare function abs(n: number): number
",
    );
    assert_format(
        "@native @[deprecated {use = 'g', reason = 'old'}] declare function f<T, U...>(x: T, ...: U...): (T, U...)",
        "\
@native
@[deprecated({ use = \"g\", reason = \"old\" })]
declare function f<T, U...>(x: T, ...: U...): (T, U...)
",
    );
}

#[test]
fn a_long_declared_function_breaks_its_parameters() {
    let options = FormatOptions { line_width: 40, ..FormatOptions::default() };
    assert_format_with(
        "declare function create(size: number, fill: number?, name: string?): buffer",
        "\
declare function create(
\tsize: number,
\tfill: number?,
\tname: string?
): buffer
",
        &options,
    );
}

#[test]
fn a_function_type_s_attributes_stay_on_its_line() {
    let options = FormatOptions { line_width: 44, ..FormatOptions::default() };
    assert_format_with(
        "declare bit32: {band: @checked (...number) -> number, lshift: @checked (x: number, disp: number) -> number}",
        "\
declare bit32: {
\tband: @checked (...number) -> number,
\tlshift: @checked (
\t\tx: number,
\t\tdisp: number
\t) -> number,
}
",
        &options,
    );
    assert_format("declare id: @native <T>(T) -> T", "declare id: @native <T>(T) -> T\n");
}

#[test]
fn attributes_before_a_lone_parenthesized_type_print_back() {
    assert_format(
        "declare f: @checked(number)?",
        "declare f: @checked (number)?
",
    );
    let options = FormatOptions { line_width: 20, ..FormatOptions::default() };
    assert_format_with(
        "declare f: @checked (number) | string | boolean",
        "declare f:
	@checked (number)
	| string
	| boolean
",
        &options,
    );
    assert_format_with(
        "declare f: @checked () -> () & ((number) -> number)",
        "declare f:
	@checked () -> ()
	& ((
		number
	) -> number)
",
        &options,
    );
}

#[test]
fn the_chain_an_attributed_declare_opens_prints_back() {
    assert_format(
        "@checked declare . x [ 1 ] 's' function f ( )",
        "@checked
declare.x[1](\"s\") function f()
",
    );
}

#[test]
fn an_extern_type_holds_a_member_to_a_line() {
    assert_format(
        "\
declare extern type Part extends Instance with
  read Name: string


  write Size: number   -- in studs
  read: boolean
  [\"Parent Name\"]: string
    [string]: any
  --- Whether it's a `className`.
  @checked function IsA(self, className: string): boolean
  function IsA(self, className: \"Part\"): true
  function Fire(self, ...: any): ...any
  -- More to come.
end",
        "\
declare extern type Part extends Instance with
\tread Name: string

\twrite Size: number -- in studs
\tread: boolean
\t[\"Parent Name\"]: string
\t[string]: any
\t--- Whether it's a `className`.
\t@checked
\tfunction IsA(self, className: string): boolean
\tfunction IsA(self, className: \"Part\"): true
\tfunction Fire(self, ...: any): ...any
\t-- More to come.
end
",
    );
}

#[test]
fn an_empty_extern_type_closes_on_its_line() {
    assert_format(
        "\
declare extern type Empty with

end",
        "declare extern type Empty with end\n",
    );
    assert_format(
        "\
declare extern type Empty with -- nothing yet
end",
        "\
declare extern type Empty with -- nothing yet
end
",
    );
}

#[test]
fn a_tree_formats_the_same_without_its_source() {
    let source = "\
@checked
declare function abs(n: number): number
declare bit32: { band: @checked (...number) -> number }
declare extern type Part extends Instance with
\tread Name: string
\t[string]: any
\t@[deprecated({ use = \"Destroy\" })]
\tfunction Remove(self)
end
";
    let allocator = Allocator::new();
    let parsed = luck_parser::parse_definitions(&allocator, source.as_bytes());
    assert!(parsed.errors.is_empty(), "{:?}", parsed.errors);
    let options = FormatOptions::default();
    let output = format_chunk(&allocator, &parsed.chunk, Comments::none(), &options).unwrap();
    assert_eq!(String::from_utf8(output).unwrap(), source);
}
