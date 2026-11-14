use core::fmt::Write as _;
use core::ops::Range;

use luck_formatter::{CollapseSimpleStatement, FormatOptions, LineEndings};
use luck_token::LuaVersion;

use crate::common::{assert_format, assert_format_range, assert_format_with};

/// Each source, its layout at width 1, and the one widths 60, 80, and 120
/// share.
const INTERIOR_COMMENT_CASES: [(&str, &str, &str); 24] = [
    (
        "if (first --[[ condition ]] and second) then run() end",
        "if (first --[[ condition ]] and second) then run() end\n",
        "if (first --[[ condition ]] and second) then run() end\n",
    ),
    (
        "if first then run() elseif second --[[ branch ]] then stop() else finish() end",
        "\
if
\tfirst
then
\trun()
elseif
\tsecond --[[ branch ]]
then
\tstop()
else
\tfinish()
end
",
        "\
if first then
\trun()
elseif second --[[ branch ]] then
\tstop()
else
\tfinish()
end
",
    ),
    (
        "while first --[[ loop ]] do run() end",
        "\
while
\tfirst --[[ loop ]]
do
\trun()
end
",
        "\
while first --[[ loop ]] do
\trun()
end
",
    ),
    (
        "repeat run() until first --[[ condition ]] and second",
        "repeat run() until first --[[ condition ]] and second\n",
        "repeat run() until first --[[ condition ]] and second\n",
    ),
    (
        "local function call() if first --[[ nested ]] then run() end end",
        "\
local function call()
\tif
\t\tfirst --[[ nested ]]
\tthen
\t\trun()
\tend
end
",
        "\
local function call()
\tif first --[[ nested ]] then
\t\trun()
\tend
end
",
    ),
    (
        "local values = {first, --[[ item ]] second}",
        "\
local values = {
\tfirst, --[[ item ]]
\tsecond,
}
",
        "\
local values = {
\tfirst, --[[ item ]]
\tsecond,
}
",
    ),
    (
        "call(first, --[[ argument ]] function() run() end)",
        "\
call(
\tfirst, --[[ argument ]]
\tfunction()
\t\trun()
\tend
)
",
        "\
call(
\tfirst, --[[ argument ]]
\tfunction()
\t\trun()
\tend
)
",
    ),
    (
        "local function call(first, --[[ parameter ]] second) return first end",
        "local function call(first, --[[ parameter ]] second) return first end\n",
        "local function call(first, --[[ parameter ]] second) return first end\n",
    ),
    (
        "\
if first then run()
-- tail inside body
end",
        "\
if
\tfirst
then
\trun()
\t-- tail inside body
end
",
        "\
if first then
\trun()
\t-- tail inside body
end
",
    ),
    (
        "\
each(function(source) ---@async
 if source then
 ---@type string
 local text = source[1]
 use(text)
 end
end)",
        "\
each(function(
\tsource
)
\t---@async
\tif
\t\tsource
\tthen
\t\t---@type string
\t\tlocal text = source[1]
\t\tuse(
\t\t\ttext
\t\t)
\tend
end)
",
        "\
each(function(source)
\t---@async
\tif source then
\t\t---@type string
\t\tlocal text = source[1]
\t\tuse(text)
\tend
end)
",
    ),
    (
        "\
do
local t = {
              a = 1, -- x
              b = 2,
}
end",
        "\
do
\tlocal t = {
\t\ta = 1, -- x
\t\tb = 2,
\t}
end
",
        "\
do
\tlocal t = {
\t\ta = 1, -- x
\t\tb = 2,
\t}
end
",
    ),
    (
        "\
do
local t = {
              a = (1 --[[why]]),
}
end",
        "\
do
\tlocal t = {
\t              a = (1 --[[why]]),
\t}
end
",
        "\
do
\tlocal t = {
\t              a = (1 --[[why]]),
\t}
end
",
    ),
    (
        "\
local x = 1; -- stalled
-- luck: format off
local  y  =  { --[[inner]] 2 }
-- luck: format on
local z = 3",
        "\
local x = 1 -- stalled
-- luck: format off
local  y  =  { --[[inner]] 2 }
-- luck: format on
local z = 3
",
        "\
local x = 1 -- stalled
-- luck: format off
local  y  =  { --[[inner]] 2 }
-- luck: format on
local z = 3
",
    ),
    (
        "\
-- comment
;(function()
\tBoo = 42
end)()",
        "\
-- comment
(function()
\tBoo = 42
end)()
",
        "\
-- comment
(function()
\tBoo = 42
end)()
",
    ),
    (
        "\
-- on the separator
;
-- leading
local y = 2",
        "\
-- on the separator
-- leading
local y = 2
",
        "\
-- on the separator
-- leading
local y = 2
",
    ),
    (
        "\
if first --[[ header ]] then
-- luck: format off
local  y  =  { --[[inner]] 2 }
-- luck: format on
end",
        "\
if
\tfirst --[[ header ]]
then
-- luck: format off
local  y  =  { --[[inner]] 2 }
-- luck: format on
end
",
        "\
if first --[[ header ]] then
-- luck: format off
local  y  =  { --[[inner]] 2 }
-- luck: format on
end
",
    ),
    (
        "\
if ready --[[ note ]] then
\tlocal t = { a = 1, -- x
\t\tb = 2 }
end",
        "\
if
\tready --[[ note ]]
then
\tlocal t = {
\t\ta = 1, -- x
\t\tb = 2,
\t}
end
",
        "\
if ready --[[ note ]] then
\tlocal t = {
\t\ta = 1, -- x
\t\tb = 2,
\t}
end
",
    ),
    (
        "\
local t = --[[ why ]] { a = 1, -- x
\tb = 2 }",
        "\
local t = --[[ why ]] { a = 1, -- x
\tb = 2 }
",
        "\
local t = --[[ why ]] { a = 1, -- x
\tb = 2 }
",
    ),
    (
        "\
render --[[ why ]] (a, -- x
\tb)",
        "\
render --[[ why ]] (a, -- x
\tb)
",
        "\
render --[[ why ]] (a, -- x
\tb)
",
    ),
    (
        "\
do
\tlocal t = { [[
line1
line2]], (1 --[[why]]) }
end",
        "\
do
\tlocal t = { [[
line1
line2]], (1 --[[why]]) }
end
",
        "\
do
\tlocal t = { [[
line1
line2]], (1 --[[why]]) }
end
",
    ),
    (
        "function f(a --[[ param ]]) end",
        "function f(a --[[ param ]]) end\n",
        "function f(a --[[ param ]]) end\n",
    ),
    (
        "for i = 1, 10 --[[ limit ]] do run() end",
        "\
for i = 1, 10 --[[ limit ]] do
\trun()
end
",
        "\
for i = 1, 10 --[[ limit ]] do
\trun()
end
",
    ),
    (
        "for k in pairs(t) --[[ iterator ]] do run() end",
        "\
for k in pairs(
\tt
) --[[ iterator ]] do
\trun()
end
",
        "\
for k in pairs(t) --[[ iterator ]] do
\trun()
end
",
    ),
    (
        "repeat run() until first --[[ until ]]",
        "\
repeat
\trun()
until first --[[ until ]]
",
        "\
repeat
\trun()
until first --[[ until ]]
",
    ),
];

