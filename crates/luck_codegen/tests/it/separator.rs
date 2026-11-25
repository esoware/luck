use luck_codegen::LiteralSpelling;

use crate::common::{assert_compacts_to, assert_spells};

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
        assert_compacts_to(source, expected);
    }
}

#[test]
fn words_glue_to_pieces_that_cannot_extend_them() {
    for (source, expected) in [
        ("function f() return 'hello' end", "function f()return'hello'end"),
        ("function f() return {} end", "function f()return{}end"),
        ("function f() return (g()) end", "function f()return(g())end"),
        ("function f() return -1 end", "function f()return-1 end"),
        ("function f() return #t end", "function f()return#t end"),
        // A name never reads on through `.`, so `return.5` is `return` `.5`.
        ("function f() return .5 end", "function f()return.5 end"),
        ("x = not(y or z)", "x=not(y or z)"),
        ("x = not 'hello'", "x=not'hello'"),
        ("x = a and .5 or b", "x=a and.5 or b"),
    ] {
        assert_compacts_to(source, expected);
    }
}

#[test]
fn space_keeps_a_number_from_absorbing_the_next_piece() {
    for (source, expected) in [
        // A numeral with no letter reads on through letters, digits, and dots.
        ("return 1 .. 2", "return 1 ..2"),
        ("return 1.0 .. 'x'", "return 1 ..'x'"),
        ("return 1.5 .. 'x'", "return 1.5 ..'x'"),
        ("return .5 .. 'x'", "return.5 ..'x'"),
        ("x = 1 and 2", "x=1 and 2"),
        ("x = 1.0 or 2", "x=1 or 2"),
        ("x = 0xff or 2", "x=255 or 2"),
    ] {
        assert_spells(source, expected, LiteralSpelling::Shortest);
    }
}

#[test]
fn a_numeral_past_its_first_letter_glues_to_a_concat() {
    for (source, expected) in [
        // The lexer reads a `.` into a numeral only before its first letter.
        ("return 0xff .. 'x'", "return 0xff..'x'"),
        ("return 0b11 .. 'x'", "return 0b11..'x'"),
        ("return 1e5 .. 'x'", "return 1e5..'x'"),
        ("return 1.5e-5 .. 'x'", "return 1.5e-5..'x'"),
        ("return 1i .. 'x'", "return 1i..'x'"),
        ("return 1_0 .. 'x'", "return 1_0 ..'x'"),
        ("return 0xffffffffffffffff .. 2", "return 0xffffffffffffffff..2"),
        ("x = 0xff and 1e5 or 2", "x=0xff and 1e5 or 2"),
    ] {
        assert_compacts_to(source, expected);
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
        assert_compacts_to(source, expected);
    }
}

#[test]
fn space_keeps_minuses_from_opening_a_comment() {
    for (source, expected) in [("x = a - -b", "x=a- -b"), ("x = a - - -b", "x=a- - -b")] {
        assert_compacts_to(source, expected);
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
        assert_spells(source, expected, LiteralSpelling::Shortest);
    }
}

#[test]
fn space_keeps_a_closing_angle_bracket_from_forming_greater_equal() {
    for (source, expected) in [
        ("local  x :  mod . Name < A , B >  =  y", "local x:mod.Name<A,B> =y"),
        ("type  Foo < T , U = string >  =  T", "type Foo<T,U=string> =T"),
        ("local x = f<<T>> == 1", "local x=f<<T>> ==1"),
    ] {
        assert_compacts_to(source, expected);
    }
}

#[test]
fn angle_brackets_stay_glued_with_no_shift_to_form() {
    for source in [
        "local x:Map<string,Array<number>> =m",
        "local x=f<<Array<number>>>()",
        "local x:Box<<T>(T)->T> =y",
        "local x=f<<T>>>=1",
    ] {
        assert_compacts_to(source, source);
    }
}

#[test]
fn space_separates_an_interpolation_opener_from_a_table() {
    assert_compacts_to(
        "return  `list { {1, 2} } and { {} == x }`",
        "return`list { {1,2}} and { {}==x}`",
    );
}

#[test]
fn symbols_that_cannot_merge_stay_glued() {
    for (source, expected) in
        [("x = ##t", "x=##t"), ("x = #{1,2,3}", "x=#{1,2,3}"), ("x = a / (1/b)", "x=a/(1/b)")]
    {
        assert_compacts_to(source, expected);
    }
}

#[test]
fn semicolon_separates_a_stmt_that_starts_with_a_paren_from_one_it_would_call() {
    for (source, expected) in [
        // Without the `;`, `1(f or g)` reads as a call.
        (
            "\
x = 1
(f or g)()",
            "x=1;(f or g)()",
        ),
        // Without the `;`, `f(x)(g or h)(y)` is one chained call.
        ("f(x);(g or h)(y)", "f(x);(g or h)(y)"),
        ("f(x)(g or h)(y)", "f(x)(g or h)(y)"),
        ("repeat until x;(f or g)()", "repeat until x;(f or g)()"),
        ("local x;(f or g).y = 1", "local x;(f or g).y=1"),
        ("local x;('s'):f()", "local x;('s'):f()"),
        ("local x;(f()).y = 1", "local x;(f()).y=1"),
        // Parentheses around a name or a chain change nothing, so they go,
        // and the `;` with them.
        ("local x;(f)()", "local x f()"),
        ("local x;(f.g).y = 1", "local x f.g.y=1"),
        // Dropping the `;` would still parse, as `f()(g or h)<<T>>()`.
        ("f();(g or h)<<T>>()", "f();(g or h)<<T>>()"),
        ("f();(g or h)<<T>>.x = 1", "f();(g or h)<<T>>.x=1"),
        ("f();(g or h)<<T>>[1] += 1", "f();(g or h)<<T>>[1]+=1"),
        ("type T = U;(f or g)()", "type T=U;(f or g)()"),
    ] {
        assert_compacts_to(source, expected);
    }
}

#[test]
fn no_semicolon_follows_a_stmt_that_a_paren_cannot_continue() {
    for (source, expected) in [
        ("do end (f or g)()", "do end(f or g)()"),
        ("while x do end (f or g)()", "while x do end(f or g)()"),
        ("if x then end (f or g)()", "if x then end(f or g)()"),
        ("for i = 1, 2 do end (f or g)()", "for i=1,2 do end(f or g)()"),
        ("for k in p do end (f or g)()", "for k in p do end(f or g)()"),
        ("function f() end (f or g)()", "function f()end(f or g)()"),
        ("local function f() end (f or g)()", "local function f()end(f or g)()"),
        ("type function F() end (f or g)()", "type function F()end(f or g)()"),
    ] {
        assert_compacts_to(source, expected);
    }
}
