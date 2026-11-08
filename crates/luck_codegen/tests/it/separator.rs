//! The separators compact output keeps: a space where two pieces would merge
//! into another token, and a `;` where a statement would extend the one
//! before it.

use luck_token::LuaVersion;

use crate::common::assert_compacts_to;

// `end"foo"` cannot occur in a valid AST. A bare string literal is not a
// statement, and `end` is a keyword, so it can never be a call prefix.
#[test]
fn space_separates_adjacent_words() {
    for (source, expected) in [
        ("do end\nif true then end", "do end if true then end"),
        ("do end\nf()", "do end f()"),
        ("do end do end", "do end do end"),
        (
            "while true do break end\nwhile true do break end",
            "while true do break end while true do break end",
        ),
        ("for i=1,1 do end\nfor i=1,1 do end", "for i=1,1 do end for i=1,1 do end"),
        ("if true then return 1 end", "if true then return 1 end"),
        ("if true then return 1 else return 2 end", "if true then return 1 else return 2 end"),
        ("do return function() end end", "do return function()end end"),
        ("function f() return not x end", "function f()return not x end"),
        ("function f() return true end", "function f()return true end"),
        ("function f() return nil end", "function f()return nil end"),
        ("repeat until not done", "repeat until not done"),
    ] {
        assert_compacts_to(source, expected, LuaVersion::Lua51);
    }
}

#[test]
fn words_glue_to_pieces_that_cannot_extend_them() {
    for (source, expected) in [
        // A quote is not word-like, so `return` glues straight onto the
        // literal.
        ("function f() return 'hello' end", "function f()return'hello'end"),
        ("function f() return {} end", "function f()return{}end"),
        ("function f() return (1) end", "function f()return(1)end"),
        ("function f() return -1 end", "function f()return-1 end"),
        ("function f() return #t end", "function f()return#t end"),
        ("x = not(y)", "x=not(y)"),
        ("x = not 'hello'", "x=not'hello'"),
        ("x = not {}", "x=not{}"),
    ] {
        assert_compacts_to(source, expected, LuaVersion::Lua51);
    }
}

#[test]
fn space_keeps_a_number_from_absorbing_the_next_piece() {
    for (source, expected) in [
        // `1..` is a malformed number, so the space is required.
        ("return 1 .. 2", "return 1 ..2"),
        ("return 1.0 .. 'x'", "return 1.0 ..'x'"),
        // `.5` is a number, so it is word-like and cannot glue to `return`.
        ("return .5 .. 'x'", "return .5 ..'x'"),
        ("return 1e5 .. 'x'", "return 1e5 ..'x'"),
        ("return 0xff .. 'x'", "return 0xff ..'x'"),
        ("x = 1 and 2", "x=1 and 2"),
        ("x = 1 or 2", "x=1 or 2"),
    ] {
        assert_compacts_to(source, expected, LuaVersion::Lua51);
    }
}

