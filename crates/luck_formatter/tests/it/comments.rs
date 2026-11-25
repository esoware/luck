use std::fmt::Write as _;
use std::ops::Range;

use luck_formatter::{CollapseSimpleStatement, FormatOptions, LineEndings};

use crate::common::{assert_format, assert_format_range, assert_format_with};

/// A Latin-1 source, which Luau reads as bytes, keeps every byte of its
/// comments and strings.
#[test]
fn comments_and_strings_that_are_not_utf8_keep_their_bytes() {
    let source = b"--[[ caf\xE9\r\n\xE8 ]]\nlocal s = { 'na\xEFve', \"\xFF\" } -- d\xE9j\xE0\n";
    let expected = b"--[[ caf\xE9\n\xE8 ]]\nlocal s = { \"na\xEFve\", \"\xFF\" } -- d\xE9j\xE0\n";
    assert_format_with(source, expected, &FormatOptions::default());
}

/// Each source, its layout at width 1, and the one widths 60, 80, and 120
/// share.
const INTERIOR_COMMENT_CASES: [(&str, &str, &str); 22] = [
    (
        "if (first --[[ condition ]] and second) then run() end",
        "\
if
\tfirst --[[ condition ]]
\tand second
then
\trun()
end
",
        "\
if first --[[ condition ]] and second then
\trun()
end
",
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
        "\
repeat
\trun()
until first --[[ condition ]]
\tand second
",
        "\
repeat
\trun()
until first --[[ condition ]] and second
",
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
\tfirst,
\t--[[ item ]] second,
}
",
        "local values = { first, --[[ item ]] second }\n",
    ),
    (
        "call(first, --[[ argument ]] function() run() end)",
        "\
call(
\tfirst,
\t--[[ argument ]] function()
\t\trun()
\tend
)
",
        "\
call(first, --[[ argument ]] function()
\trun()
end)
",
    ),
    (
        "local function call(first, --[[ parameter ]] second) return first end",
        "\
local function call(
\tfirst,
\t--[[ parameter ]] second
)
\treturn first
end
",
        "\
local function call(first, --[[ parameter ]] second)
\treturn first
end
",
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
\t\ta = (1 --[[why]]),
\t}
end
",
        "\
do
\tlocal t = {
\t\ta = (1 --[[why]]),
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
local t = --[[ why ]] {
\ta = 1, -- x
\tb = 2,
}
",
        "\
local t = --[[ why ]] {
\ta = 1, -- x
\tb = 2,
}
",
    ),
    (
        "\
render --[[ why ]] (a, -- x
\tb)",
        "\
render(
\t--[[ why ]] a, -- x
\tb
)
",
        "\
render(
\t--[[ why ]] a, -- x
\tb
)
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
\tlocal t = {
\t\t[[
line1
line2]],
\t\t(1 --[[why]]),
\t}
end
",
        "\
do
\tlocal t = {
\t\t[[
line1
line2]],
\t\t(1 --[[why]]),
\t}
end
",
    ),
    (
        "function f(a --[[ param ]]) end",
        "\
function f(
\ta --[[ param ]]
) end
",
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
fn every_comment_formats_where_it_was_written() {
    for (source, narrowest, wider) in INTERIOR_COMMENT_CASES {
        for (line_width, expected) in [(1, narrowest), (60, wider), (80, wider), (120, wider)] {
            let options = FormatOptions { line_width, ..FormatOptions::default() };
            assert_format_with(source, expected, &options);
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
fn table_fields_keep_their_comments() {
    // The ragged `=` spacing is fixed up around them.
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
    // A comment that ends its line in a nested table breaks the tables
    // around it too.
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
fn call_args_keep_their_comments() {
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
    // A comment between a condition and its `then` or `do` trails the
    // condition.
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
    // A line comment ends its line, so the header breaks around it, and the
    // `do` of a `for` loop, which has no group, goes on the next line.
    assert_format(
        "\
if input -- why
then
\trun()
end
",
        "\
if
\tinput -- why
then
\trun()
end
",
    );
    assert_format(
        "for i = 1, 10 -- why\ndo run() end\n",
        "\
for i = 1, 10 -- why
do
\trun()
end
",
    );
}

#[test]
fn a_comment_before_a_list_leads_it() {
    assert_format(
        "\
local t = --[[ why ]] { a = 1, -- x
\tb = 2 }
",
        "\
local t = --[[ why ]] {
\ta = 1, -- x
\tb = 2,
}
",
    );
    // Before call arguments it goes inside the parentheses, since a line
    // break before a call's `(` doesn't parse.
    assert_format(
        "\
render --[[ why ]] (a, -- x
\tb)
",
        "\
render(
\t--[[ why ]] a, -- x
\tb
)
",
    );
    assert_format_with(
        "f --[[\n]] \"s\"\n",
        "\
f(
\t--[[
]] \"s\"
)
",
        &FormatOptions::default(),
    );
    assert_format("local function f -- why\n() end\n", "local function f(\n\t-- why\n) end\n");
}

