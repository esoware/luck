use luck_ast::Synth;
use luck_token::LuaVersion;

use crate::common::{assert_compacts_to, compact_source};

/// Asserts that compacting the compact output of `source` changes nothing,
/// which catches output that re-parses but with a different meaning.
fn assert_idempotent(source: &str, version: LuaVersion) {
    let first = compact_source(source, version);
    let second = compact_source(&first, version);
    assert_eq!(first, second, "compact output of {source:?} changed when compacted again");
}

#[test]
fn output_capacity_hint_never_changes_output() {
    for version in LuaVersion::ALL {
        let source =
            format!("--{}\nlocal value = - -1; return value .. 'é'", "comment ".repeat(8192));
        let parsed = luck_parser::parse(&source, version);
        assert!(parsed.errors.is_empty(), "{:?}", parsed.errors);
        let expected = luck_codegen::compact(&parsed.block);
        for capacity in [0, 1, expected.len(), expected.len() + 1, source.len()] {
            let output = luck_codegen::compact_with_capacity(&parsed.block, capacity);
            assert_eq!(output, expected);
            assert_compacts_to(&output, &expected, version);
        }
    }
    let synth = Synth::new();
    let block = synth.block(Vec::new(), Some(synth.return_(vec![synth.string("source-free")])));
    let expected = luck_codegen::compact(&block);
    for capacity in [0, 1, expected.len(), 1024] {
        assert_eq!(luck_codegen::compact_with_capacity(&block, capacity), expected);
    }
}

#[test]
fn statements_compact_to_minimal_whitespace() {
    for (source, expected) in [
        ("local  x  =  1", "local x=1"),
        ("local  a , b  =  1 , 2", "local a,b=1,2"),
        ("local  x", "local x"),
        ("x  =  1", "x=1"),
        ("a , b  =  1 , 2", "a,b=1,2"),
        ("do  end", "do end"),
        ("while  true  do  end", "while true do end"),
        ("while true do  break  end", "while true do break end"),
        ("repeat  until  false", "repeat until false"),
        ("if  true  then  end", "if true then end"),
        ("if  x  then  a = 1  else  b = 2  end", "if x then a=1 else b=2 end"),
        ("if a then  elseif b then  else  end", "if a then elseif b then else end"),
        (
            "if a then\n  x = 1\n  y = 2\nelseif b then\n  z = 3\nelse\n  w = 4\nend",
            "if a then x=1 y=2 elseif b then z=3 else w=4 end",
        ),
        ("for  i = 1 , 10  do  end", "for i=1,10 do end"),
        ("for  i = 1 , 10 , 2  do  end", "for i=1,10,2 do end"),
        ("for  k , v  in  pairs ( t )  do  end", "for k,v in pairs(t)do end"),
        ("return", "return"),
        ("return  1 , 2", "return 1,2"),
    ] {
        assert_compacts_to(source, expected, LuaVersion::Lua51);
    }
}

#[test]
fn functions_compact_to_minimal_whitespace() {
    for (source, expected) in [
        ("function  f ( )  end", "function f()end"),
        ("function  a . b : c ( )  end", "function a.b:c()end"),
        ("local  function  f ( x , y )  return  x  end", "local function f(x,y)return x end"),
        ("x  =  function ( )  end", "x=function()end"),
        ("function  f ( ... )  return  ...  end", "function f(...)return...end"),
        ("function  f ( a , ... )  return  ...  end", "function f(a,...)return...end"),
        ("x  =  {  f  =  function ( a )  return  a  end  }", "x={f=function(a)return a end}"),
    ] {
        assert_compacts_to(source, expected, LuaVersion::Lua51);
    }
}

#[test]
fn expressions_compact_to_minimal_whitespace() {
    for (source, expected) in [
        ("x  =  a  +  b  *  c", "x=a+b*c"),
        ("x  =  a  +  b  *  ( c  -  d )  /  e", "x=a+b*(c-d)/e"),
        ("x  =  ( a  +  b )", "x=(a+b)"),
        ("x  =  not  y", "x=not y"),
        ("x  =  # t", "x=#t"),
        ("return  \"a\"  ..  \"b\"", "return\"a\"..\"b\""),
        ("x  =  \"a\"  ..  \"b\"  ..  \"c\"", "x=\"a\"..\"b\"..\"c\""),
        ("x  =  a . b", "x=a.b"),
        ("x  =  a . b . c . d", "x=a.b.c.d"),
        ("x  =  a [ 1 ]", "x=a[1]"),
        ("x  =  a . b . c [ 2 ]", "x=a.b.c[2]"),
    ] {
        assert_compacts_to(source, expected, LuaVersion::Lua51);
    }
}

