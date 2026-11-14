use luck_minifier::TransformConfig;
use luck_token::LuaVersion;

use crate::common::{minify_lua54, minify_luau, minify_with};

#[test]
fn lua54_const_attribute_survives_minification() {
    let result = minify_lua54(
        "\
local x <const> = 42
print(x, x)
",
    );
    assert!(result.contains("<const>"), "Got: {result}");
}

#[test]
fn lua52_goto_and_label_survive_minification() {
    let result = minify_with(
        "\
goto skip
::skip::
local x = 1
print(x)
",
        LuaVersion::Lua52,
        &TransformConfig::default(),
    );
    assert!(result.contains("goto"), "Got: {result}");
    assert!(result.contains("::"), "Got: {result}");
}

#[test]
fn luau_type_declaration_survives_minification() {
    let result = minify_luau(
        "\
type Point = { x: number, y: number }
local p: Point = { x = 1, y = 2 }
return p
",
    );
    assert!(result.contains("type"), "Got: {result}");
}

#[test]
fn luau_if_expr_survives_minification() {
    let result = minify_luau(
        "\
local x = if true then 1 else 2
return x
",
    );
    assert!(result.contains("if"), "Got: {result}");
}

#[test]
fn luau_string_interpolation_survives_minification() {
    let result = minify_luau(
        "\
local name = \"World\"
local g = `Hello, {name}!`
return g
",
    );
    assert!(result.contains('`'), "Got: {result}");
}

#[test]
fn luau_continue_survives_minification() {
    let result = minify_luau(
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
fn luau_compound_assignment_survives_minification() {
    let result = minify_luau(
        "\
local x = 0
x += 1
return x
",
    );
    assert!(result.contains("+="), "Got: {result}");
}

#[test]
fn luau_merged_rfc_syntax_and_exports_survive_minification() {
    let source = "\
export local public = 129312i
export const mask = 0xffffffffffffffffi
export function apply<T>(value: T)
    return identity<<T>>(value), public, mask
end
";
    let result = minify_luau(source);
    assert!(result.contains("export local public=129312i"), "{result}");
    assert!(result.contains("export const mask=0xffffffffffffffffi"), "{result}");
    assert!(result.contains("export function apply"), "{result}");
    assert!(result.contains("identity<<T>>"), "{result}");
    assert!(result.contains(":T"), "{result}");
    let reparsed = luck_parser::parse(result.clone(), LuaVersion::Luau);
    assert!(
        reparsed.errors.is_empty(),
        "minified merged-RFC syntax must reparse: {:?}\n{result}",
        reparsed.errors
    );
}

#[test]
fn luau_instantiated_literal_survives_minification() {
    for (source, expected) in [
        ("local a = `x{({})<<T>>}y`\n", "local l=`x{({})<<T>>}y`"),
        ("return (\"s\")<<T>>()\n", "return(\"s\")<<T>>()"),
        (
            "\
local s = \"x\"
print(s<<T>>)
",
            "print((\"x\")<<T>>)",
        ),
    ] {
        let result = minify_luau(source);
        assert_eq!(result, expected, "source: {source}");
        assert_eq!(minify_luau(&result), result, "must be idempotent");
    }
}

#[test]
fn inlined_cast_before_less_than_survives_minification() {
    let source = "\
local s = 1 :: number
return s < 2
";
    let result = minify_luau(source);
    assert_eq!(result, "return(1::number)<2");
    assert_eq!(minify_luau(&result), result, "must be idempotent");
}

#[test]
fn function_inside_a_type_survives_minification() {
    for (source, expected) in [
        ("type T = typeof(function(x) return x end)\n", "type T=typeof(function(l)return l end)"),
        (
            "return f<<typeof(function(x) return x end)>>()\n",
            "return f<<typeof(function(l)return l end)>>()",
        ),
    ] {
        let result = minify_luau(source);
        assert_eq!(result, expected, "source: {source}");
        assert_eq!(minify_luau(&result), result, "must be idempotent");
    }
}

#[test]
fn shebang_stays_on_the_first_line() {
    let output = minify_lua54(
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
    assert_eq!(minify_lua54(&output), output, "must be idempotent");
}

#[test]
fn shebang_drops_trailing_carriage_returns() {
    for source in ["#!x\r\r", "#!x\r\r\nprint(1)\n"] {
        let output = minify_lua54(source);
        assert!(output.starts_with("#!x\n"), "{output:?}");
        assert_eq!(minify_lua54(&output), output, "must be idempotent");
    }
}

#[test]
fn shebang_keeps_a_lone_carriage_return_as_text() {
    let output = minify_lua54("#!x\ry = {\nprint(1)\n");
    assert_eq!(output, "#!x\ry = {\nprint(1)");
    assert_eq!(minify_lua54(&output), output, "must be idempotent");
}

#[test]
fn shebang_stays_above_luau_hot_comments() {
    let output = minify_luau(
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
    assert_eq!(minify_luau(&output), output, "must be idempotent");
}

#[test]
fn luau_hot_comments_survive_minification() {
    let output = minify_luau(
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
    let second = minify_luau(&output);
    assert_eq!(output, second, "hot comment prefix must be stable");
}

#[test]
fn luau_plain_comments_are_dropped() {
    let output = minify_luau(
        "\
-- plain comment
return 1
",
    );
    assert!(!output.contains("plain comment"), "{output}");
}

#[test]
fn lua55_named_vararg_keeps_its_binding_over_a_same_named_local() {
    let source = "\
local args = 1
local function f(...args) return args end
print(f(2, 3))
";
    let result = minify_with(source, LuaVersion::Lua55, &TransformConfig::default());
    assert!(!result.contains("return 1"), "the vararg read must stay one: {result}");
    let (vararg_name, returned) =
        result.split_once("(...").and_then(|(_, rest)| rest.split_once(")return ")).expect(&result);
    assert!(returned.starts_with(vararg_name), "{result}");
    assert_eq!(minify_with(&result, LuaVersion::Lua55, &TransformConfig::default()), result);
}
