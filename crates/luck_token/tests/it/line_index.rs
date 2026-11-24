use LuaVersion::{Lua51, Lua54, Luau};
use luck_token::{LineIndex, LuaVersion};

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