#[test]
fn interior_comments_never_cross_stmt_boundaries() {
    for (source, narrowest, wider) in INTERIOR_COMMENT_CASES {
        for (line_width, expected) in [(1, narrowest), (60, wider), (80, wider), (120, wider)] {
            let options = FormatOptions { line_width, ..FormatOptions::default() };
            assert_format_with(source, expected, LuaVersion::Lua54, &options);
        }
    }
}

#[test]
fn semicolon_before_a_trailing_comment_keeps_it_attached() {
    assert_format(
        "\
local x = 1; -- keep me
local y = 2
",
        "\
local x = 1 -- keep me
local y = 2
",
    );
    assert_format("return 1; -- keep me\n", "return 1 -- keep me\n");
}

#[test]
fn comment_on_a_dropped_semicolon_stays_before_the_stmt() {
    assert_format(
        "\
-- comment
;(function()
\tBoo = 42
end)()
",
        "\
-- comment
(function()
\tBoo = 42
end)()
",
    );
}

#[test]
fn table_claims_its_own_comments() {
    // Per-field comments keep their lines and the table formats normally -
    // no verbatim fallback, so the ragged `=` spacing is fixed up.
    assert_format(
        "\
local config = {
      name=\"x\",   -- the name
   value=1,
}
",
        "\
local config = {
\tname = \"x\", -- the name
\tvalue = 1,
}
",
    );
    assert_format(
        "\
local t = {
\t-- leading
\ta = 1,
\t-- before the brace
}
",
        "\
local t = {
\t-- leading
\ta = 1,
\t-- before the brace
}
",
    );
    // A nested table claims its own comment; the hard breaks it needs expand
    // the outer table too, which still uses its ordinary layout.
    assert_format(
        "\
local t = { a = {x=1, -- inner
}, b = 2 }
",
        "\
local t = {
\ta = {
\t\tx = 1, -- inner
\t},
\tb = 2,
}
",
    );
}

