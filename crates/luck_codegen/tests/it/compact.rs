use luck_ast::{Block, Expression, LastStatement, MAX_NESTING_DEPTH, Synth, block_nesting_depth};
use luck_token::{BinOp, LuaVersion, UnOp};

use crate::common::assert_compacts_to;

#[test]
fn output_capacity_hint_never_changes_output() {
    for version in LuaVersion::ALL {
        let source =
            format!("--{}\nlocal value = - -1; return value .. 'é'", "comment ".repeat(8192));
        let parsed = luck_parser::parse(&source, version);
        assert!(parsed.errors.is_empty(), "{:?}", parsed.errors);
        let expected = luck_codegen::compact(&parsed.block).unwrap();
        for capacity in [0, 1, expected.len(), expected.len() + 1, source.len()] {
            let output = luck_codegen::compact_with_capacity(&parsed.block, capacity).unwrap();
            assert_eq!(output, expected);
            assert_compacts_to(&output, &expected, version);
        }
    }
    let synth = Synth::new();
    let block = synth.block(Vec::new(), Some(synth.return_(vec![synth.string("source-free")])));
    let expected = luck_codegen::compact(&block);
    for capacity in [0, 1, 1024] {
        assert_eq!(luck_codegen::compact_with_capacity(&block, capacity), expected);
    }
}

#[test]
fn a_capacity_hint_the_allocator_cannot_meet_is_dropped() {
    let synth = Synth::new();
    let block = synth.block(Vec::new(), Some(synth.return_(vec![synth.number("1")])));
    for capacity in [usize::MAX, isize::MAX.unsigned_abs()] {
        let output = luck_codegen::compact_with_capacity(&block, capacity);
        assert_eq!(output.as_deref(), Ok("return 1"), "capacity {capacity}");
    }
}

/// Nests its operand exactly one level deeper.
type Wrap = fn(&Synth, Expression) -> Expression;

/// `return x`, with `wrap` applied to `x` until the block nests `depth` deep.
fn nested_return(depth: usize, wrap: Wrap) -> Block {
    let synth = Synth::new();
    let mut expr = synth.name_expr("x");
    for _ in 1..depth {
        expr = wrap(&synth, expr);
    }
    let block = synth.block(Vec::new(), Some(synth.return_(vec![expr])));
    assert_eq!(block_nesting_depth(&block), depth, "each wrap adds one level");
    block
}

fn single_level_wraps() -> Vec<(&'static str, Wrap)> {
    vec![
        ("parentheses", |synth, expr| synth.parenthesized(expr)),
        ("unary operator", |synth, expr| synth.unary_op(UnOp::Not, expr)),
        ("call argument", |synth, expr| synth.call(synth.name_expr("f"), vec![expr])),
        ("method argument", |synth, expr| synth.method_call(synth.name_expr("o"), "m", vec![expr])),
        ("index", |synth, expr| synth.index(synth.name_expr("t"), expr)),
        ("table", |synth, expr| synth.array(vec![expr])),
    ]
}

#[test]
fn a_tree_at_the_nesting_cap_prints_and_reparses() {
    for (label, wrap) in single_level_wraps() {
        let block = nested_return(MAX_NESTING_DEPTH, wrap);
        let output = luck_codegen::compact(&block).expect(label);
        let reparsed = luck_parser::parse(output, LuaVersion::Lua54);
        assert!(reparsed.errors.is_empty(), "{label}: {:?}", reparsed.errors.first());
        assert_eq!(block_nesting_depth(&reparsed.block), MAX_NESTING_DEPTH, "{label}");
    }
}

#[test]
fn a_tree_past_the_nesting_cap_is_an_error() {
    for (label, wrap) in single_level_wraps() {
        let block = nested_return(MAX_NESTING_DEPTH + 1, wrap);
        let error = luck_codegen::compact(&block).expect_err(label);
        assert!(error.message.contains("maximum nesting depth exceeded"), "{label}: {error:?}");
    }
}

#[test]
fn a_tree_far_past_the_nesting_cap_is_refused_without_walking_down_it() {
    let synth = Synth::new();
    let mut nested = synth.name_expr("x");
    for _ in 0..200_000u32 {
        nested = synth.parenthesized(nested);
    }
    let block = synth.block(Vec::new(), Some(synth.return_(vec![nested])));
    assert_eq!(luck_codegen::compact(&block).ok(), None);

    // Dropped whole, a tree this deep recurses once per level.
    let Some(LastStatement::Return(mut return_stmt)) = block.last_stmt.map(|last| *last) else {
        panic!("the block ends in its return");
    };
    let mut expr = return_stmt.exprs.items.pop().expect("the return has its value");
    while let Expression::Parenthesized(mut parenthesized) = expr {
        expr = parenthesized.expr.take();
    }
}

