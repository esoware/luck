use luck_allocator::Allocator;

use crate::common::{minify_default, minify_typed};

const TYPED_SOURCE: &str = "\
type Point = { x: number, y: number }
local p: Point = { x = 1, y = 2 } :: Point
return p, f<<Point>>(p :: any)
";

#[test]
fn types_go_by_default() {
    assert_eq!(minify_default(TYPED_SOURCE), "local e={x=1,y=2}return e,f(e)");
}

#[test]
fn type_declaration_survives_minification_without_stripping() {
    let result = minify_typed(TYPED_SOURCE);
    assert!(result.contains("type") && result.contains("<<Point>>"), "Got: {result}");
}

#[test]
fn if_expr_survives_minification() {
    let result = minify_default(
        "\
local x = if true then 1 else 2
return x
",
    );
    assert!(result.contains("if"), "Got: {result}");
}

#[test]
fn string_interpolation_survives_minification() {
    let result = minify_default(
        "\
local name = \"World\"
local g = `Hello, {name}!`
return g
",
    );
    assert!(result.contains('`'), "Got: {result}");
}

#[test]
fn continue_survives_minification() {
    let result = minify_default(
        "\
for i = 1, 10 do
  if i == 5 then continue end
  print(i)
end
",
    );
    assert!(result.contains("continue"), "Got: {result}");
}

#[test]
fn compound_assignment_survives_minification() {
    let result = minify_default(
        "\
local x = 0
x += 1
return x
",
    );
    assert!(result.contains("+="), "Got: {result}");
}

#[test]
fn merged_rfc_syntax_and_exports_survive_minification() {
    let source = "\
export local public = 129312i
export const mask = 0xffffffffffffffffi
export function apply<T>(value: T)
    return identity<<T>>(value), public, mask
end
";
    let result = minify_typed(source);
    assert!(result.contains("export local public=129312i"), "{result}");
    assert!(result.contains("export const mask=0xffffffffffffffffi"), "{result}");
    assert!(result.contains("export function apply"), "{result}");
    assert!(result.contains("identity<<T>>"), "{result}");
    assert!(result.contains(":T"), "{result}");
    let allocator = Allocator::new();
    let reparsed = luck_parser::parse(&allocator, result.as_bytes());
    assert!(
        reparsed.errors.is_empty(),
        "minified merged-RFC syntax must reparse: {:?}\n{result}",
        reparsed.errors
    );
}

#[test]
fn instantiated_literal_survives_minification() {
    for (source, expected) in [
        ("local a = `x{({})<<T>>}y`\n", "local e=`x{({})<<T>>}y`"),
        ("return (\"s\")<<T>>()\n", "return(\"s\")<<T>>()"),
        (
            "\
local s = \"x\"
print(s<<T>>)
",
            "print((\"x\")<<T>>)",
        ),
    ] {
        let result = minify_typed(source);
        assert_eq!(result, expected, "source: {source}");
        assert_eq!(minify_typed(&result), result, "must be idempotent");
    }
}

#[test]
fn inlined_cast_before_less_than_survives_minification() {
    // Strings compare through the locale, so the comparison stays.
    let source = "\
local s = 'a' :: string
return s < 'b'
";
    let result = minify_typed(source);
    assert_eq!(result, "return('a'::string)<'b'");
    assert_eq!(minify_typed(&result), result, "must be idempotent");
}

#[test]
fn function_inside_a_type_survives_minification() {
    for (source, expected) in [
        ("type T = typeof(function(x) return x end)\n", "type T=typeof(function(e)return e end)"),
        (
            "return f<<typeof(function(x) return x end)>>()\n",
            "return f<<typeof(function(e)return e end)>>()",
        ),
    ] {
        let result = minify_typed(source);
        assert_eq!(result, expected, "source: {source}");
        assert_eq!(minify_typed(&result), result, "must be idempotent");
    }
}

#[test]
fn shebang_stays_on_the_first_line() {
    let output = minify_default(
        "\
#!/usr/bin/env lua
print(1)
",
    );
    assert_eq!(
        output,
        "\
#!/usr/bin/env lua
print(1)"
    );
    assert_eq!(minify_default(&output), output, "must be idempotent");
}

#[test]
fn shebang_drops_trailing_carriage_returns() {
    for source in ["#!x\r\r", "#!x\r\r\nprint(1)\n"] {
        let output = minify_default(source);
        assert!(output.starts_with("#!x\n"), "{output:?}");
        assert_eq!(minify_default(&output), output, "must be idempotent");
    }
}

#[test]
fn shebang_keeps_a_lone_carriage_return_as_text() {
    let output = minify_default("#!x\ry = {\nprint(1)\n");
    assert_eq!(output, "#!x\ry = {\nprint(1)");
    assert_eq!(minify_default(&output), output, "must be idempotent");
}

#[test]
fn shebang_stays_above_hot_comments() {
    let output = minify_default(
        "\
#!/usr/bin/env luau
--!strict
print(1)
",
    );
    assert_eq!(
        output,
        "\
#!/usr/bin/env luau
--!strict
print(1)"
    );
    assert_eq!(minify_default(&output), output, "must be idempotent");
}

#[test]
fn hot_comments_survive_minification() {
    let output = minify_default(
        "\
--!strict
--!native
--!optimize 2
local x: number = 1
return x
",
    );
    assert!(
        output.starts_with(
            "\
--!strict
--!native
--!optimize 2
"
        ),
        "hot comments must lead the output: {output}"
    );
    // Idempotent: a second pass keeps exactly one copy.
    let second = minify_default(&output);
    assert_eq!(output, second, "hot comment prefix must be stable");
}

#[test]
fn plain_comments_are_dropped() {
    let output = minify_default(
        "\
-- plain comment
return 1
",
    );
    assert!(!output.contains("plain comment"), "{output}");
}
