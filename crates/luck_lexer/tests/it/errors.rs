use crate::common::lex51;

#[test]
fn error_unterminated_string() {
    let result = lex51("\"hello");
    assert!(!result.errors.is_empty());
    assert!(result.errors[0].message.contains("unterminated"));
}

#[test]
fn error_unterminated_block_comment() {
    let result = lex51("--[[ oops");
    assert!(!result.errors.is_empty());
    assert!(result.errors[0].message.contains("unterminated"));
}

#[test]
fn error_invalid_escape() {
    let result = lex51("\"\\q\"");
    assert!(!result.errors.is_empty());
    assert!(result.errors[0].message.contains("invalid escape"));
}

#[test]
fn error_decimal_escape_too_large() {
    let result = lex51("\"\\256\"");
    assert!(!result.errors.is_empty());
    assert!(result.errors[0].message.contains("too large"));
}

#[test]
fn error_unterminated_long_string() {
    let result = lex51("[[oops");
    assert!(!result.errors.is_empty());
    assert!(result.errors[0].message.contains("unterminated"));
}