#[test]
fn a_run_of_any_length_prints_as_one_level() {
    let synth = Synth::new();
    let mut sum = synth.name_expr("x");
    let mut calls = synth.name_expr("f");
    for _ in 0..200_000u32 {
        sum = synth.binary_op(sum, BinOp::Add, synth.number("1"));
    }
    for _ in 0..2_000u32 {
        calls = synth.call(calls, Vec::new());
    }
    let block = synth.block(Vec::new(), Some(synth.return_(vec![sum, calls])));
    assert_eq!(block_nesting_depth(&block), 2);
    let output = luck_codegen::compact(&block).expect("the tree nests two levels deep");
    let reparsed = luck_parser::parse(output.as_str(), LuaVersion::Lua54);
    assert!(reparsed.errors.is_empty(), "{:?}", reparsed.errors.first());
    assert_eq!(block_nesting_depth(&reparsed.block), 2);
    assert_eq!(luck_codegen::compact(&reparsed.block).ok(), Some(output));
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
            "if a then\n  x = 1\n  y = 2\nelseif b then\n  z = 3\nelse\n  w = 4\nend",
            "if a then x=1 y=2 elseif b then z=3 else w=4 end",
        ),
        (
            "if a then x = 1 elseif b then y = 2 else z = 3 end",
            "if a then x=1 elseif b then y=2 else z=3 end",
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
        ("local function f(a, b) return a + b end", "local function f(a,b)return a+b end"),
        ("x  =  function ( )  end", "x=function()end"),
        ("x = function(a) return a end", "x=function(a)return a end"),
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
fn operators_compact_to_minimal_whitespace() {
    for (source, expected, version) in [
        ("x = a + b", "x=a+b", LuaVersion::Lua51),
        ("x = a - b", "x=a-b", LuaVersion::Lua51),
        ("x = a * b", "x=a*b", LuaVersion::Lua51),
        ("x = a / b", "x=a/b", LuaVersion::Lua51),
        ("x = a % b", "x=a%b", LuaVersion::Lua51),
        ("x = a ^ b", "x=a^b", LuaVersion::Lua51),
        ("x = a .. b", "x=a..b", LuaVersion::Lua51),
        ("x = a == b", "x=a==b", LuaVersion::Lua51),
        ("x = a ~= b", "x=a~=b", LuaVersion::Lua51),
        ("x = a < b", "x=a<b", LuaVersion::Lua51),
        ("x = a <= b", "x=a<=b", LuaVersion::Lua51),
        ("x = a > b", "x=a>b", LuaVersion::Lua51),
        ("x = a >= b", "x=a>=b", LuaVersion::Lua51),
        ("x = a and b", "x=a and b", LuaVersion::Lua51),
        ("x = a or b", "x=a or b", LuaVersion::Lua51),
        ("x = -a", "x=-a", LuaVersion::Lua51),
        ("x = not a", "x=not a", LuaVersion::Lua51),
        ("x = #a", "x=#a", LuaVersion::Lua51),
        ("x = - -a", "x=- -a", LuaVersion::Lua51),
        ("x = not not a", "x=not not a", LuaVersion::Lua51),
        // Lua 5.3+
        ("x = a // b", "x=a//b", LuaVersion::Lua53),
        ("x = a & b", "x=a&b", LuaVersion::Lua53),
        ("x = a | b", "x=a|b", LuaVersion::Lua53),
        ("x = a ~ b", "x=a~b", LuaVersion::Lua53),
        ("x = a << b", "x=a<<b", LuaVersion::Lua53),
        ("x = a >> b", "x=a>>b", LuaVersion::Lua53),
        ("x = ~a", "x=~a", LuaVersion::Lua53),
    ] {
        assert_compacts_to(source, expected, version);
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
        ("x = {1, 2, a = 3, [4] = 5}", "x={1,2,a=3,[4]=5}"),
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
        "f()",
        "f()()()",
        "a.b.c:d()()",
        "x={[1]=2,[3]=4}",
        "for i=1,#t do end",
    ] {
        assert_compacts_to(source, source, LuaVersion::Lua51);
    }
}

#[test]
fn version_gated_stmts_compact_to_minimal_whitespace() {
    for (source, expected, version) in [
        ("goto  skip  :: skip ::", "goto skip::skip::", LuaVersion::Lua52),
        (";", ";", LuaVersion::Lua52),
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
