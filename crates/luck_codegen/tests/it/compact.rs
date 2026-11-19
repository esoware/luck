use luck_allocator::Allocator;
use luck_ast::{Chunk, Expression, MAX_NESTING_DEPTH, SpanCounter, Synth, block_nesting_depth};
use luck_testgen::with_the_smallest_stack;
use luck_token::{BinOp, LuaVersion, UnOp};

use crate::common::{assert_compacts_to, assert_reprints};

#[test]
fn a_capacity_hint_never_changes_the_output() {
    for version in LuaVersion::ALL {
        let source = format!(
            "\
--{}
local value = - -1; return value .. '\u{e9}'",
            "comment ".repeat(8192)
        );
        let allocator = Allocator::new();
        let parsed = luck_parser::parse(&allocator, &source, version);
        assert!(parsed.errors.is_empty(), "{:?}", parsed.errors);
        let expected = luck_codegen::compact(&parsed.chunk);
        for capacity in [0, 1, expected.len(), expected.len() + 1, source.len()] {
            let output = luck_codegen::compact_with_capacity(&parsed.chunk, capacity);
            assert_eq!(output, expected);
        }
        assert_reprints(&parsed.chunk);
    }
}

#[test]
fn a_capacity_hint_the_allocator_cannot_meet_is_dropped() {
    let allocator = Allocator::new();
    let spans = SpanCounter::new();
    let synth = Synth::new(&allocator, &spans);
    let chunk = Chunk::new(LuaVersion::Lua54, synth.block([synth.return_([synth.number("1")])]));
    for capacity in [usize::MAX, isize::MAX.unsigned_abs()] {
        let output = luck_codegen::compact_with_capacity(&chunk, capacity);
        assert_eq!(output, "return 1", "capacity {capacity}");
    }
}

/// Nests its operand exactly one level deeper.
type Wrap = for<'ast> fn(Synth<'ast>, Expression<'ast>) -> Expression<'ast>;

/// `return x`, with `wrap` applied to `x` until the block nests `depth` deep.
fn nested_return(synth: Synth<'_>, depth: usize, wrap: Wrap) -> Chunk<'_> {
    let mut expr = synth.name_expr("x");
    for _ in 1..depth {
        expr = wrap(synth, expr);
    }
    let block = synth.block([synth.return_([expr])]);
    assert_eq!(block_nesting_depth(&block), depth, "each wrap adds one level");
    Chunk::new(LuaVersion::Lua54, block)
}

#[test]
fn a_tree_at_the_nesting_cap_prints_and_reparses() {
    with_the_smallest_stack(|| {
        let wraps: [(&str, Wrap); 6] = [
            ("parentheses", |synth, expr| synth.parenthesized(expr)),
            ("unary operator", |synth, expr| synth.unary_op(UnOp::Not, expr)),
            ("call argument", |synth, expr| synth.call(synth.name_expr("f"), [expr])),
            ("method argument", |synth, expr| synth.method_call(synth.name_expr("o"), "m", [expr])),
            ("index", |synth, expr| synth.index(synth.name_expr("t"), expr)),
            ("table", |synth, expr| synth.array([expr])),
        ];
        for (label, wrap) in wraps {
            let allocator = Allocator::new();
            let spans = SpanCounter::new();
            let chunk = nested_return(Synth::new(&allocator, &spans), MAX_NESTING_DEPTH, wrap);
            let output = luck_codegen::compact(&chunk);
            let reparsed = luck_parser::parse(&allocator, &output, LuaVersion::Lua54);
            assert!(reparsed.errors.is_empty(), "{label}: {:?}", reparsed.errors.first());
            assert_eq!(block_nesting_depth(&reparsed.chunk.block), MAX_NESTING_DEPTH, "{label}");
        }
    });
}

#[test]
fn a_run_of_any_length_prints_as_one_level() {
    let allocator = Allocator::new();
    let spans = SpanCounter::new();
    let synth = Synth::new(&allocator, &spans);
    let mut sum = synth.name_expr("x");
    let mut calls = synth.name_expr("f");
    for _ in 0..200_000u32 {
        sum = synth.binary_op(sum, BinOp::Add, synth.number("1"));
    }
    for _ in 0..2_000u32 {
        calls = synth.call(calls, []);
    }
    let chunk = Chunk::new(LuaVersion::Lua54, synth.block([synth.return_([sum, calls])]));
    assert_eq!(block_nesting_depth(&chunk.block), 2);
    let output = luck_codegen::compact(&chunk);
    assert_eq!(output.len(), "return x".len() + 200_000 * 2 + ",f".len() + 2_000 * 2);
    let reparsed = luck_parser::parse(&allocator, &output, LuaVersion::Lua54);
    assert!(reparsed.errors.is_empty(), "{:?}", reparsed.errors.first());
    assert_eq!(block_nesting_depth(&reparsed.chunk.block), 2);
    assert_eq!(luck_codegen::compact(&reparsed.chunk), output);
}

#[test]
fn stmts_compact_to_minimal_whitespace() {
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
        ("repeat local x = f() until not x", "repeat local x=f()until not x"),
        ("if  true  then  end", "if true then end"),
        ("if  x  then  a = 1  else  b = 2  end", "if x then a=1 else b=2 end"),
        ("if a then  elseif b then  else  end", "if a then elseif b then else end"),
        (
            "\
if a then
  x = 1
  y = 2
elseif b then
  z = 3
else
  w = 4
end",
            "if a then x=1 y=2 elseif b then z=3 else w=4 end",
        ),
        ("if not a and not b then return -c .. d end", "if not a and not b then return-c..d end"),
        ("for  i = 1 , 10  do  end", "for i=1,10 do end"),
        ("for  i = 1 , 10 , 2  do  end", "for i=1,10,2 do end"),
        ("for i = 1, #t do x = t[i] .. 'suffix' end", "for i=1,#t do x=t[i]..'suffix'end"),
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
        (
            "local f = function(a, b, ...) return a + b, ... end",
            "local f=function(a,b,...)return a+b,...end",
        ),
        ("x  =  {  f  =  function ( a )  return  a  end  }", "x={f=function(a)return a end}"),
    ] {
        assert_compacts_to(source, expected, LuaVersion::Lua51);
    }
}

