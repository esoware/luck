use luck_token::LuaVersion;

use crate::common::assert_compacts_to;

#[test]
fn space_separates_adjacent_words() {
    for (source, expected) in [
        (
            "\
do end
if true then end",
            "do end if true then end",
        ),
        (
            "\
while true do break end
f()",
            "while true do break end f()",
        ),
        ("if true then return 1 else return 2 end", "if true then return 1 else return 2 end"),
        ("do return function() end end", "do return function()end end"),
        ("function f() return not x end", "function f()return not x end"),
        ("repeat until not done", "repeat until not done"),
    ] {
        assert_compacts_to(source, expected, LuaVersion::Lua51);
    }
}

#[test]
fn words_glue_to_pieces_that_cannot_extend_them() {
    for (source, expected) in [
        ("function f() return 'hello' end", "function f()return'hello'end"),
        ("function f() return {} end", "function f()return{}end"),
        ("function f() return (1) end", "function f()return(1)end"),
        ("function f() return -1 end", "function f()return-1 end"),
        ("function f() return #t end", "function f()return#t end"),
        // A name never reads on through `.`, so `return.5` is `return` `.5`.
        ("function f() return .5 end", "function f()return.5 end"),
        ("x = not(y)", "x=not(y)"),
        ("x = not 'hello'", "x=not'hello'"),
        ("x = a and .5 or b", "x=a and.5 or b"),
    ] {
        assert_compacts_to(source, expected, LuaVersion::Lua51);
    }
}

#[test]
fn space_keeps_a_number_from_absorbing_the_next_piece() {
    let (lua51, lua54) = (LuaVersion::Lua51, LuaVersion::Lua54);
    for (source, expected, version) in [
        // A number reads on through letters, digits, and dots.
        ("return 1 .. 2", "return 1 ..2", lua51),
        ("return 1.0 .. 'x'", "return 1. ..'x'", lua54),
        ("return .5 .. 'x'", "return.5 ..'x'", lua51),
        ("return 0xffffffffffffffff .. 'x'", "return 0xffffffffffffffff ..'x'", lua54),
        ("x = 1 and 2", "x=1 and 2", lua51),
        ("x = 1.0 or 2", "x=1. or 2", lua54),
        ("x = 1.0 or 2", "x=1 or 2", lua51),
    ] {
        assert_compacts_to(source, expected, version);
    }
}

