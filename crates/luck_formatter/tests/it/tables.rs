use luck_formatter::FormatOptions;
use luck_token::LuaVersion;

use crate::common::{assert_format, assert_format_with};

#[test]
fn empty_table() {
    assert_format("local t={}\n", "local t = {}\n");
}

#[test]
fn flat_table_named() {
    assert_format("local t={a=1,b=2}\n", "local t = { a = 1, b = 2 }\n");
}

#[test]
fn expanded_table() {
    let options = FormatOptions { line_width: 30, ..FormatOptions::default() };
    assert_format_with(
        "local t={very_long_key=1,another_key=2,third_key=3}\n",
        "local t = {\n\tvery_long_key = 1,\n\tanother_key = 2,\n\tthird_key = 3,\n}\n",
        LuaVersion::Lua54,
        &options,
    );
}

#[test]
fn semicolons_to_commas() {
    assert_format("local t={a=1;b=2}\n", "local t = { a = 1, b = 2 }\n");
}

#[test]
fn positional_fields() {
    assert_format("local t={1,2,3}\n", "local t = { 1, 2, 3 }\n");
}

#[test]
fn bracketed_field() {
    assert_format("local t={[\"key\"]=1}\n", "local t = { [\"key\"] = 1 }\n");
}

#[test]
fn nested_table() {
    assert_format("local t={a={1,2}}\n", "local t = { a = { 1, 2 } }\n");
}

#[test]
fn mixed_positional_and_named_fields() {
    assert_format("local t={1,2,x=3,y=4}\n", "local t = { 1, 2, x = 3, y = 4 }\n");
}

#[test]
fn deeply_nested_table() {
    assert_format("local t={a={b={c=1}}}\n", "local t = { a = { b = { c = 1 } } }\n");
}

#[test]
fn long_single_field_expanded() {
    let options = FormatOptions { line_width: 30, ..FormatOptions::default() };
    assert_format_with(
        "local t={very_long_field_name_here=some_very_long_value_here}\n",
        "local t = {\n\tvery_long_field_name_here = some_very_long_value_here,\n}\n",
        LuaVersion::Lua54,
        &options,
    );
}

#[test]
fn long_string_bracket_key_keeps_space() {
    // Without the space, `[[[k]]]` lexes its first two brackets as a
    // long-string opener.
    assert_format("local t = { [ [[k]] ] = 1 }\n", "local t = { [ [[k]] ] = 1 }\n");
}

#[test]
fn long_string_index_keeps_space() {
    assert_format("t[ [[k]] ] = 2\n", "t[ [[k]] ] = 2\n");
}

#[test]
fn blank_line_between_fields_is_kept() {
    let source = "local t = {\n\ta = 1,\n\n\tb = 2,\n\tc = 3,\n}\n";
    assert_format(source, source);
    // A run of blank lines is one, and the edges of the table take none.
    assert_format(
        "local t = {\n\n\ta = 1,\n\n\n\tb = 2,\n\n}\n",
        "local t = {\n\ta = 1,\n\n\tb = 2,\n}\n",
    );
}

#[test]
fn blank_line_between_fields_breaks_a_table_that_fits() {
    assert_format("local t = { a = 1,\n\n b = 2 }\n", "local t = {\n\ta = 1,\n\n\tb = 2,\n}\n");
}

#[test]
fn blank_line_groups_a_packed_list() {
    let source =
        "local libs = {\n\t\"roact\", \"rodux\", \"cryo\",\n\n\t\"testez\", \"otter\",\n}\n";
    assert_format(source, source);
}

#[test]
fn blank_line_between_commented_fields_is_kept() {
    let source = "local Status = {\n\t-- nothing runs\n\tDisabled = \"Disabled\",\n\n\t-- everything runs\n\tEnabled = \"Enabled\",\n}\n";
    assert_format(source, source);
    // The comment right after the opener takes no blank line above it.
    assert_format(
        "local Status = {\n\n\t-- nothing runs\n\tDisabled = \"Disabled\",\n}\n",
        "local Status = {\n\t-- nothing runs\n\tDisabled = \"Disabled\",\n}\n",
    );
}

#[test]
fn fill_entry_with_grouped_dot_chain() {
    // A dot chain of three or more accesses gets its own group inside the
    // fill entry, and the fill printer must not take that group's soft lines
    // for entry separators.
    assert_format("local t={a.b.c.d,x,y}\n", "local t = { a.b.c.d, x, y }\n");
}