#[test]
fn tables_compact_to_minimal_whitespace() {
    for (source, expected) in [
        ("x  =  {  1 ,  2 ,  3  }", "x={1,2,3}"),
        ("x  =  {  a  =  1 ,  b  =  2  }", "x={a=1,b=2}"),
        ("x  =  {  [ 1 ]  =  2  }", "x={[1]=2}"),
        ("x  =  {  }", "x={}"),
        ("x  =  { { 1 } , { 2 } }", "x={{1},{2}}"),
    ] {
        assert_compacts_to(source, expected, LuaVersion::Lua51);
    }
}

#[test]
fn calls_compact_to_minimal_whitespace() {
    for (source, expected) in [
        ("f ( 1 , 2 )", "f(1,2)"),
        ("f  \"hello\"", "f\"hello\""),
        ("f  {  1  }", "f{1}"),
        ("a : b ( 1 )", "a:b(1)"),
        ("f ( g ( h ( 1 ) ) )", "f(g(h(1)))"),
    ] {
        assert_compacts_to(source, expected, LuaVersion::Lua51);
    }
}

#[test]
fn compact_input_prints_unchanged() {
    for source in [
        "local a,b,c=1,2,3",
        "x=a+b-c*d/e%f^g",
        "x=a and b or c",
        "x=not not not true",
        "x=#t+#s",
        "return 1,2,3",
        "return'a'..'b'..'c'",
        "f()()()",
        "a.b.c:d()()",
        "x={[1]=2,[3]=4}",
        "for i=1,#t do end",
    ] {
        assert_compacts_to(source, source, LuaVersion::Lua51);
    }
}

#[test]
fn version_gated_statements_compact_to_minimal_whitespace() {
    for (source, expected, version) in [
        ("goto  skip  :: skip ::", "goto skip::skip::", LuaVersion::Lua52),
        ("; ;", ";;", LuaVersion::Lua52),
        ("global  x", "global x", LuaVersion::Lua55),
        ("global  function  f ( )  end", "global function f()end", LuaVersion::Lua55),
        ("global  *", "global*", LuaVersion::Lua55),
    ] {
        assert_compacts_to(source, expected, version);
    }
}

#[test]
fn luau_syntax_compacts_to_minimal_whitespace() {
    for (source, expected) in [
        ("x  +=  1", "x+=1"),
        ("for  i = 1 , 10  do  continue  end", "for i=1,10 do continue end"),
        ("local  x  =  if  a  then  1  else  2", "local x=if a then 1 else 2"),
        (
            "local result = receiver:map<<number, string>>(value)",
            "local result=receiver:map<<number,string>>(value)",
        ),
        (
            "local partial = f<<number>>\nlocal called = t.f<<number, string>>(value)",
            "local partial=f<<number>>local called=t.f<<number,string>>(value)",
        ),
        // Chaining requires parens (asexp allows one cast per simpleexp).
        ("local  x  =  ( y  ::  number )  ::  string", "local x=(y::number)::string"),
        (
            "return 1i, 1_000_000i, 0xffffffffffffffffi, -123i",
            "return 1i,1_000_000i,0xffffffffffffffffi,-123i",
        ),
        (
            "export local seen = 0i\nexport const MAX = 2i\n@native\nexport function run()\nreturn seen\nend",
            "export local seen=0i export const MAX=2i@native export function run()return seen end",
        ),
        ("@native function f() end", "@native function f()end"),
    ] {
        assert_compacts_to(source, expected, LuaVersion::Luau);
    }
}

#[test]
fn luau_interpolated_strings_keep_their_text_and_holes() {
    for (source, expected) in [
        ("local  x  =  `hello {name}`", "local x=`hello {name}`"),
        // A plain backtick string parses as a Begin("") + End(text) pair with
        // no expression; it must not gain a spurious `{}` hole.
        ("local  x  =  `plain text`", "local x=`plain text`"),
        ("local  x  =  `a {b} c`", "local x=`a {b} c`"),
    ] {
        assert_compacts_to(source, expected, LuaVersion::Luau);
    }
}

