use luck_token::LineIndex;

#[test]
fn line_index_counts_lines_and_byte_columns_from_one() {
    let source = "\
local x
do local y
end";
    let lines = LineIndex::new(source.as_bytes());
    assert_eq!(lines.line_column(0), (1, 1));
    assert_eq!(lines.line_column(6), (1, 7));
    // The `d` of `do`.
    assert_eq!(lines.line_column(8), (2, 1));
    assert_eq!(lines.line_column(17), (2, 10));
    assert_eq!(lines.line(17), 2);
    let latin1 = LineIndex::new(b"-- \xE9\xE8\nx");
    assert_eq!(latin1.line_column(5), (1, 6));
    assert_eq!(latin1.line_column(6), (2, 1));
}

#[test]
fn line_index_breaks_lines_only_at_an_lf() {
    assert_eq!(LineIndex::new(b"a\rb").line(2), 1);
    assert_eq!(LineIndex::new(b"a\r\nb").line_column(3), (2, 1));
    assert_eq!(LineIndex::new(b"#!x\ry\r\nz").line_column(7), (2, 1));
}

#[test]
fn line_index_counts_a_bom_as_part_of_the_first_line() {
    let source = "\u{feff}local x
y";
    assert_eq!(LineIndex::new(source.as_bytes()).line_column(3), (1, 4));
    assert_eq!(LineIndex::new(source.as_bytes()).line_column(11), (2, 1));
}

#[test]
fn line_index_offset_inverts_line_column() {
    let source = "\
local x
do local y
end";
    let lines = LineIndex::new(source.as_bytes());
    for offset in [0, 6, 8, 17] {
        let (line, column) = lines.line_column(offset);
        assert_eq!(lines.offset(line, column), Some(offset), "at {offset}");
    }
    assert_eq!(lines.offset(0, 1), None);
    assert_eq!(lines.offset(4, 1), None);
}
