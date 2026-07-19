use crate::common::{assert_format, assert_format_with};
use luck_formatter::FormatOptions;
use luck_token::LuaVersion;

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
    let opts = FormatOptions {
        line_width: 30,
        ..FormatOptions::default()
    };
    assert_format_with(
        "local t={very_long_key=1,another_key=2,third_key=3}\n",
        "local t = {\n\tvery_long_key = 1,\n\tanother_key = 2,\n\tthird_key = 3,\n}\n",
        LuaVersion::Lua54,
        &opts,
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
    assert_format(
        "local t={1,2,x=3,y=4}\n",
        "local t = { 1, 2, x = 3, y = 4 }\n",
    );
}

#[test]
fn deeply_nested_table() {
    assert_format(
        "local t={a={b={c=1}}}\n",
        "local t = { a = { b = { c = 1 } } }\n",
    );
}

#[test]
fn long_single_field_expanded() {
    let opts = FormatOptions {
        line_width: 30,
        ..FormatOptions::default()
    };
    assert_format_with(
        "local t={very_long_field_name_here=some_very_long_value_here}\n",
        "local t = {\n\tvery_long_field_name_here = some_very_long_value_here,\n}\n",
        LuaVersion::Lua54,
        &opts,
    );
}

#[test]
fn long_string_bracket_key_keeps_space() {
    // `[[[k]]]` would re-lex the first two brackets as a long-string
    // opener; the space is load-bearing.
    assert_format(
        "local t = { [ [[k]] ] = 1 }\n",
        "local t = { [ [[k]] ] = 1 }\n",
    );
}

#[test]
fn long_string_index_keeps_space() {
    assert_format("t[ [[k]] ] = 2\n", "t[ [[k]] ] = 2\n");
}

#[test]
fn fill_entry_with_grouped_dot_chain() {
    // A >=3-segment access chain forms its own group inside a fill entry;
    // the fill printer must not mistake the group's soft lines for entry
    // separators.
    assert_format("local t={a.b.c.d,x,y}\n", "local t = { a.b.c.d, x, y }\n");
}