#[test]
fn call_args_claim_their_own_comments() {
    assert_format(
        "\
render(a, b, -- why
\tc)
",
        "\
render(
\ta,
\tb, -- why
\tc
)
",
    );
}

#[test]
fn header_comment_keeps_its_place_and_its_stmt_formatted() {
    // The gap between a condition and its `then`/`do` belongs to no emitter.
    // Printing the comment where it was written keeps the body formatted;
    // stalling it would take the whole statement down the verbatim fallback.
    assert_format(
        "if a --[[c]] then run() end\n",
        "\
if a --[[c]] then
\trun()
end
",
    );
    assert_format(
        "\
if ready --[[ note ]] then
\tlocal t = { a = 1, -- x
\t\tb = 2 }
end
",
        "\
if ready --[[ note ]] then
\tlocal t = {
\t\ta = 1, -- x
\t\tb = 2,
\t}
end
",
    );
    assert_format(
        "if a then x() elseif b --[[c]] then y() end\n",
        "\
if a then
\tx()
elseif b --[[c]] then
\ty()
end
",
    );
    assert_format(
        "while a --[[c]] do run() end\n",
        "\
while a --[[c]] do
\trun()
end
",
    );
    assert_format(
        "for i = 1, 10 --[[c]] do run() end\n",
        "\
for i = 1, 10 --[[c]] do
\trun()
end
",
    );
    assert_format(
        "for k in pairs(t) --[[c]] do run() end\n",
        "\
for k in pairs(t) --[[c]] do
\trun()
end
",
    );
    // A line comment would swallow the keyword that follows it, so the
    // statement still falls back to verbatim.
    assert_format(
        "\
if input -- why
then
\trun()
end
",
        "\
if input -- why
then
\trun()
end
",
    );
}

#[test]
fn a_list_claims_only_the_comments_written_inside_it() {
    // `--[[ why ]]` sits before the `{`/`(`. Pulling it in with the comments
    // that do belong to the list would carry it across the delimiter, so the
    // statement goes verbatim instead.
    assert_format(
        "\
local t = --[[ why ]] { a = 1, -- x
\tb = 2 }
",
        "\
local t = --[[ why ]] { a = 1, -- x
\tb = 2 }
",
    );
    assert_format(
        "\
render --[[ why ]] (a, -- x
\tb)
",
        "\
render --[[ why ]] (a, -- x
\tb)
",
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
        "\
function f()
\t--[[c]]
end
",
    );
}

#[test]
fn verbatim_stmt_keeps_long_string_contents() {
    // The newlines inside `[[...]]` are the string's own bytes: re-anchoring
    // the lines around them would indent the value.
    assert_format(
        "\
do
\tlocal t = { [[
line1
line2]], (1 --[[why]]) }
end
",
        "\
do
\tlocal t = { [[
line1
line2]], (1 --[[why]]) }
end
",
    );
}

#[test]
fn verbatim_stmt_is_reindented_to_its_block() {
    // A comment inside a field, which no list emitter can place, forces the
    // whole statement verbatim; its continuation lines still move to the
    // depth the `do` block dictates.
    assert_format(
        "\
do
local t = {
              a = (1 --[[why]]),
}
end
",
        "\
do
\tlocal t = {
\t              a = (1 --[[why]]),
\t}
end
",
    );
}

