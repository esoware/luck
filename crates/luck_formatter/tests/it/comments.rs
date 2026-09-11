use crate::common::{assert_format, assert_format_with};
use luck_formatter::{FormatOptions, LineEndings};
use luck_token::LuaVersion;

#[test]
fn interior_comments_never_cross_statement_boundaries() {
    let sources = [
        "if (first --[[ condition ]] and second) then run() end",
        "if first then run() elseif second --[[ branch ]] then stop() else finish() end",
        "while first --[[ loop ]] do run() end",
        "repeat run() until first --[[ condition ]] and second",
        "local function call() if first --[[ nested ]] then run() end end",
        "local values = {first, --[[ item ]] second}",
        "call(first, --[[ argument ]] function() run() end)",
        "local function call(first, --[[ parameter ]] second) return first end",
        "if first then run()\n-- tail inside body\nend",
        "each(function(source) ---@async\n if source then\n ---@type string\n local text = source[1]\n use(text)\n end\nend)",
        "do\nlocal t = {\n              a = 1, -- x\n              b = 2,\n}\nend",
        "do\nlocal t = {\n              a = (1 --[[why]]),\n}\nend",
        "local x = 1; -- stalled\n-- luck: format off\nlocal  y  =  { --[[inner]] 2 }\n-- luck: format on\nlocal z = 3",
        "-- comment\n;(function()\n\tBoo = 42\nend)()",
        "-- on the separator\n;\n-- leading\nlocal y = 2",
        "if first --[[ header ]] then\n-- luck: format off\nlocal  y  =  { --[[inner]] 2 }\n-- luck: format on\nend",
        "if ready --[[ note ]] then\n\tlocal t = { a = 1, -- x\n\t\tb = 2 }\nend",
        "local t = --[[ why ]] { a = 1, -- x\n\tb = 2 }",
        "render --[[ why ]] (a, -- x\n\tb)",
        "do\n\tlocal t = { [[\nline1\nline2]], (1 --[[why]]) }\nend",
        "function f(a --[[ param ]]) end",
        "for i = 1, 10 --[[ limit ]] do run() end",
        "for k in pairs(t) --[[ iterator ]] do run() end",
        "repeat run() until first --[[ until ]]",
    ];
    for source in sources {
        for width in [1, 60, 80, 120] {
            let options = FormatOptions {
                line_width: width,
                ..FormatOptions::default()
            };
            let formatted = luck_formatter::format_and_verify(source, LuaVersion::Lua54, &options)
                .unwrap_or_else(|(_, diff)| panic!("{source}: {diff:?}"));
            assert_format_with(source, &formatted.output, LuaVersion::Lua54, &options);
        }
    }
}

#[test]
fn semicolon_before_a_trailing_comment_keeps_it_attached() {
    assert_format(
        "local x = 1; -- keep me\nlocal y = 2\n",
        "local x = 1 -- keep me\nlocal y = 2\n",
    );
    assert_format("return 1; -- keep me\n", "return 1 -- keep me\n");
}

#[test]
fn comment_on_a_dropped_semicolon_stays_before_the_statement() {
    assert_format(
        "-- comment\n;(function()\n\tBoo = 42\nend)()\n",
        "-- comment\n(function()\n\tBoo = 42\nend)()\n",
    );
}

#[test]
fn table_claims_its_own_comments() {
    // Per-field comments keep their lines and the table formats normally -
    // no verbatim fallback, so the ragged `=` spacing is fixed up.
    assert_format(
        "local config = {\n      name=\"x\",   -- the name\n   value=1,\n}\n",
        "local config = {\n\tname = \"x\", -- the name\n\tvalue = 1,\n}\n",
    );
    assert_format(
        "local t = {\n\t-- leading\n\ta = 1,\n\t-- before the brace\n}\n",
        "local t = {\n\t-- leading\n\ta = 1,\n\t-- before the brace\n}\n",
    );
    // A nested table claims its own comment; the hard breaks it needs expand
    // the outer table too, which still uses its ordinary layout.
    assert_format(
        "local t = { a = {x=1, -- inner\n}, b = 2 }\n",
        "local t = {\n\ta = {\n\t\tx = 1, -- inner\n\t},\n\tb = 2,\n}\n",
    );
}

#[test]
fn call_arguments_claim_their_own_comments() {
    assert_format(
        "render(a, b, -- why\n\tc)\n",
        "render(\n\ta,\n\tb, -- why\n\tc\n)\n",
    );
}

