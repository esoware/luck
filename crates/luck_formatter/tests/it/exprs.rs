use luck_formatter::FormatOptions;

use crate::common::{assert_format, assert_format_with};

#[test]
fn literals() {
    assert_format("local x=1\n", "local x = 1\n");
    assert_format("local x=true\n", "local x = true\n");
    assert_format("local x=nil\n", "local x = nil\n");
    assert_format("local x = \"hello\"\n", "local x = \"hello\"\n");
}

#[test]
fn binary_op_flat() {
    assert_format("local x=a+b+c\n", "local x = a + b + c\n");
}

#[test]
fn unary_op() {
    assert_format("local x=-1\n", "local x = -1\n");
    assert_format("local x=not y\n", "local x = not y\n");
    assert_format("local x=#t\n", "local x = #t\n");
}

#[test]
fn parenthesized() {
    // A user's parentheses show how an operator expression groups, so they
    // stay, a pair for each.
    assert_format("local x=(a+b)\n", "local x = (a + b)\n");
    assert_format("local x=((a+b))\n", "local x = (a + b)\n");
    assert_format("local x=(a and b) or c\n", "local x = (a and b) or c\n");
    assert_format("local x=(not a) == b, (-a) + b\n", "local x = (not a) == b, (-a) + b\n");
    assert_format("local x=(a < b) == c\n", "local x = (a < b) == c\n");
    // Around anything else they group nothing, so they go.
    assert_format(
        "local x=(a), ((\"s\")), ({}), (f).y, (g)()\n",
        "local x = a, \"s\", {}, f.y, g()\n",
    );
    // Those that truncate a call or `...` to one value stay, once.
    assert_format("local x=((f())), (...)\n", "local x = (f()), (...)\n");
    // So do those a comment sits in, which needs the line breaks they hold.
    assert_format("local x=(a --[[c]])\n", "local x = (a --[[c]])\n");
    // A position that needs them gets them.
    assert_format("local x=(\"s\"):rep(2)\n", "local x = (\"s\"):rep(2)\n");
}

#[test]
fn binary_op_expanded() {
    let options = FormatOptions { line_width: 20, ..FormatOptions::default() };
    assert_format_with(
        "local x=aaaa+bbbb+cccc\n",
        "\
local x = aaaa
\t+ bbbb
\t+ cccc
",
        &options,
    );
}

#[test]
fn binary_chain_breaks_at_one_precedence_level() {
    let options = FormatOptions { line_width: 40, ..FormatOptions::default() };
    assert_format_with(
        "local total = first_value * second_value + third_value * fourth_value\n",
        "\
local total = first_value * second_value
\t+ third_value * fourth_value
",
        &options,
    );
}

#[test]
fn args_measure_a_long_string_after_them_to_its_first_line_break() {
    // `local v = f(aaaa)[ [[x` is 22 columns.
    let source = "\
local v = f(aaaa)[ [[x
y]] ]
";
    let width = |line_width| FormatOptions { line_width, ..FormatOptions::default() };
    assert_format_with(source, source, &width(22));
    assert_format_with(
        source,
        "\
local v = f(
\taaaa
)[ [[x
y]] ]
",
        &width(21),
    );
}

#[test]
fn multi_assignment() {
    assert_format("x,y=1,2\n", "x, y = 1, 2\n");
}

#[test]
fn condition_parens_removed() {
    assert_format(
        "\
if (x) then
y = 1
end
",
        "\
if x then
\ty = 1
end
",
    );
}

#[test]
fn long_if_expression_starts_a_line_at_then_and_else() {
    let options = FormatOptions { line_width: 40, ..FormatOptions::default() };
    assert_format_with(
        "local scale = if someLongFlagName() then firstValue else secondValue\n",
        "\
local scale = if someLongFlagName()
\tthen firstValue
\telse secondValue
",
        &options,
    );
}

#[test]
fn long_if_expression_keeps_an_elseif_and_its_then_together() {
    let options = FormatOptions { line_width: 40, ..FormatOptions::default() };
    assert_format_with(
        "local scale = if firstFlag() then 1 elseif secondFlag() then 0.5 else 2\n",
        "\
local scale = if firstFlag()
\tthen 1
\telseif secondFlag() then 0.5
\telse 2
",
        &options,
    );
}

#[test]
fn if_expression_branches_hug_their_tables() {
    let source = "\
local thing = makeSomething(\"Foo\", {
\tOneChild = if someFlag()
\t\tthen makeSomething(\"Bar\", {
\t\t\tscale = 1,
\t\t})
\t\telse makeSomething(\"Bar\", {
\t\t\tscale = 2,
\t\t}),
\tTwoChild = makeSomething(\"Baz\"),
})
";
    assert_format(source, source);
}

#[test]
fn interpolated_expr_starting_with_table_gets_space() {
    // `{{` is a parse error, so the formatter separates the interpolation
    // opener from a leading table constructor.
    assert_format("return `list { {1, 2} } tail`\n", "return `list { { 1, 2 } } tail`\n");
}