#[test]
fn stalled_comment_does_not_duplicate_a_format_off_region() {
    assert_format(
        "\
local x = 1; -- stalled
-- luck: format off
local  y  =  { --[[inner]] 2 }
-- luck: format on
local z = 3
",
        "\
local x = 1 -- stalled
-- luck: format off
local  y  =  { --[[inner]] 2 }
-- luck: format on
local z = 3
",
    );
    // The region is at statement level inside the `if`, so it goes out as
    // written and the header still formats.
    assert_format(
        "\
if first --[[ header ]] then
-- luck: format off
local  y  =  { --[[inner]] 2 }
-- luck: format on
end
",
        "\
if first --[[ header ]] then
-- luck: format off
local  y  =  { --[[inner]] 2 }
-- luck: format on
end
",
    );
}

#[test]
fn stalled_comment_stays_above_the_stmts_own_leading_comment() {
    // The `;` carries the first comment; the second leads `local y`. The run
    // is movable across the separator, so both stay above the statement.
    assert_format(
        "\
-- on the separator
;
-- leading
local y = 2
",
        "\
-- on the separator
-- leading
local y = 2
",
    );
}

#[test]
fn leading_comment() {
    assert_format(
        "\
-- comment
local x = 1
",
        "\
-- comment
local x = 1
",
    );
}

#[test]
fn trailing_comment() {
    assert_format("local x = 1 -- comment\n", "local x = 1 -- comment\n");
}

#[test]
fn comment_after_only_empty_stmts() {
    // A block holding only dropped `;` statements has nothing to print, so
    // the comment flush must not open a line before it.
    assert_format(";-- a\n", "-- a\n");
    assert_format(
        "\
;
-- a
-- b
",
        "\
-- a
-- b
",
    );
}

#[test]
fn comment_between_stmts() {
    assert_format(
        "\
local x = 1
-- between
local y = 2
",
        "\
local x = 1
-- between
local y = 2
",
    );
}

#[test]
fn block_comment_verbatim() {
    assert_format(
        "\
--[[ block ]]
local x = 1
",
        "\
--[[ block ]]
local x = 1
",
    );
}

#[test]
fn shebang_preserved() {
    assert_format(
        "\
#!/usr/bin/env lua
local x = 1
",
        "\
#!/usr/bin/env lua
local x = 1
",
    );
}

#[test]
fn shebang_keeps_a_lone_carriage_return_as_text() {
    assert_format("#!x\ry = {\nlocal x = 1\n", "#!x\ry = {\nlocal x = 1\n");
}

#[test]
fn shebang_drops_trailing_carriage_returns() {
    assert_format("#!x\r\r\nlocal x = 1\n", "#!x\nlocal x = 1\n");
    assert_format("#!x\r\r", "#!x\n");
}

#[test]
fn blank_line_preserved() {
    assert_format(
        "\
local x = 1

local y = 2
",
        "\
local x = 1

local y = 2
",
    );
}

#[test]
fn multiple_blank_lines_collapsed() {
    assert_format(
        "\
local x = 1



local y = 2
",
        "\
local x = 1

local y = 2
",
    );
}

#[test]
fn trailing_newline_added() {
    assert_format("local x = 1", "local x = 1\n");
}

#[test]
fn comment_at_eof_gains_a_trailing_newline() {
    assert_format("local x = 1 -- end", "local x = 1 -- end\n");
}

#[test]
fn comment_inside_empty_function() {
    assert_format(
        "\
function foo()
-- nothing
end
",
        "\
function foo()
\t-- nothing
end
",
    );
}

#[test]
fn format_off_region() {
    assert_format(
        "\
local   x  =  1
-- luck: format off
local   y  =  2
-- luck: format on
local   z  =  3
",
        "\
local x = 1
-- luck: format off
local   y  =  2
-- luck: format on
local z = 3
",
    );
}

#[test]
fn format_off_unclosed_extends_to_eof() {
    assert_format(
        "\
local   x  =  1
-- luck: format off
local   y  =  2
local   z  =  3
",
        "\
local x = 1
-- luck: format off
local   y  =  2
local   z  =  3
",
    );
}

#[test]
fn format_ignore_single_stmt() {
    assert_format(
        "\
-- luck: ignore
local   x  =  1
local   y  =  2
",
        "\
-- luck: ignore
local   x  =  1
local y = 2
",
    );
}

#[test]
fn windows_line_endings() {
    assert_format_with(
        "if x then\ny=1\nend\n",
        "if x then\r\n\ty = 1\r\nend\r\n",
        LuaVersion::Lua54,
        &windows_options(),
    );
}

