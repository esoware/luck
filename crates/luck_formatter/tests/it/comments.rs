use crate::common::{assert_format, assert_format_with};
use luck_formatter::{FormatOptions, LineEndings};
use luck_token::LuaVersion;

#[test]
fn leading_comment() {
    assert_format("-- comment\nlocal x = 1\n", "-- comment\nlocal x = 1\n");
}

#[test]
fn trailing_comment() {
    assert_format("local x = 1 -- comment\n", "local x = 1 -- comment\n");
}

#[test]
fn comment_after_only_empty_statements() {
    // Fuzz-found: a block holding only dropped `;` statements counted as
    // "has statements", opening a spurious line before the comment flush.
    assert_format(";-- a\n", "-- a\n");
    assert_format(";\n-- a\n-- b\n", "-- a\n-- b\n");
}

#[test]
fn comment_between_statements() {
    assert_format(
        "local x = 1\n-- between\nlocal y = 2\n",
        "local x = 1\n-- between\nlocal y = 2\n",
    );
}

#[test]
fn block_comment_verbatim() {
    assert_format(
        "--[[ block ]]\nlocal x = 1\n",
        "--[[ block ]]\nlocal x = 1\n",
    );
}

#[test]
fn shebang_preserved() {
    assert_format(
        "#!/usr/bin/env lua\nlocal x = 1\n",
        "#!/usr/bin/env lua\nlocal x = 1\n",
    );
}

#[test]
fn blank_line_preserved() {
    assert_format(
        "local x = 1\n\nlocal y = 2\n",
        "local x = 1\n\nlocal y = 2\n",
    );
}

#[test]
fn multiple_blank_lines_collapsed() {
    assert_format(
        "local x = 1\n\n\n\nlocal y = 2\n",
        "local x = 1\n\nlocal y = 2\n",
    );
}

#[test]
fn trailing_newline_added() {
    assert_format("local x = 1", "local x = 1\n");
}

#[test]
fn comment_at_eof_no_trailing_newline() {
    assert_format("local x = 1 -- end", "local x = 1 -- end\n");
}

#[test]
fn comment_inside_empty_function() {
    assert_format(
        "function foo()\n-- nothing\nend\n",
        "function foo()\n\t-- nothing\nend\n",
    );
}

#[test]
fn format_off_region() {
    assert_format(
        "local   x  =  1\n-- luck: format off\nlocal   y  =  2\n-- luck: format on\nlocal   z  =  3\n",
        "local x = 1\n-- luck: format off\nlocal   y  =  2\n-- luck: format on\nlocal z = 3\n",
    );
}

#[test]
fn format_off_unclosed_extends_to_eof() {
    let input = "local   x  =  1\n-- luck: format off\nlocal   y  =  2\nlocal   z  =  3\n";
    let result = luck_formatter::format(input, LuaVersion::Lua54, &FormatOptions::default());
    assert!(
        result.errors.is_empty(),
        "parse errors: {:?}",
        result.errors
    );
    assert!(
        result.output.contains("local x = 1"),
        "x should be formatted: {}",
        result.output
    );
    assert!(
        result.output.contains("local   y  =  2"),
        "y should be preserved: {}",
        result.output
    );
    assert!(
        result.output.contains("local   z  =  3"),
        "z should be preserved: {}",
        result.output
    );
}

#[test]
fn format_ignore_single_statement() {
    assert_format(
        "-- luck: ignore\nlocal   x  =  1\nlocal   y  =  2\n",
        "-- luck: ignore\nlocal   x  =  1\nlocal y = 2\n",
    );
}

#[test]
fn windows_line_endings() {
    let options = FormatOptions {
        line_endings: LineEndings::Windows,
        ..FormatOptions::default()
    };
    let result = luck_formatter::format("if x then\ny=1\nend\n", LuaVersion::Lua54, &options);
    assert!(result.errors.is_empty());
    assert_eq!(result.output, "if x then\r\n\ty = 1\r\nend\r\n");
}

#[test]
fn format_range_only_formats_selection() {
    let input = "local   x  =  1\nlocal   y  =  2\nlocal   z  =  3\n";
    // Range covers only the second statement (byte offsets for "local   y  =  2")
    let second_start = input.find("local   y").unwrap();
    let second_end = input[second_start..].find('\n').unwrap() + second_start;
    let result = luck_formatter::format_range(
        input,
        LuaVersion::Lua54,
        &FormatOptions::default(),
        second_start..second_end,
    );
    assert!(
        result.errors.is_empty(),
        "parse errors: {:?}",
        result.errors
    );
    assert!(
        result.output.contains("local   x  =  1"),
        "x should be verbatim (outside range): {}",
        result.output
    );
    assert!(
        result.output.contains("local y = 2"),
        "y should be formatted (inside range): {}",
        result.output
    );
    assert!(
        result.output.contains("local   z  =  3"),
        "z should be verbatim (outside range): {}",
        result.output
    );
}

#[test]
fn fill_mode_packs_positional_table_entries() {
    let options = FormatOptions {
        line_width: 20,
        ..FormatOptions::default()
    };
    // Fill mode packs as many positional values per line as fit the width
    // (the old printer overflowed the limit here; wrapping after `6,` is the
    // width-correct layout).
    assert_format_with(
        "local t = {1, 2, 3, 4, 5, 6, 7, 8}\n",
        "local t = {\n\t1, 2, 3, 4, 5, 6,\n\t7, 8,\n}\n",
        LuaVersion::Lua54,
        &options,
    );
}