#[test]
fn exprs_compact_to_minimal_whitespace() {
    for (source, expected) in [
        ("x  =  a  +  b  *  ( c  -  d )  /  e", "x=a+b*(c-d)/e"),
        ("x  =  ( a  +  b )", "x=(a+b)"),
        ("x  =  not  y", "x=not y"),
        ("x  =  # t", "x=#t"),
        ("return  \"a\"  ..  \"b\"  ..  \"c\"", "return\"a\"..\"b\"..\"c\""),
        ("x  =  a . b . c [ 2 ]", "x=a.b.c[2]"),
        ("return  ...  ..  x", "return.....x"),
    ] {
        assert_compacts_to(source, expected, LuaVersion::Lua51);
    }
}

#[test]
fn operators_compact_to_minimal_whitespace() {
    for (source, expected, version) in [
        ("x = a + b - c * d / e % f ^ g", "x=a+b-c*d/e%f^g", LuaVersion::Lua51),
        ("x = a .. b", "x=a..b", LuaVersion::Lua51),
        ("x = a == b", "x=a==b", LuaVersion::Lua51),
        ("x = a ~= b", "x=a~=b", LuaVersion::Lua51),
        ("x = a < b", "x=a<b", LuaVersion::Lua51),
        ("x = a <= b", "x=a<=b", LuaVersion::Lua51),
        ("x = a > b", "x=a>b", LuaVersion::Lua51),
        ("x = a >= b", "x=a>=b", LuaVersion::Lua51),
        ("x = a and b or c", "x=a and b or c", LuaVersion::Lua51),
        ("x = -a", "x=-a", LuaVersion::Lua51),
        ("x = - -a", "x=- -a", LuaVersion::Lua51),
        ("x = not not a", "x=not not a", LuaVersion::Lua51),
        // Lua 5.3+
        ("x = a // b", "x=a//b", LuaVersion::Lua53),
        ("x = a & b | c ~ d", "x=a&b|c~d", LuaVersion::Lua53),
        ("x = a << b >> c", "x=a<<b>>c", LuaVersion::Lua53),
        ("x = ~a", "x=~a", LuaVersion::Lua53),
    ] {
        assert_compacts_to(source, expected, version);
    }
}

#[test]
fn tables_compact_to_minimal_whitespace() {
    for (source, expected) in [
        ("x  =  {  1 ,  2 ,  3  }", "x={1,2,3}"),
        ("x  =  {  a  =  1 ;  b  =  2 ;  }", "x={a=1,b=2}"),
        ("x  =  {  [ 1 ]  =  2  }", "x={[1]=2}"),
        ("x  =  {  }", "x={}"),
        ("x  =  { { 1 } , { 2 } }", "x={{1},{2}}"),
        ("x = {a = 1, b = {c = 2}, [3] = function() end}", "x={a=1,b={c=2},[3]=function()end}"),
    ] {
        assert_compacts_to(source, expected, LuaVersion::Lua51);
    }
}

#[test]
fn calls_compact_to_minimal_whitespace() {
    for (source, expected) in [
        ("f ( 1 , 2 )", "f(1,2)"),
        ("f  \"hello\"", "f\"hello\""),
        ("f  [[long]]", "f[[long]]"),
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
        "x=a and b or c",
        "x=#t+#s",
        "return'a'..'b'..'c'",
        "f()()()",
        "a.b.c:d()()",
        "x={[1]=2,[3]=4}",
    ] {
        assert_compacts_to(source, source, LuaVersion::Lua51);
    }
}

