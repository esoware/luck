use crate::common::{minify_lua54, reparses};

#[test]
fn comment_heavy_input_does_not_reserve_source_sized_output() {
    let source = format!("--{}\nreturn 1", "comment ".repeat(8192));
    let output = minify_lua54(&source);
    assert_eq!(output, "return 1");
    assert!(output.capacity() < 4096, "output capacity: {}", output.capacity());
    assert_eq!(minify_lua54(&output), output);
    assert!(reparses(&output));
}

#[test]
fn strips_comments_and_whitespace() {
    let result = minify_lua54("-- comment\nlocal x = 1 -- inline\n--[[ block ]]\nreturn x\n");
    assert!(!result.contains("--"));
    assert!(!result.contains("comment"));
    assert!(!result.contains("block"));
}

#[test]
fn output_has_no_double_spaces() {
    let result = minify_lua54("local   x   =   1\nreturn   x\n");
    assert!(!result.contains("  "));
}

#[test]
fn chained_calls_produce_valid_output() {
    let result = minify_lua54("f()\ng()\nh()\n");
    assert!(reparses(&result), "Parse errors: {result}");
    assert!(
        result.contains("f(") && result.contains("g(") && result.contains("h("),
        "All calls must be preserved: {result}"
    );
}

#[test]
fn minus_does_not_become_comment() {
    // `x - -y` must not become `x--y`, which lexes as a comment.
    let result = minify_lua54("local x = 5\nlocal y = 3\nreturn x - -y\n");
    assert!(!result.contains("--"), "Minus became comment: {result}");
}

#[test]
fn and_or_have_spaces() {
    let result = minify_lua54("local x = a and b\nlocal y = a or b\nreturn x, y\n");
    assert!(result.contains(" and ") || result.contains(" and\n"), "and needs spaces: {result}");
    assert!(result.contains(" or ") || result.contains(" or\n"), "or needs spaces: {result}");
}

#[test]
fn strips_trailing_comma_in_table() {
    let result = minify_lua54("return {1, 2, 3}\n");
    assert!(
        !result.contains(",}") && !result.contains(", }"),
        "Should not have trailing comma, got: {result}"
    );
}

#[test]
fn empty_input_minifies_to_empty_output() {
    let result = minify_lua54("");
    assert!(
        result.is_empty() || result.trim().is_empty(),
        "Empty input should produce empty or near-empty output, got: {result}"
    );
}
