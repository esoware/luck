use LuaVersion::{Lua51, Lua54, Luau};
use luck_token::{
    LineIndex, LuaVersion, chunk_start, ends_with_line_break, has_line_break, is_lua_whitespace,
    line_breaks, strip_line_break,
};

#[test]
fn puc_reads_lf_cr_crlf_and_lfcr_as_one_line_break_each() {
    for version in [Lua51, Lua54] {
        assert_eq!(
            line_breaks(b"a\nb\rc\r\nd\n\re", version).collect::<Vec<_>>(),
            [1..2, 3..4, 5..7, 8..10],
            "{version:?}"
        );
        assert!(has_line_break(b"a\rb", version));
        assert!(ends_with_line_break(b"a\r", version));
        assert!(ends_with_line_break(b"a\n\r", version));
    }
}

#[test]
fn luau_counts_only_lf_as_a_line_break() {
    assert_eq!(line_breaks(b"a\nb\rc\r\nd\n\re", Luau).collect::<Vec<_>>(), [1..2, 5..7, 8..9]);
    assert!(!has_line_break(b"a\rb", Luau));
    assert!(has_line_break(b"a\r\nb", Luau));
    assert!(!ends_with_line_break(b"a\r", Luau));
    assert!(ends_with_line_break(b"a\r\n", Luau));
}

#[test]
fn text_without_a_line_break_has_none() {
    for version in LuaVersion::ALL {
        assert_eq!(line_breaks(b"", version).next(), None);
        assert_eq!(line_breaks(b"no break\t\x0B\x0C", version).next(), None);
        assert!(!has_line_break(b"no break", version));
        assert!(!ends_with_line_break(b"", version));
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
    let with_bom = "\u{FEFF}return".as_bytes();
    assert_eq!(chunk_start(with_bom, Lua54), 3);
    assert_eq!(chunk_start(b"return", Lua54), 0);
    assert_eq!(chunk_start(with_bom, Lua51), 0);
    assert_eq!(chunk_start(with_bom, Luau), 0);
}

#[test]
fn line_index_counts_lines_and_byte_columns_from_one() {
    let source = "\
local x
do local y
end";
    let lines = LineIndex::new(source.as_bytes(), Lua54);
    assert_eq!(lines.line_column(0), (1, 1));
    assert_eq!(lines.line_column(6), (1, 7));
    // The `d` of `do`.
    assert_eq!(lines.line_column(8), (2, 1));
    assert_eq!(lines.line_column(17), (2, 10));
    assert_eq!(lines.line(17), 2);
    let latin1 = LineIndex::new(b"-- \xE9\xE8\nx", Lua54);
    assert_eq!(latin1.line_column(5), (1, 6));
    assert_eq!(latin1.line_column(6), (2, 1));
}

#[test]
fn line_index_breaks_lines_as_the_version_does() {
    assert_eq!(LineIndex::new(b"a\rb", Lua54).line(2), 2);
    assert_eq!(LineIndex::new(b"a\rb", Luau).line(2), 1);
}

#[test]
fn line_index_reads_a_shebang_as_one_line_up_to_its_lf() {
    for version in LuaVersion::ALL {
        assert_eq!(LineIndex::new(b"#!x\ry\r\nz", version).line_column(7), (2, 1), "{version:?}");
    }
}

#[test]
fn line_index_starts_the_first_line_past_a_skipped_bom() {
    let source = "\u{feff}local x
y";
    assert_eq!(LineIndex::new(source.as_bytes(), Lua54).line_column(3), (1, 1));
    assert_eq!(LineIndex::new(source.as_bytes(), Lua54).line_column(11), (2, 1));
    assert_eq!(LineIndex::new(source.as_bytes(), Lua51).line_column(3), (1, 4));
}