#[test]
fn format_range_only_formats_selection() {
    let input = "\
local   x  =  1
local   y  =  2
local   z  =  3
";
    assert_format_range(
        input,
        range_of(input, "local   y  =  2"),
        "\
local   x  =  1
local y = 2
local   z  =  3
",
    );
}

#[test]
fn fill_mode_packs_positional_table_entries() {
    let options = FormatOptions { line_width: 20, ..FormatOptions::default() };
    // Fill mode packs as many positional values per line as the width allows.
    // The tab counts for four columns, so the wrap lands after `5,`.
    assert_format_with(
        "local t = {1, 2, 3, 4, 5, 6, 7, 8}\n",
        "\
local t = {
\t1, 2, 3, 4, 5,
\t6, 7, 8,
}
",
        LuaVersion::Lua54,
        &options,
    );
}

#[test]
fn comment_after_a_multi_line_block_comment_formats_stably() {
    assert_format(
        "\
local x --[[
]] -- c
local y
",
        "\
local x --[[
]]
-- c
local y
",
    );
}

#[test]
fn line_break_inside_a_trailing_block_comment_is_not_a_blank_line() {
    assert_format(
        "\
local x --[[
]]
local y
",
        "\
local x --[[
]]
local y
",
    );
    assert_format(
        "\
local x --[[
]]

local y
",
        "\
local x --[[
]]

local y
",
    );
}

fn windows_options() -> FormatOptions {
    FormatOptions { line_endings: LineEndings::Windows, ..FormatOptions::default() }
}

#[test]
fn windows_line_endings_add_one_cr_to_verbatim_text_across_passes() {
    let cases = [
        (
            "local s = [[\nbody\nmore\n]]\nprint(#s)\n",
            "local s = [[\r\nbody\r\nmore\r\n]]\r\nprint(#s)\r\n",
        ),
        ("--[[\nblock\n]]\nlocal x = 1\n", "--[[\r\nblock\r\n]]\r\nlocal x = 1\r\n"),
        (
            "-- luck: format off\nlocal   x  =  [[\na]]\n-- luck: format on\n",
            "-- luck: format off\r\nlocal   x  =  [[\r\na]]\r\n-- luck: format on\r\n",
        ),
    ];
    for (input, expected) in cases {
        assert_format_with(input, expected, LuaVersion::Lua54, &windows_options());
    }
}

#[test]
fn windows_line_endings_keep_crlf_input_as_one_line_ending() {
    assert_format_with(
        "local s = [[\r\nbody\r\n]]\r\n--[[\r\nblock\r\n]]\r\n",
        "local s = [[\r\nbody\r\n]]\r\n--[[\r\nblock\r\n]]\r\n",
        LuaVersion::Lua54,
        &windows_options(),
    );
}

#[test]
fn windows_line_endings_keep_other_verbatim_line_breaks() {
    // Each of these is already a line break in a long string, so adding a CR
    // would change the value.
    let options = windows_options();
    for line_break in ["\n\r", "\r\r\n", "\r"] {
        let source = format!("local s = [[a{line_break}b]]\n");
        let expected = format!("local s = [[a{line_break}b]]\r\n");
        assert_format_with(&source, &expected, LuaVersion::Lua54, &options);
    }
}

fn conditional_collapse_options() -> FormatOptions {
    FormatOptions {
        collapse_simple_statement: CollapseSimpleStatement::ConditionalOnly,
        ..FormatOptions::default()
    }
}

#[test]
fn collapse_leaves_a_body_with_a_comment_expanded() {
    let options = conditional_collapse_options();
    for (input, expected) in [
        (
            "\
if a then
  i = 7 -- c
end
",
            "\
if a then
\ti = 7 -- c
end
",
        ),
        (
            "\
if a then
  return 1 -- c
end
",
            "\
if a then
\treturn 1 -- c
end
",
        ),
        (
            "\
if a then
  -- c
  i = 7
end
",
            "\
if a then
\t-- c
\ti = 7
end
",
        ),
        (
            "if a then i = 7 --[[c]] end\n",
            "\
if a then
\ti = 7 --[[c]]
end
",
        ),
        (
            "if a then --[[c]] i = 7 end\n",
            "\
if a then
\t--[[c]]
\ti = 7
end
",
        ),
        (
            "\
function s()
if a then
  repeat -- c
  until b
end
end
",
            "\
function s()
\tif a then
\t\trepeat
\t\t\t-- c
\t\tuntil b
\tend
end
",
        ),
        (
            "\
function s()
if a then
  -- luck: ignore
  x   =   1
end
end
",
            "\
function s()
\tif a then
\t\t-- luck: ignore
\t\tx   =   1
\tend
end
",
        ),
    ] {
        assert_format_with(input, expected, LuaVersion::Lua54, &options);
    }
}