#[test]
fn space_keeps_concat_from_absorbing_a_dot() {
    for (source, expected) in [
        // `...5` would read as a vararg and then `5`.
        (r#"x = "a" .. .5"#, r#"x="a".. .5"#),
        ("x = 1 .. .5", "x=1 .. .5"),
        ("function f(...) return 'a' .. ... end", "function f(...)return'a'.. ...end"),
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
fn space_keeps_a_bracket_from_opening_a_long_string() {
    // Each value has more backslashes than its long brackets cost, so it
    // keeps them.
    for (source, expected) in [
        (r"x = {[ [[\\\]] ] = 1}", r"x={[ [[\\\]]]=1}"),
        (r"x = t[ [==[]]\\\\\]==] ]", r"x=t[ [=[]]\\\\\]=]]"),
    ] {
        assert_compacts_to(source, expected, LuaVersion::Lua51);
    }
}

#[test]
fn space_keeps_a_closing_angle_bracket_from_forming_greater_equal() {
    for (source, expected, version) in [
        ("local x <const> = 1", "local x<const> =1", LuaVersion::Lua54),
        ("local  x :  mod . Name < A , B >  =  y", "local x:mod.Name<A,B> =y", LuaVersion::Luau),
        ("type  Foo < T , U = string >  =  T", "type Foo<T,U=string> =T", LuaVersion::Luau),
        ("local x = f<<T>> == 1", "local x=f<<T>> ==1", LuaVersion::Luau),
    ] {
        assert_compacts_to(source, expected, version);
    }
}

#[test]
fn angle_brackets_stay_glued_in_luau_which_has_no_shift() {
    for source in [
        "local x:Map<string,Array<number>> =m",
        "local x=f<<Array<number>>>()",
        "local x:Box<<T>(T)->T> =y",
        "local x=f<<T>>>=1",
    ] {
        assert_compacts_to(source, source, LuaVersion::Luau);
    }
}

#[test]
fn space_separates_an_interpolation_opener_from_a_table() {
    assert_compacts_to(
        "return  `list { {1, 2} } and { {} == x }`",
        "return`list { {1,2}} and { {}==x}`",
        LuaVersion::Luau,
    );
}

#[test]
fn symbols_that_cannot_merge_stay_glued() {
    for (source, expected, version) in [
        ("x = ##t", "x=##t", LuaVersion::Lua51),
        ("x = #{1,2,3}", "x=#{1,2,3}", LuaVersion::Lua51),
        ("x = a / (1/b)", "x=a/(1/b)", LuaVersion::Lua51),
        ("x = ~(~y)", "x=~(~y)", LuaVersion::Lua53),
        ("::a:: ::b::", "::a::::b::", LuaVersion::Lua52),
        ("::lbl:: x = 1", "::lbl::x=1", LuaVersion::Lua52),
    ] {
        assert_compacts_to(source, expected, version);
    }
}

#[test]
fn semicolon_separates_a_stmt_that_starts_with_a_paren_from_one_it_would_call() {
    for (source, expected, version) in [
        // Without the `;`, `1(f)` reads as a call.
        (
            "\
x = 1
(f)()",
            "x=1;(f)()",
            LuaVersion::Lua51,
        ),
        // Without the `;`, `f(x)(g)(y)` is one chained call.
        ("f(x);(g)(y)", "f(x);(g)(y)", LuaVersion::Lua51),
        ("f(x)(g)(y)", "f(x)(g)(y)", LuaVersion::Lua51),
        ("repeat until x;(f)()", "repeat until x;(f)()", LuaVersion::Lua51),
        ("local x;(f).y = 1", "local x;(f).y=1", LuaVersion::Lua51),
        // Lua 5.2+: the tree holds no `;`, so a run prints as the one a `(` needs.
        ("f(x);(g)(y)", "f(x);(g)(y)", LuaVersion::Lua52),
        ("f(x);;(g)(y)", "f(x);(g)(y)", LuaVersion::Lua52),
        // Dropping the `;` would still parse, as `f()(g or h)<<T>>()`.
        ("f();(g or h)<<T>>()", "f();(g or h)<<T>>()", LuaVersion::Luau),
        ("f();(g or h)<<T>>.x = 1", "f();(g or h)<<T>>.x=1", LuaVersion::Luau),
        ("f();(g or h)<<T>>[1] += 1", "f();(g or h)<<T>>[1]+=1", LuaVersion::Luau),
        ("type T = U;(f)()", "type T=U;(f)()", LuaVersion::Luau),
    ] {
        assert_compacts_to(source, expected, version);
    }
}

#[test]
fn no_semicolon_follows_a_stmt_that_a_paren_cannot_continue() {
    for (source, expected, version) in [
        ("do end (f)()", "do end(f)()", LuaVersion::Lua51),
        ("while x do end (f)()", "while x do end(f)()", LuaVersion::Lua51),
        ("if x then end (f)()", "if x then end(f)()", LuaVersion::Lua51),
        ("for i = 1, 2 do end (f)()", "for i=1,2 do end(f)()", LuaVersion::Lua51),
        ("for k in p do end (f)()", "for k in p do end(f)()", LuaVersion::Lua51),
        ("function f() end (f)()", "function f()end(f)()", LuaVersion::Lua51),
        ("local function f() end (f)()", "local function f()end(f)()", LuaVersion::Lua51),
        ("goto l ::l:: (f)()", "goto l::l::(f)()", LuaVersion::Lua52),
        ("while x do goto l (f)() ::l:: end", "while x do goto l(f)()::l::end", LuaVersion::Lua52),
        ("while x do break (f)() end", "while x do break(f)()end", LuaVersion::Lua52),
        ("global function f() end (f)()", "global function f()end(f)()", LuaVersion::Lua55),
        ("global * (f)()", "global*(f)()", LuaVersion::Lua55),
        ("type function F() end (f)()", "type function F()end(f)()", LuaVersion::Luau),
    ] {
        assert_compacts_to(source, expected, version);
    }
}
