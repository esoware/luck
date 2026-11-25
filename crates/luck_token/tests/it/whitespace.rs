use luck_token::{
    ends_with_line_break, has_line_break, is_luau_whitespace, line_breaks, strip_line_break,
};

#[test]
fn only_an_lf_breaks_a_line_and_a_crlf_is_one_break() {
    assert_eq!(line_breaks(b"a\nb\rc\r\nd\n\re").collect::<Vec<_>>(), [1..2, 5..7, 8..9]);
    assert!(!has_line_break(b"a\rb"));
    assert!(has_line_break(b"a\r\nb"));
    assert!(!ends_with_line_break(b"a\r"));
    assert!(ends_with_line_break(b"a\r\n"));
    assert!(ends_with_line_break(b"a\n"));
}

#[test]
fn text_without_a_line_break_has_none() {
    assert_eq!(line_breaks(b"").next(), None);
    assert_eq!(line_breaks(b"no break\t\r\x0B\x0C").next(), None);
    assert!(!has_line_break(b"no break"));
    assert!(!ends_with_line_break(b""));
}

#[test]
fn strip_line_break_takes_one_whole_break_or_none() {
    assert_eq!(strip_line_break(b"\r\nx"), Some(&b"x"[..]));
    assert_eq!(strip_line_break(b"\nx"), Some(&b"x"[..]));
    assert_eq!(strip_line_break(b"\n\rx"), Some(&b"\rx"[..]));
    assert_eq!(strip_line_break(b"\rx"), None);
    assert_eq!(strip_line_break(b"x"), None);
}

#[test]
fn only_c_locale_isspace_bytes_are_luau_whitespace() {
    for b in [b' ', b'\t', b'\n', b'\r', 0x0B, 0x0C] {
        assert!(is_luau_whitespace(b), "{b:#x}");
    }
    for b in [b'a', 0x00, 0x08, 0xA0, 0xFE] {
        assert!(!is_luau_whitespace(b), "{b:#x}");
    }
}