#[test]
fn collapse_keeps_comments_that_stay_in_place() {
    let options = conditional_collapse_options();
    for (input, expected) in [
        (
            "\
if a then
  i = 7
end
",
            "if a then i = 7 end\n",
        ),
        ("if a then i = --[[c]] 7 end\n", "if a then i = --[[c]] 7 end\n"),
        ("if a --[[c]] then i = 7 end\n", "if a --[[c]] then i = 7 end\n"),
        (
            "\
if a then
  i = 7
end -- c
",
            "if a then i = 7 end -- c\n",
        ),
        (
            "\
-- c
if a then
  i = 7
end
",
            "\
-- c
if a then i = 7 end
",
        ),
    ] {
        assert_format_with(input, expected, LuaVersion::Lua54, &options);
    }
}

#[test]
fn directives_must_end_at_a_word_boundary() {
    for (input, expected) in [
        // `format on` must not close the region.
        (
            "\
-- luck: format off
local   x  =  1
-- luck: format one-liner
local   y  =  2
",
            "\
-- luck: format off
local   x  =  1
-- luck: format one-liner
local   y  =  2
",
        ),
        // `format off` must not open one.
        (
            "\
-- luck: format offset
local   x  =  1
",
            "\
-- luck: format offset
local x = 1
",
        ),
        // `ignore` must not keep the next statement.
        (
            "\
-- luck: ignored
local   x  =  1
",
            "\
-- luck: ignored
local x = 1
",
        ),
        (
            "\
-- luck: format ignored
local   x  =  1
",
            "\
-- luck: format ignored
local x = 1
",
        ),
    ] {
        assert_format(input, expected);
    }
}

#[test]
fn directives_may_be_followed_by_other_words() {
    for (input, expected) in [
        (
            "\
-- luck: format off because generated
local   x  =  1
",
            "\
-- luck: format off because generated
local   x  =  1
",
        ),
        (
            "\
-- luck: ignore this one
local   x  =  1
",
            "\
-- luck: ignore this one
local   x  =  1
",
        ),
    ] {
        assert_format(input, expected);
    }
}

fn range_of(input: &str, needle: &str) -> Range<usize> {
    let start = input.find(needle).unwrap_or(input.len());
    start..start.saturating_add(needle.len())
}

#[test]
fn comment_after_the_last_stmt_keeps_the_function_formatted() {
    // The comment has a safe place in the block, so the function needn't go
    // verbatim.
    assert_format(
        "\
function f()
  x   =   1
  -- tail
end
",
        "\
function f()
\tx = 1
\t-- tail
end
",
    );
}

#[test]
fn comment_only_do_block_formats_like_an_empty_function_body() {
    assert_format(
        "\
do
      -- only
end
",
        "\
do
\t-- only
end
",
    );
}

#[test]
fn comment_before_else_keeps_the_if_formatted() {
    assert_format(
        "\
if a then
  x   =   1
  -- before else
else
  y   =   2
end
",
        "\
if a then
\tx = 1
\t-- before else
else
\ty = 2
end
",
    );
}

#[test]
fn blank_line_after_a_leading_comment_is_preserved() {
    let input = "\
-- license header

local x = 1
";
    assert_format(input, input);
}

#[test]
fn blank_line_before_a_final_comment_is_preserved() {
    let input = "\
local x = 1

-- end of file
";
    assert_format(input, input);
}

#[test]
fn second_trailing_comment_stays_on_its_stmt_line() {
    let input = "\
x = 1 --[[a]] -- b
y = 2
";
    assert_format(input, input);
}