#[test]
fn an_empty_body_takes_only_the_comments_after_its_parameters() {
    // This one trails the last parameter, inside the parentheses.
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
fn a_comment_inside_a_field_value_stays_there() {
    assert_format(
        "\
do
local t = {
              a = (1 --[[why]]),
              [k --[[key]]] = f(x --[[arg]]).y,
}
end
",
        "\
do
\tlocal t = {
\t\ta = (1 --[[why]]),
\t\t[k --[[key]]] = f(x --[[arg]]).y,
\t}
end
",
    );
}

#[test]
fn code_after_a_comment_ending_its_line_stays_on_the_next_line() {
    // Moved up past the `]`, the comment would trail the `1` on the next
    // format.
    let input = "\
t[a -- x
] = 1
";
    assert_format(input, input);
    // Two line comments can't share a line, so the parentheses around a
    // condition stay when a comment needs the line breaks they hold.
    assert_format(
        "if (a -- x\n) -- y\nthen end\n",
        "\
if
\t(a -- x
\t) -- y
then
end
",
    );
    // A `return`'s span takes its `;`, which the formatter drops.
    assert_format(
        "\
local function f()
return x -- a
; -- b
end
",
        "\
local function f()
\treturn x -- a
\t-- b
end
",
    );
}

#[test]
fn a_comment_ending_its_line_before_an_operand_goes_above_its_operator() {
    for input in ["x = a\n-- c\n+ b\n", "x = a + -- c\nb\n"] {
        assert_format(
            input,
            "\
x = a
\t-- c
\t+ b
",
        );
    }
    assert_format(
        "x = obj\n-- c\n:method()\n",
        "\
x = obj
-- c
:method()
",
    );
    assert_format("x = obj: -- c\nmethod()\n", "x = obj -- c\n:method()\n");
    assert_format(
        "x = obj:first():second()\n-- c\n:third()\n",
        "\
x = obj
\t:first()
\t:second()
\t-- c
\t:third()
",
    );
}

#[test]
fn a_block_comment_counts_toward_its_line_once_it_ends_it() {
    // The comment is the 31st column either way, whether `+` follows it or a
    // line break does, as on the second format.
    assert_format_with(
        "local n = size(t) --[[ total ]] + size(u) + size(v)\n",
        "\
local n = size(
\tt
) --[[ total ]]
\t+ size(u)
\t+ size(v)
",
        &FormatOptions { line_width: 30, ..FormatOptions::default() },
    );
}

#[test]
fn a_comment_ending_its_line_after_an_assignment_moves_its_value_down() {
    assert_format(
        "local x = -- c\n1\n",
        "\
local x =
\t-- c
\t1
",
    );
    assert_format(
        "local function f()\nreturn -- c\nx\nend\n",
        "\
local function f()
\treturn
\t\t-- c
\t\tx
end
",
    );
    assert_format(
        "t = { a = -- c\n1 }\n",
        "\
t = {
\ta =
\t\t-- c
\t\t1,
}
",
    );
}

#[test]
fn an_empty_list_keeps_its_comments_inside() {
    for source in ["f(--[[c]])\n", "local t = { --[[c]] }\n"] {
        assert_format(source, source);
    }
    assert_format(
        "function f(-- c\n) end\n",
        "\
function f(
\t-- c
) end
",
    );
}