#[test]
fn luau_types_compact_to_minimal_whitespace() {
    for (source, expected) in [
        // Type annotations live in the AST and must survive round-tripping.
        ("local  x :  number  =  5", "local x:number=5"),
        ("local  x :  number ?  =  nil", "local x:number?=nil"),
        ("local  x :  | number | string  =  y", "local x:|number|string=y"),
        ("local  x :  A & B  =  y", "local x:A&B=y"),
        ("local  x :  typeof ( y )  =  y", "local x:typeof(y)=y"),
        ("type  Foo  =  number", "type Foo=number"),
        (
            "type  T  =  { read  x :  number , [ string ] :  boolean }",
            "type T={read x:number,[string]:boolean}",
        ),
        ("type  T  =  { number }", "type T={number}"),
        (
            "type  Fn  =  ( x :  number , string )  ->  boolean",
            "type Fn=(x:number,string)->boolean",
        ),
        ("function  f < T > ( a :  T )  :  T  return  a  end", "function f<T>(a:T):T return a end"),
        ("function  f ( )  :  ( number , ... string )  end", "function f():(number,...string)end"),
        ("type  function  Add ( a )  return  a  end", "type function Add(a)return a end"),
        ("type function Pair(t)\n\treturn t\nend", "type function Pair(t)return t end"),
        (
            "for  k :  string , v :  number  in  pairs ( t )  do  end",
            "for k:string,v:number in pairs(t)do end",
        ),
        ("for  i :  number  = 1 , 10  do  end", "for i:number=1,10 do end"),
    ] {
        assert_compacts_to(source, expected, LuaVersion::Luau);
    }
}

#[test]
fn compact_is_idempotent_on_lua51() {
    for source in [
        "local x = 1",
        "x = 1",
        "f()",
        "do end",
        "while true do end",
        "repeat until false",
        "if true then end",
        "if a then x = 1 elseif b then y = 2 else z = 3 end",
        "for i = 1, 10 do end",
        "for k, v in pairs(t) do end",
        "function f() end",
        "local function f(a, b) return a + b end",
        "return 1, 2, 3",
        "x = a + b * (c - d) / e",
        "x = {1, 2, a = 3, [4] = 5}",
        "x = \"a\" .. \"b\" .. \"c\"",
        "f(g(h(1)))",
        "x = a.b.c.d",
        "x = function(a) return a end",
        "if not a and not b then return -c .. d end",
        "for i = 1, #t do x = t[i] .. 'suffix' end",
        "local f = function(a, b, ...) return a + b, ... end",
        "x = {a = 1, b = {c = 2}, [3] = function() end}",
        "repeat local x = f() until not x",
    ] {
        assert_idempotent(source, LuaVersion::Lua51);
    }
}

#[test]
fn compact_is_idempotent_on_every_lua51_operator() {
    for source in [
        "x = a + b",
        "x = a - b",
        "x = a * b",
        "x = a / b",
        "x = a % b",
        "x = a ^ b",
        "x = a .. b",
        "x = a == b",
        "x = a ~= b",
        "x = a < b",
        "x = a <= b",
        "x = a > b",
        "x = a >= b",
        "x = a and b",
        "x = a or b",
        "x = -a",
        "x = not a",
        "x = #a",
        "x = - -a",
        "x = not not a",
        "x = ##t",
    ] {
        assert_idempotent(source, LuaVersion::Lua51);
    }
}

#[test]
fn compact_is_idempotent_on_every_lua53_operator() {
    for source in
        ["x = a // b", "x = a & b", "x = a | b", "x = a ~ b", "x = a << b", "x = a >> b", "x = ~a"]
    {
        assert_idempotent(source, LuaVersion::Lua53);
    }
}

#[test]
fn compact_is_idempotent_across_separators() {
    for (source, version) in [
        // The space in `1 ..2` must survive; `1..2` is a malformed number.
        ("return 1 .. 2", LuaVersion::Lua51),
        // `a- -b` must not reparse as `a--b` (a comment).
        ("x = a - -b", LuaVersion::Lua51),
        (r#"x = "a" .. .5"#, LuaVersion::Lua51),
        ("x = 1 .. .5", LuaVersion::Lua51),
        ("x = {[ [[key]] ] = 1}", LuaVersion::Lua51),
        // `>` `=` must not become `>=`.
        ("local x <const> = 1", LuaVersion::Lua54),
        // After `end`, a statement starting with `(` must stay its own
        // statement.
        ("do end\n;(f)()", LuaVersion::Lua51),
        ("f(x);(g)(y)", LuaVersion::Lua52),
        // Without the `;` this is one chained call, and must stay one.
        ("f(x)(g)(y)", LuaVersion::Lua51),
    ] {
        assert_idempotent(source, version);
    }
}

#[test]
fn compact_is_idempotent_on_version_gated_syntax() {
    for (source, version) in [
        ("goto skip ::skip::", LuaVersion::Lua52),
        (";", LuaVersion::Lua52),
        ("global x", LuaVersion::Lua55),
        ("global function f() end", LuaVersion::Lua55),
        ("global *", LuaVersion::Lua55),
        ("local x = if a then 1 else 2", LuaVersion::Luau),
        ("x += 1", LuaVersion::Luau),
        ("type Foo = number", LuaVersion::Luau),
        ("for i = 1, 10 do continue end", LuaVersion::Luau),
    ] {
        assert_idempotent(source, version);
    }
}
