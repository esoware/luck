use crate::common::{minify_default, reparses};

#[test]
fn comment_heavy_input_does_not_reserve_source_sized_output() {
    let source = format!(
        "\
--{}
return 1",
        "comment ".repeat(8192)
    );
    let output = minify_default(&source);
    assert_eq!(output, "return 1");
    assert!(output.capacity() < 4096, "output capacity: {}", output.capacity());
    assert_eq!(minify_default(&output), output);
    assert!(reparses(&output));
}

#[test]
fn strips_comments_and_whitespace() {
    let result = minify_default(
        "\
-- comment
local x = 1 -- inline
--[[ block ]]
return x
",
    );
    assert!(!result.contains("--"));
    assert!(!result.contains("comment"));
    assert!(!result.contains("block"));
}

#[test]
fn output_has_no_double_spaces() {
    let result = minify_default(
        "\
local   x   =   1
return   x
",
    );
    assert!(!result.contains("  "));
}

#[test]
fn chained_calls_produce_valid_output() {
    let result = minify_default(
        "\
f()
g()
h()
",
    );
    assert!(reparses(&result), "Parse errors: {result}");
    assert!(
        result.contains("f(") && result.contains("g(") && result.contains("h("),
        "All calls must be preserved: {result}"
    );
}

#[test]
fn minus_does_not_become_comment() {
    // `x - -y` must not become `x--y`, which lexes as a comment.
    let result = minify_default(
        "\
local x = 5
local y = 3
return x - -y
",
    );
    assert!(!result.contains("--"), "Minus became comment: {result}");
}

#[test]
fn and_or_have_spaces() {
    let result = minify_default(
        "\
local x = a and b
local y = a or b
return x, y
",
    );
    assert!(result.contains(" and ") || result.contains(" and\n"), "and needs spaces: {result}");
    assert!(result.contains(" or ") || result.contains(" or\n"), "or needs spaces: {result}");
}

#[test]
fn strips_trailing_comma_in_table() {
    let result = minify_default("return {1, 2, 3}\n");
    assert!(
        !result.contains(",}") && !result.contains(", }"),
        "Should not have trailing comma, got: {result}"
    );
}

#[test]
fn empty_input_minifies_to_empty_output() {
    let result = minify_default("");
    assert!(
        result.is_empty() || result.trim().is_empty(),
        "Empty input should produce empty or near-empty output, got: {result}"
    );
}

#[test]
fn shebang_stays_on_the_first_line() {
    let output = minify_default(
        "\
#!/usr/bin/env luau
print(1)
",
    );
    assert_eq!(
        output,
        "\
#!/usr/bin/env luau
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