#[test]
fn a_comment_leading_an_alternation_goes_after_the_line_break_it_opens_with() {
    let options = FormatOptions::default();
    assert_format_with("type T = --[[c]] A | B\n", "type T = --[[c]] A | B\n", &options);
    assert_format_with(
        "type T = -- c\nA | B\n",
        "\
type T =
\t-- c
\t| A
\t| B
",
        &options,
    );
    // An instantiation's comments go inside its doubled brackets.
    assert_format_with(
        "local x = convert --[[a]] < --[[b]] < integer > --[[e]] > (current)\n",
        "local x = convert --[[a]]<<--[[b]] integer --[[e]]>>(current)\n",
        &options,
    );
}

#[test]
fn a_comment_on_its_own_line_keeps_one_before_a_closer() {
    assert_format(
        "\
local t = { a, (b
-- in parens
), c[d
-- in brackets
] }
",
        "\
local t = {
\ta,
\t(b
\t-- in parens
\t),
\tc[d
\t-- in brackets
\t],
}
",
    );
    assert_format(
        "\
function f(a
-- after a parameter
) end
",
        "\
function f(
\ta
\t-- after a parameter
) end
",
    );
}

#[test]
fn a_comment_after_a_separator_does_not_duplicate_a_format_off_region() {
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
#!/usr/bin/env luau
local x = 1
",
        "\
#!/usr/bin/env luau
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
fn shebang_drops_a_carriage_return_behind_trailing_spaces() {
    assert_format("#!x\r \t\nlocal x = 1\n", "#!x\nlocal x = 1\n");
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
        &options,
    );
}

#[test]
fn comment_after_a_multi_line_block_comment_formats_stably() {
    let input = "\
local x --[[
]] -- c
local y
";
    assert_format(input, input);
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
        assert_format_with(input, expected, &windows_options());
    }
}

#[test]
fn windows_line_endings_keep_crlf_input_as_one_line_ending() {
    assert_format_with(
        "local s = [[\r\nbody\r\n]]\r\n--[[\r\nblock\r\n]]\r\n",
        "local s = [[\r\nbody\r\n]]\r\n--[[\r\nblock\r\n]]\r\n",
        &windows_options(),
    );
}

#[test]
fn windows_line_endings_keep_other_verbatim_line_breaks() {
    // The printer adds a CR only to an LF without one before it, so each of
    // these keeps its bytes.
    let options = windows_options();
    for line_break in ["\r\r\n", "\r"] {
        let source = format!("local s = [[a{line_break}b]]\n");
        let expected = format!("local s = [[a{line_break}b]]\r\n");
        assert_format_with(&source, &expected, &options);
    }
}

#[test]
fn windows_line_endings_add_a_cr_to_an_lf_before_a_cr() {
    // A long string reads a CRLF as one LF and keeps a lone CR, so its value
    // stays the same.
    assert_format_with("local s = [[a\n\rb]]\n", "local s = [[a\r\n\rb]]\r\n", &windows_options());
}

#[test]
fn windows_line_endings_make_the_lf_before_a_verbatim_cr_crlf() {
    // A CR breaks no line, so the region starts with one, after the line
    // break the printer writes.
    assert_format_with(
        "-- luck: format off\n\rlocal x=1\n-- luck: format on\nlocal y=2\n",
        "-- luck: format off\r\n\rlocal x=1\r\n-- luck: format on\r\nlocal y = 2\r\n",
        &windows_options(),
    );
}

#[test]
fn a_format_off_after_a_lone_cr_trails_the_statement_before_it() {
    // A lone CR is a space, so the region starts on the next line, as it does
    // after a trailing directive.
    let expected = "\
local x = 1 -- luck: format off
local   y   =   2
-- luck: format on
local z = 3
";
    for line_space in ["\r", " "] {
        let input = format!(
            "\
local x = 1{line_space}-- luck: format off
local   y   =   2
-- luck: format on
local   z   =   3
"
        );
        assert_format_with(&input, expected, &FormatOptions::default());
    }
}

fn conditional_collapse_options() -> FormatOptions {
    FormatOptions {
        collapse_simple_statement: CollapseSimpleStatement::ConditionalOnly,
        ..FormatOptions::default()
    }
}