#[test]
fn version_gated_stmts_compact_to_minimal_whitespace() {
    for (source, expected, version) in [
        ("goto  skip  :: skip ::", "goto skip::skip::", LuaVersion::Lua52),
        ("; ;", "", LuaVersion::Lua52),
        ("local  x <const>, y <close> =  1, z", "local x<const>,y<close> =1,z", LuaVersion::Lua54),
        ("global  x", "global x", LuaVersion::Lua55),
        ("global  function  f ( )  end", "global function f()end", LuaVersion::Lua55),
        ("global  *", "global*", LuaVersion::Lua55),
        ("global <const> *", "global<const>*", LuaVersion::Lua55),
        (
            "function  f ( ... t )  return  t  end",
            "function f(...t)return t end",
            LuaVersion::Lua55,
        ),
    ] {
        assert_compacts_to(source, expected, version);
    }
}

#[test]
fn luau_syntax_compacts_to_minimal_whitespace() {
    for (source, expected) in [
        ("x  +=  1  x ..= 'a'", "x+=1 x..='a'"),
        ("for  i = 1 , 10  do  continue  end", "for i=1,10 do continue end"),
        (
            "local  x  =  if  a  then  1  elseif b then 2 else  3",
            "local x=if a then 1 elseif b then 2 else 3",
        ),
        (
            "local result = receiver:map<<number, string>>(value)",
            "local result=receiver:map<<number,string>>(value)",
        ),
        (
            "\
local partial = f<<number>>
local called = t.f<<number, string>>(value)",
            "local partial=f<<number>>local called=t.f<<number,string>>(value)",
        ),
        ("local  x  =  ( y  ::  number )  ::  string", "local x=(y::number)::string"),
        (
            "return 1i, 1_000_000i, 0xffffffffffffffffi, -123i",
            "return 1i,1_000_000i,0xffffffffffffffffi,-123i",
        ),
        (
            "\
export local seen = 0i
export const MAX = 2i
@native
export function run()
return seen
end",
            "export local seen=0i export const MAX=2i@native export function run()return seen end",
        ),
        ("@native @checked function f() end", "@native@checked function f()end"),
        ("@[deprecated()] function f() end", "@[deprecated()]function f()end"),
        (
            "@[native, deprecated({use = 'g', reason = 'old'})] local function f() end",
            "@native@[deprecated{use='g',reason='old'}]local function f()end",
        ),
    ] {
        assert_compacts_to(source, expected, LuaVersion::Luau);
    }
}

#[test]
fn luau_interpolated_strings_keep_their_text_and_holes() {
    for (source, expected) in [
        ("local  x  =  `hello {name}`", "local x=`hello {name}`"),
        ("local  x  =  `plain text`", "local x=`plain text`"),
        ("local  x  =  `a {b} c {d .. e} f`", "local x=`a {b} c {d..e} f`"),
        ("local  x  =  `{ `{inner}` }`", "local x=`{`{inner}`}`"),
    ] {
        assert_compacts_to(source, expected, LuaVersion::Luau);
    }
}

#[test]
fn luau_types_compact_to_minimal_whitespace() {
    for (source, expected) in [
        ("local  x :  number  =  5", "local x:number=5"),
        ("local  x :  number ? ?  =  nil", "local x:number??=nil"),
        ("local  x :  | number | string  =  y", "local x:|number|string=y"),
        ("local  x :  & A & B  =  y", "local x:&A&B=y"),
        ("local  x :  typeof ( y )  =  y", "local x:typeof(y)=y"),
        ("local  x :  'a' | true | nil  =  y", "local x:'a'|true|nil=y"),
        ("type  Foo  =  number", "type Foo=number"),
        (
            "export type  T  =  { read  x :  number , write [ string ] :  boolean }",
            "export type T={read x:number,write[string]:boolean}",
        ),
        ("type  T  =  { read  number }", "type T={read number}"),
        (
            "type  Fn  =  < T ... > ( x :  number , string )  ->  ( T ... )",
            "type Fn=<T...>(x:number,string)->(T...)",
        ),
        ("type  F  =  ( ... number )  ->  ...string", "type F=(...number)->...string"),
        ("function  f < T > ( a :  T )  :  T  return  a  end", "function f<T>(a:T):T return a end"),
        ("function  f ( ... :  T ... )  end", "function f(...:T...)end"),
        ("type  function  Add ( a )  return  a  end", "type function Add(a)return a end"),
        (
            "for  k :  string , v :  number  in  pairs ( t )  do  end",
            "for k:string,v:number in pairs(t)do end",
        ),
        ("for  i :  number  = 1 , 10  do  end", "for i:number=1,10 do end"),
    ] {
        assert_compacts_to(source, expected, LuaVersion::Luau);
    }
}