#[test]
fn space_keeps_concat_from_absorbing_a_dot_number() {
    for (source, expected) in [
        // `..` followed by `.5` must not produce `...5`, which reads as a
        // vararg and then `5`.
        (r#"x = "a" .. .5"#, r#"x="a".. .5"#),
        // Both hazards at once: number before `..`, and `..` before a dot
        // number.
        ("x = 1 .. .5", "x=1 .. .5"),
    ] {
        assert_compacts_to(source, expected, LuaVersion::Lua51);
    }
}

#[test]
fn space_keeps_minuses_from_opening_a_comment() {
    for (source, expected) in [("x = a - -b", "x=a- -b"), ("x = a - - -b", "x=a- - -b")] {
        assert_compacts_to(source, expected, LuaVersion::Lua51);
    }
}

#[test]
fn slashes_never_form_floor_division() {
    assert_compacts_to("x = a / (1/b)", "x=a/(1/b)", LuaVersion::Lua51);
}

#[test]
fn space_keeps_brackets_from_opening_a_long_string() {
    assert_compacts_to("x = {[ [[key]] ] = 1}", "x={[ [[key]]]=1}", LuaVersion::Lua51);
}

#[test]
fn space_keeps_angle_brackets_from_forming_a_shift_or_comparison() {
    for (source, expected, version) in [
        // `>` `=` must not become `>=`.
        ("local x <const> = 1", "local x<const> =1", LuaVersion::Lua54),
        // `> =` keeps a space: glued `>=` would lex as one token and break
        // the generic close on re-parse.
        ("local  x :  mod . Name < A , B >  =  y", "local x:mod.Name<A,B> =y", LuaVersion::Luau),
        // Two `>` closes are separated by a space so they cannot lex as a
        // single `>>` shift; `>` before `=` is spaced for the same reason.
        (
            "local  x :  Map < string , Array < number > >  =  m",
            "local x:Map<string,Array<number> > =m",
            LuaVersion::Luau,
        ),
        // `>` before the alias `=` is spaced to avoid forming `>=`.
        ("type  Foo < T , U = string >  =  T", "type Foo<T,U=string> =T", LuaVersion::Luau),
    ] {
        assert_compacts_to(source, expected, version);
    }
}

#[test]
fn space_separates_an_interpolation_opener_from_a_table() {
    // `{{` is a parse error in Luau; the printer must separate the
    // interpolation opener from a leading table constructor.
    assert_compacts_to(
        "return  `list { {1, 2} } tail`",
        "return`list { {1,2}} tail`",
        LuaVersion::Luau,
    );
}

#[test]
fn symbols_that_cannot_merge_stay_glued() {
    for (source, expected, version) in [
        // `#` is not word-like, so `##` needs no separator.
        ("x = ##t", "x=##t", LuaVersion::Lua51),
        ("x = #'hello'", "x=#'hello'", LuaVersion::Lua51),
        ("x = #{1,2,3}", "x=#{1,2,3}", LuaVersion::Lua51),
        ("x = #(t)", "x=#(t)", LuaVersion::Lua51),
        // Lua 5.3+: `~` is bitwise not, so `~(~y)` nests two of them.
        ("x = ~(~y)", "x=~(~y)", LuaVersion::Lua53),
        ("::a:: ::b::", "::a::::b::", LuaVersion::Lua52),
        ("::lbl:: x = 1", "::lbl::x=1", LuaVersion::Lua52),
    ] {
        assert_compacts_to(source, expected, version);
    }
}

#[test]
fn semicolon_separates_a_statement_that_starts_with_a_paren() {
    for (source, expected, version) in [
        // Lua 5.2+: a bare `;` is an EmptyStatement, which survives the round
        // trip.
        ("do end\n;(f)()", "do end;(f)()", LuaVersion::Lua52),
        // Lua 5.1 has no empty statement, so its `;` is only an optional
        // separator, but the printer adds one before any statement that
        // starts with `(`.
        ("do end\n;(f)()", "do end;(f)()", LuaVersion::Lua51),
        ("do end\n(f)()", "do end;(f)()", LuaVersion::Lua51),
        // Two statements. Without the `;`, `1(f)` reads as a call.
        ("x = 1\n(f)()", "x=1;(f)()", LuaVersion::Lua51),
        // The `;` separates two call statements. Dropping it would leave the
        // chained call `f(x)(g)(y)`.
        ("f(x);(g)(y)", "f(x);(g)(y)", LuaVersion::Lua52),
        // Without the `;` this is one chained call, and it stays one.
        ("f(x)(g)(y)", "f(x)(g)(y)", LuaVersion::Lua51),
        // Dropping the `;` would leave one chained call, `f()(g or h)<<T>>()`,
        // which still parses, so only the exact output catches it.
        ("f();(g or h)<<T>>()", "f();(g or h)<<T>>()", LuaVersion::Luau),
        ("f();(g or h)<<T>>.x = 1", "f();(g or h)<<T>>.x=1", LuaVersion::Luau),
        ("f();(g or h)<<T>>[1] = 1", "f();(g or h)<<T>>[1]=1", LuaVersion::Luau),
        ("f();(g or h)<<T>>.x += 1", "f();(g or h)<<T>>.x+=1", LuaVersion::Luau),
    ] {
        assert_compacts_to(source, expected, version);
    }
}