#[test]
fn format_off_keeps_trailing_comment_alignment() {
    let input = "\
-- luck: format off
x   = 1    -- one
yyy = 22   -- two
-- luck: format on
";
    assert_format(input, input);
}

#[test]
fn format_off_keeps_blank_line_runs() {
    let input = "\
-- luck: format off
local a = 1


local b = 2
-- luck: format on
";
    assert_format(input, input);
}

#[test]
fn format_off_keeps_stmts_sharing_a_line() {
    let input = "\
-- luck: format off
local a = 1; local b = 2
-- luck: format on
";
    assert_format(input, input);
}

#[test]
fn format_off_inside_a_table_keeps_the_region() {
    let input = "\
local t = {
\t-- luck: format off
\t1, 0,
\t0, 1,
\t-- luck: format on
}
";
    assert_format(input, input);
}

#[test]
fn ignore_directive_survives_the_semicolon_guard() {
    // The guard goes after the directive, which still keeps `(g   )()` as
    // written.
    assert_format(
        "\
f();
-- luck: ignore
(g   )()
",
        "\
f()
-- luck: ignore
;(g   )()
",
    );
}

#[test]
fn collapse_stays_idempotent_with_a_comment_in_a_list() {
    // An argument list that claims a comment breaks across lines, so the
    // `if` can't collapse around it.
    assert_format_with(
        "if a then b(1, --[[c]] 2) end\n",
        "\
if a then
\tb(
\t\t1, --[[c]]
\t\t2
\t)
end
",
        LuaVersion::Lua54,
        &conditional_collapse_options(),
    );
}

#[test]
fn collapse_leaves_a_multi_line_body_expanded() {
    assert_format_with(
        "\
if a then
  do x() end
end
",
        "\
if a then
\tdo
\t\tx()
\tend
end
",
        LuaVersion::Lua54,
        &conditional_collapse_options(),
    );
}

#[test]
fn collapse_leaves_a_too_long_if_expanded() {
    let options = FormatOptions { line_width: 30, ..conditional_collapse_options() };
    assert_format_with(
        "if condition_name then call_function_name() end\n",
        "\
if condition_name then
\tcall_function_name()
end
",
        LuaVersion::Lua54,
        &options,
    );
}

#[test]
fn format_range_keeps_blank_line_runs_outside_the_selection() {
    let input = "\
local a = 1


local   b   =   2
";
    assert_format_range(
        input,
        range_of(input, "local   b   =   2"),
        "\
local a = 1


local b = 2
",
    );
}

#[test]
fn format_range_keeps_indentation_outside_the_selection() {
    let input = "\
function f()
  local a=1
  local   b=2
end
";
    assert_format_range(
        input,
        range_of(input, "local   b=2"),
        "\
function f()
  local a=1
  local b = 2
end
",
    );
}

#[test]
fn format_range_keeps_crlf_outside_the_selection() {
    let input = "local   a   =   1\r\nlocal   b   =   2\r\n";
    assert_format_range(
        input,
        range_of(input, "local   a   =   1"),
        "local a = 1\r\nlocal   b   =   2\r\n",
    );
}

#[test]
fn ignored_multi_line_stmt_keeps_its_lines_aligned() {
    assert_format(
        "\
do
    -- luck: ignore
    local   t = {
    }
end
",
        "\
do
\t-- luck: ignore
\tlocal   t = {
\t}
end
",
    );
}

#[test]
fn unix_line_endings_normalize_a_block_comment() {
    assert_format("--[[a\r\nb]]\r\nlocal x\r\n", "--[[a\nb]]\nlocal x\n");
}

#[test]
fn a_long_commented_argument_list_formats_without_rescanning_its_comments() {
    // Each argument asks whether a comment falls inside it. Answered by
    // walking the pending comments from the first, this takes minutes.
    let count = 450_000u32;
    let arguments: Vec<String> =
        (0..count).map(|index| format!("--[[c{index}]] {index}")).collect();
    let last = count.saturating_sub(1);
    let mut expected = String::from(
        "\
f(
\t--[[c0]]
",
    );
    for index in 0..last {
        writeln!(expected, "\t{index}, --[[c{}]]", index.saturating_add(1)).unwrap();
    }
    writeln!(
        expected,
        "\
\t{last}
)"
    )
    .unwrap();
    assert_format(&format!("f({})\n", arguments.join(", ")), &expected);
}
