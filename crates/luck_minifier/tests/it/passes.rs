use luck_core::TransformConfig;
use luck_token::LuaVersion;

use crate::common::{minify_lua54, minify_luau, minify_with, reparses};

#[test]
fn folds_arithmetic() {
    let result = minify_lua54("local x = 1 + 2\nreturn x\n");
    assert!(result.contains('3'), "Expected folded 3, got: {result}");
    assert!(!result.contains('+'), "Expected no +, got: {result}");
}

#[test]
fn folds_string_concat() {
    let result = minify_lua54("local x = \"hello\" .. \" world\"\nreturn x\n");
    assert!(result.contains("\"hello world\""), "Got: {result}");
}

#[test]
fn folds_not_true() {
    let result = minify_lua54("local x = not true\nreturn x\n");
    assert!(result.contains("false"), "Got: {result}");
}

#[test]
fn hash_string_not_folded() {
    // #"str" must not be folded: escape sequences make the raw length
    // unreliable.
    let result = minify_lua54("local x = #\"hello\"\nreturn x\n");
    assert!(result.contains('#'), "Got: {result}");
}

#[test]
fn no_fold_division_by_zero() {
    let result = minify_lua54("local x = 1 / 0\nreturn x\n");
    assert!(result.contains('/'), "Should not fold div by zero: {result}");
}

#[test]
fn folds_exponentiation() {
    let result = minify_lua54("local x = 2 ^ 10\nreturn x\n");
    assert!(result.contains("1024"), "2^10 should be folded to 1024, got: {result}");
}

#[test]
fn no_fold_overflow_to_inf() {
    let result = minify_lua54("local x = 1e308 * 10\nreturn x\n");
    assert!(result.contains('*'), "Should not fold to inf: {result}");
}

#[test]
fn disabled_folding_keeps_arithmetic() {
    let config = TransformConfig { fold_constants: false, ..Default::default() };
    let result = minify_with("local x = 1 + 2\nreturn x\n", LuaVersion::Lua54, &config);
    assert!(result.contains('+'), "Folding should be disabled: {result}");
}

#[test]
fn inlining_keeps_operand_grouping_through_the_pipeline() {
    let no_folding = TransformConfig { fold_constants: false, ..Default::default() };
    let cases = [
        ("local s = 1 / 0\nreturn 3 % s\n", "return 3%(1/0)"),
        ("local s = 1 / 0\nreturn s\n", "return 1/0"),
        ("local s = -2\nreturn s ^ y\n", "return(-2)^y"),
        ("local s = -2\nreturn s ^ 2\n", "return 4."),
    ];
    for (source, expected) in cases {
        let result = minify_lua54(source);
        assert_eq!(result, expected, "source: {source}");
        assert_eq!(minify_lua54(&result), result, "must be idempotent");
    }
    for (source, expected) in [
        ("local s = 1 + 2\nreturn s * 2\n", "return(1+2)*2"),
        ("local s = 1 + 2\nreturn s\n", "return 1+2"),
        ("local s = 1 + 2\nreturn -s\n", "return-(1+2)"),
    ] {
        let result = minify_with(source, LuaVersion::Lua54, &no_folding);
        assert_eq!(result, expected, "source: {source}");
        assert_eq!(
            minify_with(&result, LuaVersion::Lua54, &no_folding),
            result,
            "must be idempotent"
        );
    }
}

#[test]
fn shortens_trailing_zero() {
    let result = minify_lua54("local x = 1.0\nreturn x\n");
    assert!(!result.contains("1.0"), "Got: {result}");
}

#[test]
fn shortens_leading_zero() {
    let result = minify_lua54("local x = 0.5\nreturn x\n");
    assert!(result.contains(".5"), "Got: {result}");
}

#[test]
fn shortens_large_round_numbers() {
    // 5.4 has integer/float subtypes: `1e6` is a float, so the
    // integer literal must stay spelled as an integer.
    let result = minify_lua54("local x = 1000000\nreturn x\n");
    assert!(result.contains("1000000"), "Got: {result}");

    // Luau's single number type makes the scientific form safe.
    let luau_result = minify_luau("local x = 1000000\nreturn x\n");
    assert!(luau_result.contains("1e6"), "Got: {luau_result}");
}

#[test]
fn removes_parens_around_literal() {
    let result = minify_lua54("local x = (42)\nreturn x\n");
    assert!(!result.contains("(42)"), "Got: {result}");
}

#[test]
fn keeps_parens_around_function_call() {
    // Parens around a call truncate its multiple returns.
    let result = minify_lua54("local x = (foo())\nreturn x\n");
    let paren_count = result.matches('(').count();
    assert!(
        paren_count >= 2,
        "Should keep wrapping parens (expected >=2 '(' chars), got {paren_count} in: {result}"
    );
}

#[test]
fn concat_right_associativity_preserved() {
    // Concat is right-associative, so (a .. b) .. c differs from
    // a .. b .. c and the parens must survive.
    let result = minify_lua54("return (a .. b) .. c\n");
    assert!(reparses(&result), "Parse errors\nOutput: {result}");
    assert!(
        result.contains('('),
        "Parens around left-side concat must be kept (changes associativity): {result}"
    );
}

#[test]
fn parens_around_returned_call_are_kept() {
    // (f()) truncates f's multiple returns, so the parens must be kept.
    let result = minify_lua54("return (f())\n");
    assert!(
        result.contains("(f())"),
        "Parens around f() in return must be preserved (multi-return truncation): {result}"
    );
}

#[test]
fn strips_default_for_step() {
    let result = minify_lua54("for i = 1, 10, 1 do print(i) end\n");
    assert!(
        !result.contains(",1 do")
            && !result.contains(",1,")
            && !result.contains(", 1 do")
            && !result.contains(", 1,"),
        "Default step=1 should be removed, got: {result}"
    );
}

#[test]
fn dead_local_with_call_extracts_side_effect() {
    let result = minify_lua54("local unused = foo()\n");
    assert!(
        result.contains("foo("),
        "Side-effectful call should be preserved even when result unused: {result}"
    );
    assert!(!result.contains("local"), "Unused local declaration should be removed: {result}");
}

#[test]
fn metamethod_safe_dead_code_preserves_variable_ops() {
    // x + 1 is not pure: x may carry a side-effecting __add metamethod.
    let result = minify_lua54("local unused = x + 1\n");
    assert!(result.contains('+'), "Should preserve x + 1 (metamethod side effects), got: {result}");
}

#[test]
fn metamethod_safe_dead_code_removes_literal_ops() {
    // 1 + 2 is pure: literal numbers carry no metamethods.
    let result = minify_lua54("local unused = 1 + 2\n");
    assert!(!result.contains('+'), "Should remove literal 1 + 2 (no metamethods), got: {result}");
}

#[test]
fn metamethod_safe_truthiness_preserves_variable_ops() {
    // x + 1 is not guaranteed truthy: x's __add may return nil.
    let source = "local x = setmetatable({}, {__add = function() return nil end})\nif true then return x + 1 end\nreturn 0\n";
    let result = minify_lua54(source);
    assert!(
        !result.contains(" and "),
        "Should not simplify to and/or (metamethod may return falsy), got: {result}"
    );
}

#[test]
fn metamethod_safe_truthiness_simplifies_literal_ops() {
    let result = minify_lua54("if true then return 1 + 2 end\nreturn 0\n");
    assert!(reparses(&result), "Parse errors\nOutput: {result}");
    assert!(result.contains('3'), "1+2 should be folded to 3, got: {result}");
    assert!(!result.contains("if"), "if true should be eliminated, got: {result}");
}