#[test]
fn header_comment_keeps_its_place_and_its_statement_formatted() {
    // The gap between a condition and its `then`/`do` belongs to no emitter.
    // Printing the comment where it was written keeps the body formatted;
    // stalling it would take the whole statement down the verbatim fallback.
    assert_format(
        "if a --[[c]] then run() end\n",
        "if a --[[c]] then\n\trun()\nend\n",
    );
    assert_format(
        "if ready --[[ note ]] then\n\tlocal t = { a = 1, -- x\n\t\tb = 2 }\nend\n",
        "if ready --[[ note ]] then\n\tlocal t = {\n\t\ta = 1, -- x\n\t\tb = 2,\n\t}\nend\n",
    );
    assert_format(
        "if a then x() elseif b --[[c]] then y() end\n",
        "if a then\n\tx()\nelseif b --[[c]] then\n\ty()\nend\n",
    );
    assert_format(
        "while a --[[c]] do run() end\n",
        "while a --[[c]] do\n\trun()\nend\n",
    );
    assert_format(
        "for i = 1, 10 --[[c]] do run() end\n",
        "for i = 1, 10 --[[c]] do\n\trun()\nend\n",
    );
    assert_format(
        "for k in pairs(t) --[[c]] do run() end\n",
        "for k in pairs(t) --[[c]] do\n\trun()\nend\n",
    );
    // A line comment would swallow the keyword that follows it, so the
    // statement still falls back to verbatim.
    assert_format(
        "if input -- why\nthen\n\trun()\nend\n",
        "if input -- why\nthen\n\trun()\nend\n",
    );
}

#[test]
fn a_list_claims_only_the_comments_written_inside_it() {
    // `--[[ why ]]` sits before the `{`/`(`. Pulling it in with the comments
    // that do belong to the list would carry it across the delimiter, so the
    // statement goes verbatim instead.
    assert_format(
        "local t = --[[ why ]] { a = 1, -- x\n\tb = 2 }\n",
        "local t = --[[ why ]] { a = 1, -- x\n\tb = 2 }\n",
    );
    assert_format(
        "render --[[ why ]] (a, -- x\n\tb)\n",
        "render --[[ why ]] (a, -- x\n\tb)\n",
    );
}

#[test]
fn an_empty_body_claims_only_the_comments_it_can_reach() {
    // The comment is inside the parameter list; moving it into the body would
    // carry it across the `)`.
    assert_format("function f(a --[[c]]) end\n", "function f(a --[[c]]) end\n");
    // Nothing but whitespace separates this one from the body, so it lands
    // inside, indented.
    assert_format(
        "function f() --[[c]] end\n",
        "function f()\n\t--[[c]]\nend\n",
    );
}

#[test]
fn verbatim_statement_keeps_long_string_contents() {
    // The newlines inside `[[...]]` are the string's own bytes: re-anchoring
    // the lines around them would indent the value.
    assert_format(
        "do\n\tlocal t = { [[\nline1\nline2]], (1 --[[why]]) }\nend\n",
        "do\n\tlocal t = { [[\nline1\nline2]], (1 --[[why]]) }\nend\n",
    );
}

#[test]
fn verbatim_statement_is_reindented_to_its_block() {
    // A comment inside a field, which no list emitter can place, forces the
    // whole statement verbatim; its continuation lines still move to the
    // depth the `do` block dictates.
    assert_format(
        "do\nlocal t = {\n              a = (1 --[[why]]),\n}\nend\n",
        "do\n\tlocal t = {\n\t              a = (1 --[[why]]),\n\t}\nend\n",
    );
}

#[test]
fn stalled_comment_does_not_duplicate_a_format_off_region() {
    assert_format(
        "local x = 1; -- stalled\n-- luck: format off\nlocal  y  =  { --[[inner]] 2 }\n-- luck: format on\nlocal z = 3\n",
        "local x = 1 -- stalled\n-- luck: format off\nlocal  y  =  { --[[inner]] 2 }\n-- luck: format on\nlocal z = 3\n",
    );
    // The stalled comment is unmovable here - `then` sits between it and the
    // statement - so the whole `if` goes verbatim rather than relocating it.
    assert_format(
        "if first --[[ header ]] then\n-- luck: format off\nlocal  y  =  { --[[inner]] 2 }\n-- luck: format on\nend\n",
        "if first --[[ header ]] then\n-- luck: format off\nlocal  y  =  { --[[inner]] 2 }\n-- luck: format on\nend\n",
    );
}

#[test]
fn stalled_comment_stays_above_the_statements_own_leading_comment() {
    // The `;` carries the first comment; the second leads `local y`. The run
    // is movable across the separator, so both stay above the statement.
    assert_format(
        "-- on the separator\n;\n-- leading\nlocal y = 2\n",
        "-- on the separator\n-- leading\nlocal y = 2\n",
    );
}

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
