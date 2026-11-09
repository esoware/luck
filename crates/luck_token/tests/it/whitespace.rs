use LuaVersion::{Lua51, Lua54, Luau};
use luck_token::{
    LuaVersion, chunk_start, ends_with_line_break, has_line_break, is_lua_whitespace, line_breaks,
    strip_line_break,
};

#[test]
fn puc_reads_lf_cr_crlf_and_lfcr_as_one_line_break_each() {
    for version in [Lua51, Lua54] {
        assert_eq!(
            line_breaks("a\nb\rc\r\nd\n\re", version).collect::<Vec<_>>(),
            [1..2, 3..4, 5..7, 8..10],
            "{version:?}"
        );
        assert!(has_line_break("a\rb", version));
        assert!(ends_with_line_break("a\r", version));
        assert!(ends_with_line_break("a\n\r", version));
    }
}

#[test]
fn luau_counts_only_lf_as_a_line_break() {
    assert_eq!(line_breaks("a\nb\rc\r\nd\n\re", Luau).collect::<Vec<_>>(), [1..2, 5..7, 8..9]);
    assert!(!has_line_break("a\rb", Luau));
    assert!(has_line_break("a\r\nb", Luau));
    assert!(!ends_with_line_break("a\r", Luau));
    assert!(ends_with_line_break("a\r\n", Luau));
}

#[test]
fn text_without_a_line_break_has_none() {
    for version in LuaVersion::ALL {
        assert_eq!(line_breaks("", version).next(), None);
        assert_eq!(line_breaks("no break\t\x0B\x0C", version).next(), None);
        assert!(!has_line_break("no break", version));
        assert!(!ends_with_line_break("", version));
    }
}

#[test]
fn strip_line_break_takes_one_whole_break_or_none() {
    assert_eq!(strip_line_break(b"\r\nx", Lua54), Some(&b"x"[..]));
    assert_eq!(strip_line_break(b"\n\rx", Lua54), Some(&b"x"[..]));
    assert_eq!(strip_line_break(b"\rx", Lua54), Some(&b"x"[..]));
    assert_eq!(strip_line_break(b"\r\nx", Luau), Some(&b"x"[..]));
    assert_eq!(strip_line_break(b"\n\rx", Luau), Some(&b"\rx"[..]));
    assert_eq!(strip_line_break(b"\rx", Luau), None);
    assert_eq!(strip_line_break(b"x", Lua54), None);
}

#[test]
fn only_c_locale_isspace_bytes_are_lua_whitespace() {
    for b in [b' ', b'\t', b'\n', b'\r', 0x0B, 0x0C] {
        assert!(is_lua_whitespace(b), "{b:#x}");
    }
    for b in [b'a', 0x00, 0x08, 0xA0, 0xFE] {
        assert!(!is_lua_whitespace(b), "{b:#x}");
    }
}

#[test]
fn chunk_start_skips_a_bom_only_where_the_loader_does() {
    assert_eq!(chunk_start("\u{FEFF}return", Lua54), 3);
    assert_eq!(chunk_start("return", Lua54), 0);
    assert_eq!(chunk_start("\u{FEFF}return", Lua51), 0);
    assert_eq!(chunk_start("\u{FEFF}return", Luau), 0);
}
