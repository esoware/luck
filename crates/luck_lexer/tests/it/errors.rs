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
    // 5.2+ and Luau reject undefined escapes; real 5.1 accepts them as
    // the literal character (5.2 §8.1 documents the tightening).
    for version in [luck_token::LuaVersion::Lua52, luck_token::LuaVersion::Luau] {
        let result = luck_lexer::lex("\"\\q\"", version);
        assert!(!result.errors.is_empty(), "{version:?} must reject \\q");
        assert!(result.errors[0].message.contains("invalid escape"));
    }
    let result = lex51("\"\\q\"");
    assert!(result.errors.is_empty(), "5.1 accepts \\q as literal 'q'");
}

#[test]
fn error_invalid_multibyte_escape_recovers_on_char_boundary() {
    // Fuzz-found: the invalid-escape error path used to advance one byte
    // past a multi-byte escaped char, leaving the cursor mid-sequence and
    // panicking on the next token's source slice.
    let result = luck_lexer::lex("\"\\\u{7C5}\" y", luck_token::LuaVersion::Lua55);
    assert!(!result.errors.is_empty(), "{:?}", result.errors);
    assert!(result.errors[0].message.contains("invalid escape"));
    assert!(
        result.errors[0].message.contains('\u{7C5}'),
        "message should carry the full char, not its first byte: {:?}",
        result.errors[0].message
    );
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
