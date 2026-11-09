use core::ops::Range;

use luck_formatter::{CollapseSimpleStatement, FormatOptions, LineEndings};
use luck_token::LuaVersion;

use crate::common::{assert_format, assert_format_with, format_once};

#[test]
fn interior_comments_never_cross_stmt_boundaries() {
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
            let options = FormatOptions { line_width: width, ..FormatOptions::default() };
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
fn comment_on_a_dropped_semicolon_stays_before_the_stmt() {
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
fn call_args_claim_their_own_comments() {
    assert_format("render(a, b, -- why\n\tc)\n", "render(\n\ta,\n\tb, -- why\n\tc\n)\n");
}

#[test]
fn header_comment_keeps_its_place_and_its_stmt_formatted() {
    // The gap between a condition and its `then`/`do` belongs to no emitter.
    // Printing the comment where it was written keeps the body formatted;
    // stalling it would take the whole statement down the verbatim fallback.
    assert_format("if a --[[c]] then run() end\n", "if a --[[c]] then\n\trun()\nend\n");
    assert_format(
        "if ready --[[ note ]] then\n\tlocal t = { a = 1, -- x\n\t\tb = 2 }\nend\n",
        "if ready --[[ note ]] then\n\tlocal t = {\n\t\ta = 1, -- x\n\t\tb = 2,\n\t}\nend\n",
    );
    assert_format(
        "if a then x() elseif b --[[c]] then y() end\n",
        "if a then\n\tx()\nelseif b --[[c]] then\n\ty()\nend\n",
    );
    assert_format("while a --[[c]] do run() end\n", "while a --[[c]] do\n\trun()\nend\n");
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
    assert_format("if input -- why\nthen\n\trun()\nend\n", "if input -- why\nthen\n\trun()\nend\n");
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
    assert_format("render --[[ why ]] (a, -- x\n\tb)\n", "render --[[ why ]] (a, -- x\n\tb)\n");
}

#[test]
fn an_empty_body_claims_only_the_comments_it_can_reach() {
    // The comment is inside the parameter list; moving it into the body would
    // carry it across the `)`.
    assert_format("function f(a --[[c]]) end\n", "function f(a --[[c]]) end\n");
    // Nothing but whitespace separates this one from the body, so it lands
    // inside, indented.
    assert_format("function f() --[[c]] end\n", "function f()\n\t--[[c]]\nend\n");
}

#[test]
fn verbatim_stmt_keeps_long_string_contents() {
    // The newlines inside `[[...]]` are the string's own bytes: re-anchoring
    // the lines around them would indent the value.
    assert_format(
        "do\n\tlocal t = { [[\nline1\nline2]], (1 --[[why]]) }\nend\n",
        "do\n\tlocal t = { [[\nline1\nline2]], (1 --[[why]]) }\nend\n",
    );
}

#[test]
fn verbatim_stmt_is_reindented_to_its_block() {
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
fn stalled_comment_stays_above_the_stmts_own_leading_comment() {
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
fn comment_after_only_empty_stmts() {
    // A block holding only dropped `;` statements has nothing to print, so
    // the comment flush must not open a line before it.
    assert_format(";-- a\n", "-- a\n");
    assert_format(";\n-- a\n-- b\n", "-- a\n-- b\n");
}

#[test]
fn comment_between_stmts() {
    assert_format(
        "local x = 1\n-- between\nlocal y = 2\n",
        "local x = 1\n-- between\nlocal y = 2\n",
    );
}

#[test]
fn block_comment_verbatim() {
    assert_format("--[[ block ]]\nlocal x = 1\n", "--[[ block ]]\nlocal x = 1\n");
}

#[test]
fn shebang_preserved() {
    assert_format("#!/usr/bin/env lua\nlocal x = 1\n", "#!/usr/bin/env lua\nlocal x = 1\n");
}

#[test]
fn blank_line_preserved() {
    assert_format("local x = 1\n\nlocal y = 2\n", "local x = 1\n\nlocal y = 2\n");
}

#[test]
fn multiple_blank_lines_collapsed() {
    assert_format("local x = 1\n\n\n\nlocal y = 2\n", "local x = 1\n\nlocal y = 2\n");
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
    assert_format("function foo()\n-- nothing\nend\n", "function foo()\n\t-- nothing\nend\n");
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
    assert!(result.errors.is_empty(), "parse errors: {:?}", result.errors);
    assert!(result.output.contains("local x = 1"), "x should be formatted: {}", result.output);
    assert!(result.output.contains("local   y  =  2"), "y should be preserved: {}", result.output);
    assert!(result.output.contains("local   z  =  3"), "z should be preserved: {}", result.output);
}

#[test]
fn format_ignore_single_stmt() {
    assert_format(
        "-- luck: ignore\nlocal   x  =  1\nlocal   y  =  2\n",
        "-- luck: ignore\nlocal   x  =  1\nlocal y = 2\n",
    );
}

#[test]
fn windows_line_endings() {
    let options = FormatOptions { line_endings: LineEndings::Windows, ..FormatOptions::default() };
    let result = luck_formatter::format("if x then\ny=1\nend\n", LuaVersion::Lua54, &options);
    assert_eq!(result.errors, Vec::<luck_token::SourceError>::new());
    assert_eq!(result.output, "if x then\r\n\ty = 1\r\nend\r\n");
}

#[test]
fn format_range_only_formats_selection() {
    let input = "local   x  =  1\nlocal   y  =  2\nlocal   z  =  3\n";
    // The range covers only the second statement.
    let second_start = input.find("local   y").unwrap();
    let second_end = input.find("\nlocal   z").unwrap();
    let result = luck_formatter::format_range(
        input,
        LuaVersion::Lua54,
        &FormatOptions::default(),
        second_start..second_end,
    );
    assert!(result.errors.is_empty(), "parse errors: {:?}", result.errors);
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
    let options = FormatOptions { line_width: 20, ..FormatOptions::default() };
    // Fill mode packs as many positional values per line as the width allows,
    // so the wrap lands after `6,`.
    assert_format_with(
        "local t = {1, 2, 3, 4, 5, 6, 7, 8}\n",
        "local t = {\n\t1, 2, 3, 4, 5, 6,\n\t7, 8,\n}\n",
        LuaVersion::Lua54,
        &options,
    );
}

#[test]
fn comment_after_a_multi_line_block_comment_formats_stably() {
    assert_format("local x --[[\n]] -- c\nlocal y\n", "local x --[[\n]]\n-- c\nlocal y\n");
}

#[test]
fn line_break_inside_a_trailing_block_comment_is_not_a_blank_line() {
    assert_format("local x --[[\n]]\nlocal y\n", "local x --[[\n]]\nlocal y\n");
    assert_format("local x --[[\n]]\n\nlocal y\n", "local x --[[\n]]\n\nlocal y\n");
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
        ("if a then\n  i = 7 -- c\nend\n", "if a then\n\ti = 7 -- c\nend\n"),
        ("if a then\n  return 1 -- c\nend\n", "if a then\n\treturn 1 -- c\nend\n"),
        ("if a then\n  -- c\n  i = 7\nend\n", "if a then\n\t-- c\n\ti = 7\nend\n"),
        ("if a then i = 7 --[[c]] end\n", "if a then\n\ti = 7 --[[c]]\nend\n"),
        ("if a then --[[c]] i = 7 end\n", "if a then\n\t--[[c]]\n\ti = 7\nend\n"),
        (
            "function s()\nif a then\n  repeat -- c\n  until b\nend\nend\n",
            "function s()\n\tif a then\n\t\trepeat -- c\n\t\tuntil b\n\tend\nend\n",
        ),
        (
            "function s()\nif a then\n  -- luck: ignore\n  x   =   1\nend\nend\n",
            "function s()\n\tif a then\n\t\t-- luck: ignore\n\t\tx   =   1\n\tend\nend\n",
        ),
    ] {
        assert_format_with(input, expected, LuaVersion::Lua54, &options);
    }
}

#[test]
fn collapse_keeps_comments_that_stay_in_place() {
    let options = conditional_collapse_options();
    for (input, expected) in [
        ("if a then\n  i = 7\nend\n", "if a then i = 7 end\n"),
        ("if a then i = --[[c]] 7 end\n", "if a then i = --[[c]] 7 end\n"),
        ("if a --[[c]] then i = 7 end\n", "if a --[[c]] then i = 7 end\n"),
        ("if a then\n  i = 7\nend -- c\n", "if a then i = 7 end -- c\n"),
        ("-- c\nif a then\n  i = 7\nend\n", "-- c\nif a then i = 7 end\n"),
    ] {
        assert_format_with(input, expected, LuaVersion::Lua54, &options);
    }
}

#[test]
fn directives_must_end_at_a_word_boundary() {
    for (input, expected) in [
        // `format on` must not close the region.
        (
            "-- luck: format off\nlocal   x  =  1\n-- luck: format one-liner\nlocal   y  =  2\n",
            "-- luck: format off\nlocal   x  =  1\n-- luck: format one-liner\nlocal   y  =  2\n",
        ),
        // `format off` must not open one.
        ("-- luck: format offset\nlocal   x  =  1\n", "-- luck: format offset\nlocal x = 1\n"),
        // `ignore` must not keep the next statement.
        ("-- luck: ignored\nlocal   x  =  1\n", "-- luck: ignored\nlocal x = 1\n"),
        ("-- luck: format ignored\nlocal   x  =  1\n", "-- luck: format ignored\nlocal x = 1\n"),
    ] {
        assert_format(input, expected);
    }
}

#[test]
fn directives_may_be_followed_by_other_words() {
    for (input, expected) in [
        (
            "-- luck: format off because generated\nlocal   x  =  1\n",
            "-- luck: format off because generated\nlocal   x  =  1\n",
        ),
        (
            "-- luck: ignore this one\nlocal   x  =  1\n",
            "-- luck: ignore this one\nlocal   x  =  1\n",
        ),
    ] {
        assert_format(input, expected);
    }
}

fn default_format(input: &str) -> String {
    format_once(input, LuaVersion::Lua54, &FormatOptions::default())
}

fn range_of(input: &str, needle: &str) -> Range<usize> {
    let start = input.find(needle).unwrap_or(input.len());
    start..start.saturating_add(needle.len())
}

#[test]
fn bug_comment_after_the_last_stmt_keeps_the_function_formatted() {
    assert_eq!(
        default_format("function f()\n  x   =   1\n  -- tail\nend\n"),
        "function f()\n\tx = 1\n\t-- tail\nend\n",
        "a comment after a block's last statement has a safe place in the block, so the function must still format"
    );
}

#[test]
fn bug_comment_only_do_block_formats_like_an_empty_function_body() {
    assert_eq!(
        default_format("do\n      -- only\nend\n"),
        "do\n\t-- only\nend\n",
        "a comment alone in a `do` block must be indented like one alone in a function body"
    );
}

#[test]
fn bug_comment_before_else_keeps_the_if_formatted() {
    assert_eq!(
        default_format("if a then\n  x   =   1\n  -- before else\nelse\n  y   =   2\nend\n"),
        "if a then\n\tx = 1\n\t-- before else\nelse\n\ty = 2\nend\n",
        "a comment closing a branch must not send the whole `if` to the verbatim fallback"
    );
}

#[test]
fn bug_blank_line_after_a_leading_comment_is_preserved() {
    assert_eq!(
        default_format("-- license header\n\nlocal x = 1\n"),
        "-- license header\n\nlocal x = 1\n",
        "the blank line detaching a comment from the next statement must survive"
    );
}

#[test]
fn bug_blank_line_before_a_final_comment_is_preserved() {
    assert_eq!(
        default_format("local x = 1\n\n-- end of file\n"),
        "local x = 1\n\n-- end of file\n",
        "the blank line before a comment at end of file must survive"
    );
}

#[test]
fn bug_second_trailing_comment_stays_on_its_stmt_line() {
    assert_eq!(
        default_format("x = 1 --[[a]] -- b\ny = 2\n"),
        "x = 1 --[[a]] -- b\ny = 2\n",
        "`-- b` trails `x = 1` and must not become a leading comment of `y = 2`"
    );
}

#[test]
fn bug_format_off_keeps_trailing_comment_alignment() {
    let input = "-- luck: format off\nx   = 1    -- one\nyyy = 22   -- two\n-- luck: format on\n";
    assert_eq!(default_format(input), input, "a format-off region must keep its comment columns");
}

#[test]
fn bug_format_off_keeps_blank_line_runs() {
    let input = "-- luck: format off\nlocal a = 1\n\n\nlocal b = 2\n-- luck: format on\n";
    assert_eq!(default_format(input), input, "a format-off region must keep its blank lines");
}

#[test]
fn bug_format_off_keeps_stmts_sharing_a_line() {
    let input = "-- luck: format off\nlocal a = 1; local b = 2\n-- luck: format on\n";
    assert_eq!(
        default_format(input),
        input,
        "a format-off region must not drop `;` or split statements sharing a line"
    );
}

#[test]
fn bug_format_off_inside_a_table_keeps_the_region() {
    let input = "local t = {\n\t-- luck: format off\n\t1, 0,\n\t0, 1,\n\t-- luck: format on\n}\n";
    assert_eq!(default_format(input), input, "a format-off region inside a table must stay packed");
}

#[test]
fn bug_ignore_directive_survives_the_semicolon_guard() {
    let first_pass = default_format("f();\n-- luck: ignore\n(g   )()\n");
    assert_eq!(
        default_format(&first_pass),
        first_pass,
        "`luck: ignore` must keep `(g   )()` as written after the formatter adds its `;` guard"
    );
}

#[test]
fn bug_collapse_stays_idempotent_with_a_comment_in_a_list() {
    let options = conditional_collapse_options();
    let first_pass = format_once("if a then b(1, --[[c]] 2) end\n", LuaVersion::Lua54, &options);
    assert_eq!(
        format_once(&first_pass, LuaVersion::Lua54, &options),
        first_pass,
        "an argument list that claims a comment breaks across lines, so the `if` must not collapse around it"
    );
}

#[test]
fn bug_collapse_leaves_a_multi_line_body_expanded() {
    assert_eq!(
        format_once(
            "if a then\n  do x() end\nend\n",
            LuaVersion::Lua54,
            &conditional_collapse_options()
        ),
        "if a then\n\tdo\n\t\tx()\n\tend\nend\n",
        "collapsing means onto one line, so a body that spans lines must stay expanded"
    );
}

#[test]
fn bug_collapse_leaves_a_too_long_if_expanded() {
    let options = FormatOptions { line_width: 30, ..conditional_collapse_options() };
    assert_eq!(
        format_once(
            "if condition_name then call_function_name() end\n",
            LuaVersion::Lua54,
            &options
        ),
        "if condition_name then\n\tcall_function_name()\nend\n",
        "a collapsed `if` wider than `line_width` must expand instead"
    );
}

#[test]
fn bug_format_range_keeps_blank_line_runs_outside_the_selection() {
    let input = "local a = 1\n\n\nlocal   b   =   2\n";
    let result = luck_formatter::format_range(
        input,
        LuaVersion::Lua54,
        &FormatOptions::default(),
        range_of(input, "local   b   =   2"),
    );
    assert_eq!(
        result.output, "local a = 1\n\n\nlocal b = 2\n",
        "text outside the selection, blank lines included, must be copied verbatim"
    );
}

#[test]
fn bug_format_range_keeps_indentation_outside_the_selection() {
    let input = "function f()\n  local a=1\n  local   b=2\nend\n";
    let result = luck_formatter::format_range(
        input,
        LuaVersion::Lua54,
        &FormatOptions::default(),
        range_of(input, "local   b=2"),
    );
    assert!(
        result.output.contains("\n  local a=1\n"),
        "an unselected sibling must keep its indentation: {:?}",
        result.output
    );
}

#[test]
fn bug_format_range_keeps_crlf_outside_the_selection() {
    let input = "local   a   =   1\r\nlocal   b   =   2\r\n";
    let result = luck_formatter::format_range(
        input,
        LuaVersion::Lua54,
        &FormatOptions::default(),
        range_of(input, "local   a   =   1"),
    );
    assert!(
        result.output.ends_with("\nlocal   b   =   2\r\n"),
        "an unselected line must keep its CRLF: {:?}",
        result.output
    );
}

#[test]
fn bug_ignored_multi_line_stmt_keeps_its_lines_aligned() {
    let output = default_format("do\n    -- luck: ignore\n    local   t = {\n    }\nend\n");
    let indent_of = |needle: &str| {
        output
            .lines()
            .find(|line| line.contains(needle))
            .map(|line| line.strip_suffix(line.trim_start()).unwrap_or_default().to_owned())
            .expect("the line is in the output")
    };
    assert_eq!(
        indent_of("local   t"),
        indent_of("}"),
        "an ignored statement's lines must keep their relative layout: {output:?}"
    );
}

#[test]
fn bug_unix_line_endings_normalize_a_block_comment() {
    assert_eq!(
        default_format("--[[a\r\nb]]\r\nlocal x\r\n"),
        "--[[a\nb]]\nlocal x\n",
        "`line_endings = unix` must not leave a CRLF inside a comment"
    );
}