#[test]
fn a_comment_beside_a_callback_keeps_it_hugged() {
    assert_format_with(
        "if a then g --[[c]] (function() return 1 end) end\n",
        "\
if a then
\tg(--[[c]] function()
\t\treturn 1
\tend)
end
",
        &conditional_collapse_options(),
    );
    assert_format(
        "\
local t = f(function() return 1 end, x --[[e]] .y)
local u = f(function() local z = q --[[k]] (r) return z end, 2)
",
        "\
local t = f(function()
\treturn 1
end, x --[[e]].y)
local u = f(function()
\tlocal z = q(--[[k]] r)
\treturn z
end, 2)
",
    );
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
        assert_format_with(input, expected, &options);
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
        ("if a then --[[c]] i = 7 end\n", "if a then --[[c]] i = 7 end\n"),
        ("if a then b(1, --[[c]] 2) end\n", "if a then b(1, --[[c]] 2) end\n"),
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
        assert_format_with(input, expected, &options);
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

#[test]
fn directives_take_the_linter_s_comment_forms() {
    for (input, expected) in [
        (
            "\
--[[ luck: format off ]]
local   x  =  1
--[==[ luck: format on ]==]
local   y  =  2
",
            "\
--[[ luck: format off ]]
local   x  =  1
--[==[ luck: format on ]==]
local y = 2
",
        ),
        (
            "\
--luck:ignore
local   x  =  1
",
            "\
--luck:ignore
local   x  =  1
",
        ),
    ] {
        assert_format(input, expected);
    }
}

fn range_of(input: &str, needle: &str) -> Range<usize> {
    let start = input.find(needle).unwrap();
    start..start + needle.len()
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
fn collapse_leaves_a_comment_ending_a_line_in_a_list_expanded() {
    assert_format_with(
        "if a then b(1, -- c\n2) end\n",
        "\
if a then
\tb(
\t\t1, -- c
\t\t2
\t)
end
",
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
    // Each argument looks for its leading and trailing comments. Answered by
    // counting every comment left before the call's end, this takes minutes.
    let count = 450_000;
    let arguments: Vec<String> =
        (0..count).map(|index| format!("--[[c{index}]] {index}")).collect();
    let last = count - 1;
    let mut expected = String::from("f(\n");
    for index in 0..last {
        writeln!(expected, "\t--[[c{index}]] {index},").unwrap();
    }
    writeln!(
        expected,
        "\
\t--[[c{last}]] {last}
)"
    )
    .unwrap();
    assert_format(&format!("f({})\n", arguments.join(", ")), &expected);
}

#[test]
fn a_long_table_closing_on_a_comment_formats_without_rescanning_its_fields() {
    // Each field looks for a comment trailing it, and the one before the
    // closer is past every later field. Answered by reading every token
    // between, rather than stopping at the next field, this takes minutes.
    let count = 300_000;
    let fields: Vec<String> = (0..count).map(|index| index.to_string()).collect();
    let mut expected = String::from("local t = {\n");
    for field in &fields {
        writeln!(expected, "\t{field},").unwrap();
    }
    expected.push_str("\t-- c\n}\n");
    assert_format(&format!("local t = {{{}\n-- c\n}}\n", fields.join(", ")), &expected);
}

#[test]
fn a_comment_after_a_line_comment_a_lone_cr_ends_starts_its_own_line() {
    // A line comment ends at a lone CR, which breaks no line.
    assert_format(
        "x = 1 -- a\r-- b\r",
        "\
x = 1 -- a
-- b
",
    );
    assert_format(
        "f(a, -- x\r--[[y]] b)\n",
        "\
f(
\ta, -- x
\t--[[y]] b
)
",
    );
}

#[test]
fn lone_crs_ending_a_line_go_as_spaces_do() {
    // A lone CR is a space, so only the last before an LF is half of a CRLF.
    assert_format(
        "x = -- c\r\r\n1\n",
        "\
x =
\t-- c
\t1
",
    );
    assert_format(
        "--[[a\r\r\nb]]\nlocal x\n",
        "\
--[[a
b]]
local x
",
    );
}
