use luck_token::LuaVersion;

use crate::common::{roundtrip_compact, v51, v54, v55, verify_roundtrip};

#[test]
fn local_assignment() {
    v51("local  x  =  1", "local x=1");
}

#[test]
fn local_multiple() {
    v51("local  a , b  =  1 , 2", "local a,b=1,2");
}

#[test]
fn local_no_init() {
    v51("local  x", "local x");
}

#[test]
fn assignment() {
    v51("x  =  1", "x=1");
}

#[test]
fn multi_assignment() {
    v51("a , b  =  1 , 2", "a,b=1,2");
}

#[test]
fn if_then_end() {
    v51("if  true  then  end", "if true then end");
}

#[test]
fn if_else() {
    v51(
        "if  x  then  a = 1  else  b = 2  end",
        "if x then a=1 else b=2 end",
    );
}

#[test]
fn if_elseif_else() {
    v51(
        "if a then  elseif b then  else  end",
        "if a then elseif b then else end",
    );
}

#[test]
fn while_loop() {
    v51("while  true  do  end", "while true do end");
}

#[test]
fn repeat_until() {
    v51("repeat  until  false", "repeat until false");
}

#[test]
fn do_end() {
    v51("do  end", "do end");
}

#[test]
fn numeric_for() {
    v51("for  i = 1 , 10  do  end", "for i=1,10 do end");
}

#[test]
fn numeric_for_step() {
    v51("for  i = 1 , 10 , 2  do  end", "for i=1,10,2 do end");
}

#[test]
fn generic_for() {
    v51(
        "for  k , v  in  pairs ( t )  do  end",
        "for k,v in pairs(t)do end",
    );
}

#[test]
fn local_function() {
    v51(
        "local  function  f ( x , y )  return  x  end",
        "local function f(x,y)return x end",
    );
}

#[test]
fn function_decl() {
    v51("function  f ( )  end", "function f()end");
}

#[test]
fn function_dotted_name() {
    v51("function  a . b : c ( )  end", "function a.b:c()end");
}

#[test]
fn anonymous_function() {
    v51("x  =  function ( )  end", "x=function()end");
}

#[test]
fn binary_ops() {
    v51("x  =  a  +  b  *  c", "x=a+b*c");
}

#[test]
fn minus_minus_separation() {
    v51("x  =  a  -  - b", "x=a- -b");
}

#[test]
fn number_dotdot_separation() {
    v51("return  1  ..  2", "return 1 ..2");
}

#[test]
fn concat_string() {
    v51("return  \"a\"  ..  \"b\"", "return\"a\"..\"b\"");
}

#[test]
fn unary_not() {
    v51("x  =  not  y", "x=not y");
}

#[test]
fn unary_hash() {
    v51("x  =  # t", "x=#t");
}

#[test]
fn parenthesized() {
    v51("x  =  ( a  +  b )", "x=(a+b)");
}

#[test]
fn table_simple() {
    v51("x  =  {  1 ,  2 ,  3  }", "x={1,2,3}");
}

#[test]
fn table_named_fields() {
    v51("x  =  {  a  =  1 ,  b  =  2  }", "x={a=1,b=2}");
}

#[test]
fn table_bracketed_key() {
    v51("x  =  {  [ 1 ]  =  2  }", "x={[1]=2}");
}

#[test]
fn table_empty() {
    v51("x  =  {  }", "x={}");
}

#[test]
fn call_parens() {
    v51("f ( 1 , 2 )", "f(1,2)");
}

#[test]
fn call_string() {
    v51("f  \"hello\"", "f\"hello\"");
}

#[test]
fn call_table() {
    v51("f  {  1  }", "f{1}");
}

#[test]
fn method_call() {
    v51("a : b ( 1 )", "a:b(1)");
}

#[test]
fn field_access() {
    v51("x  =  a . b", "x=a.b");
}

#[test]
fn index_access() {
    v51("x  =  a [ 1 ]", "x=a[1]");
}

#[test]
fn return_empty() {
    v51("return", "return");
}

#[test]
fn return_values() {
    v51("return  1 , 2", "return 1,2");
}

#[test]
fn break_statement() {
    v51("while true do  break  end", "while true do break end");
}

#[test]
fn goto_label() {
    verify_roundtrip(
        "goto  skip  :: skip ::",
        "goto skip::skip::",
        LuaVersion::Lua52,
    );
}

#[test]
fn local_with_attribute() {
    v54("local  x  <const>  =  1", "local x<const> =1");
}

#[test]
fn compound_add() {
    verify_roundtrip("x  +=  1", "x+=1", LuaVersion::Luau);
}

#[test]
fn empty_statement() {
    verify_roundtrip("; ;", ";;", LuaVersion::Lua52);
}

#[test]
fn vararg_function() {
    v51(
        "function  f ( ... )  return  ...  end",
        "function f(...)return...end",
    );
}

#[test]
fn vararg_with_params() {
    v51(
        "function  f ( a , ... )  return  ...  end",
        "function f(a,...)return...end",
    );
}

#[test]
fn nested_function_calls() {
    v51("f ( g ( h ( 1 ) ) )", "f(g(h(1)))");
}

#[test]
fn chained_field_access() {
    v51("x  =  a . b . c . d", "x=a.b.c.d");
}

#[test]
fn mixed_access_and_call() {
    v51("x  =  a . b . c [ 2 ]", "x=a.b.c[2]");
}

#[test]
fn nested_tables() {
    v51("x  =  { { 1 } , { 2 } }", "x={{1},{2}}");
}

#[test]
fn complex_expression() {
    v51("x  =  a  +  b  *  ( c  -  d )  /  e", "x=a+b*(c-d)/e");
}

#[test]
fn string_concat_chain() {
    v51("x  =  \"a\"  ..  \"b\"  ..  \"c\"", "x=\"a\"..\"b\"..\"c\"");
}

#[test]
fn multiline_if() {
    v51(
        "if a then\n  x = 1\n  y = 2\nelseif b then\n  z = 3\nelse\n  w = 4\nend",
        "if a then x=1 y=2 elseif b then z=3 else w=4 end",
    );
}

#[test]
fn function_in_table() {
    v51(
        "x  =  {  f  =  function ( a )  return  a  end  }",
        "x={f=function(a)return a end}",
    );
}

fn vluau(source: &str, expected: &str) {
    verify_roundtrip(source, expected, LuaVersion::Luau);
}

#[test]
fn luau_if_expression() {
    vluau(
        "local  x  =  if  a  then  1  else  2",
        "local x=if a then 1 else 2",
    );
}

#[test]
fn luau_interpolated_string() {
    vluau("local  x  =  `hello {name}`", "local x=`hello {name}`");
}

#[test]
fn luau_local_type_annotation() {
    // Annotations are part of the AST now and must survive round-tripping.
    vluau("local  x :  number  =  5", "local x:number=5");
}

#[test]
fn luau_local_optional_annotation() {
    vluau("local  x :  number ?  =  nil", "local x:number?=nil");
}

#[test]
fn luau_local_union_leading_pipe() {
    vluau(
        "local  x :  | number | string  =  y",
        "local x:|number|string=y",
    );
}

#[test]
fn luau_local_intersection() {
    vluau("local  x :  A & B  =  y", "local x:A&B=y");
}

#[test]
fn luau_local_typeof() {
    vluau("local  x :  typeof ( y )  =  y", "local x:typeof(y)=y");
}

#[test]
fn luau_qualified_generic_type() {
    // `> =` keeps a space: glued `>=` would lex as one token and break the
    // generic close on re-parse.
    vluau(
        "local  x :  mod . Name < A , B >  =  y",
        "local x:mod.Name<A,B> =y",
    );
}

#[test]
fn luau_nested_generic_closes() {
    // Two `>` closes are separated by a space so they cannot lex as a
    // single `>>` shift; `>` before `=` is spaced for the same reason.
    vluau(
        "local  x :  Map < string , Array < number > >  =  m",
        "local x:Map<string,Array<number> > =m",
    );
}

#[test]
fn luau_table_type_with_indexer() {
    vluau(
        "type  T  =  { read  x :  number , [ string ] :  boolean }",
        "type T={read x:number,[string]:boolean}",
    );
}

#[test]
fn luau_table_type_array_shorthand() {
    vluau("type  T  =  { number }", "type T={number}");
}

#[test]
fn luau_function_generics_and_annotations() {
    vluau(
        "function  f < T > ( a :  T )  :  T  return  a  end",
        "function f<T>(a:T):T return a end",
    );
}

#[test]
fn luau_function_type_value() {
    vluau(
        "type  Fn  =  ( x :  number , string )  ->  boolean",
        "type Fn=(x:number,string)->boolean",
    );
}

#[test]
fn luau_return_type_pack_with_variadic() {
    vluau(
        "function  f ( )  :  ( number , ... string )  end",
        "function f():(number,...string)end",
    );
}

#[test]
fn luau_type_alias_generics_defaults() {
    // `>` before the alias `=` is spaced to avoid forming `>=`.
    vluau(
        "type  Foo < T , U = string >  =  T",
        "type Foo<T,U=string> =T",
    );
}

#[test]
fn luau_type_function_declaration() {
    vluau(
        "type  function  Add ( a )  return  a  end",
        "type function Add(a)return a end",
    );
}

#[test]
fn luau_cast_chain() {
    vluau(
        "local  x  =  y  ::  number  ::  string",
        "local x=y::number::string",
    );
}

#[test]
fn luau_generic_for_typed_binding() {
    vluau(
        "for  k :  string , v :  number  in  pairs ( t )  do  end",
        "for k:string,v:number in pairs(t)do end",
    );
}

#[test]
fn luau_numeric_for_typed_binding() {
    vluau(
        "for  i :  number  = 1 , 10  do  end",
        "for i:number=1,10 do end",
    );
}

#[test]
fn luau_type_declaration() {
    vluau("type  Foo  =  number", "type Foo=number");
}

#[test]
fn luau_continue() {
    vluau(
        "for  i = 1 , 10  do  continue  end",
        "for i=1,10 do continue end",
    );
}

#[test]
fn lua55_global_declaration() {
    v55("global  x", "global x");
}

#[test]
fn lua55_global_function() {
    v55("global  function  f ( )  end", "global function f()end");
}

#[test]
fn lua55_global_star() {
    v55("global  *", "global*");
}

fn verify_idempotent(source: &str, version: LuaVersion) {
    let first = roundtrip_compact(source, version);
    let second = roundtrip_compact(&first, version);
    assert_eq!(
        first, second,
        "compact printer is not idempotent for {:?}",
        source
    );
}

#[test]
fn idempotency_lua51() {
    let cases = [
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
    ];
    for source in &cases {
        verify_idempotent(source, LuaVersion::Lua51);
    }
}

#[test]
fn idempotency_lua52() {
    verify_idempotent("goto skip ::skip::", LuaVersion::Lua52);
    verify_idempotent(";", LuaVersion::Lua52);
}

#[test]
fn idempotency_lua54() {
    verify_idempotent("local x <const> = 1", LuaVersion::Lua54);
}

#[test]
fn idempotency_luau() {
    let cases = [
        "local x = if a then 1 else 2",
        "x += 1",
        "type Foo = number",
        "for i = 1, 10 do continue end",
    ];
    for source in &cases {
        verify_idempotent(source, LuaVersion::Luau);
    }
}

#[test]
fn idempotency_lua55() {
    let cases = ["global x", "global function f() end", "global *"];
    for source in &cases {
        verify_idempotent(source, LuaVersion::Lua55);
    }
}

fn audit_reparse(source: &str, version: LuaVersion) -> String {
    let compact = roundtrip_compact(source, version);
    let reparsed = luck_parser::parse(&compact, version);
    assert!(
        reparsed.errors.is_empty(),
        "ADJACENCY BUG: {:?} -> {:?} fails to reparse: {:?}",
        source,
        compact,
        reparsed.errors
    );
    compact
}

#[test]
fn adjacency_end_if() {
    let out = audit_reparse("do end\nif true then end", LuaVersion::Lua51);
    assert!(
        out.contains("end if") || out.contains("end;if"),
        "end/if need separation: {:?}",
        out
    );
}

#[test]
fn adjacency_end_call() {
    // `end` followed by `f()` - end is a keyword, f is identifier, needs space
    let out = audit_reparse("do end\nf()", LuaVersion::Lua51);
    assert!(
        !out.contains("endf"),
        "end/f() must have separation: {:?}",
        out
    );
}

#[test]
fn adjacency_then_return() {
    let out = audit_reparse("if true then return 1 end", LuaVersion::Lua51);
    assert!(
        out.contains("then return") || out.contains("then;return"),
        "then/return need separation: {:?}",
        out
    );
}

#[test]
fn adjacency_return_function() {
    let out = audit_reparse("do return function() end end", LuaVersion::Lua51);
    assert!(
        out.contains("return function"),
        "return/function need separation: {:?}",
        out
    );
}

#[test]
fn adjacency_end_do() {
    let out = audit_reparse("do end do end", LuaVersion::Lua51);
    assert!(out.contains("end do"), "end/do need separation: {:?}", out);
}

#[test]
fn adjacency_end_while() {
    let out = audit_reparse(
        "while true do break end\nwhile true do break end",
        LuaVersion::Lua51,
    );
    assert!(
        out.contains("end while"),
        "end/while need separation: {:?}",
        out
    );
}

#[test]
fn adjacency_end_for() {
    let out = audit_reparse("for i=1,1 do end\nfor i=1,1 do end", LuaVersion::Lua51);
    assert!(
        out.contains("end for"),
        "end/for need separation: {:?}",
        out
    );
}

#[test]
fn adjacency_integer_dotdot() {
    let out = audit_reparse("return 1 .. 2", LuaVersion::Lua51);
    assert!(out.contains("1 .."), "1/.. need space: {:?}", out);
}

#[test]
fn adjacency_float_dotdot() {
    let out = audit_reparse("return 1.0 .. 'x'", LuaVersion::Lua51);
    assert!(
        out.contains("1.0 ..") || out.contains("1. .."),
        "float/.. need space: {:?}",
        out
    );
}

#[test]
fn adjacency_dotnum_dotdot() {
    let out = audit_reparse("return .5 .. 'x'", LuaVersion::Lua51);
    assert!(out.contains(".5 .."), ".5/.. need space: {:?}", out);
}

#[test]
fn adjacency_scinotation_dotdot() {
    let out = audit_reparse("return 1e5 .. 'x'", LuaVersion::Lua51);
    assert!(
        out.contains("1e5 ..") || out.contains("100000 .."),
        "1e5/.. need space: {:?}",
        out
    );
}

#[test]
fn adjacency_hex_dotdot() {
    let out = audit_reparse("return 0xff .. 'x'", LuaVersion::Lua51);
    assert!(
        out.contains("0xff ..") || out.contains("ff .."),
        "hex/.. need space: {:?}",
        out
    );
}

#[test]
fn adjacency_number_and() {
    let out = audit_reparse("x = 1 and 2", LuaVersion::Lua51);
    assert!(out.contains("1 and"), "number/and need space: {:?}", out);
}

#[test]
fn adjacency_number_or() {
    let out = audit_reparse("x = 1 or 2", LuaVersion::Lua51);
    assert!(out.contains("1 or"), "number/or need space: {:?}", out);
}

#[test]
fn adjacency_minus_minus() {
    let out = audit_reparse("x = a - -b", LuaVersion::Lua51);
    assert!(
        out.contains("- -") || out.contains("-(-"),
        "minus/minus need separation: {:?}",
        out
    );
}

#[test]
fn adjacency_triple_minus() {
    let out = audit_reparse("x = a - - -b", LuaVersion::Lua51);
    assert!(
        !out.contains("---"),
        "triple minus must not form comment: {:?}",
        out
    );
}

#[test]
fn adjacency_index_no_space() {
    let out = audit_reparse("x = a[1]", LuaVersion::Lua51);
    assert!(
        out.contains("a[1]"),
        "index should have no space: {:?}",
        out
    );
}

#[test]
fn adjacency_slash_slash_no_merge() {
    let out = audit_reparse("x = a / (1/b)", LuaVersion::Lua51);
    assert!(
        !out.contains("//"),
        "adjacent slashes must not form floor-div: {:?}",
        out
    );
}

#[test]
fn adjacency_adjacent_labels() {
    let out = audit_reparse("::label:: ::other::", LuaVersion::Lua52);
    assert!(
        out.contains("::label::::other::") || out.contains("::label:: ::other::"),
        "adjacent labels must reparse: {:?}",
        out
    );
}

#[test]
fn adjacency_return_not() {
    let out = audit_reparse("function f() return not x end", LuaVersion::Lua51);
    assert!(
        out.contains("return not"),
        "return/not need space: {:?}",
        out
    );
}

#[test]
fn adjacency_return_true() {
    let out = audit_reparse("function f() return true end", LuaVersion::Lua51);
    assert!(
        out.contains("return true"),
        "return/true need space: {:?}",
        out
    );
}

#[test]
fn adjacency_return_nil() {
    let out = audit_reparse("function f() return nil end", LuaVersion::Lua51);
    assert!(
        out.contains("return nil"),
        "return/nil need space: {:?}",
        out
    );
}

#[test]
fn adjacency_attribute_close_equal() {
    let out = audit_reparse("local x <const> = 1", LuaVersion::Lua54);
    assert!(
        out.contains("> =") || !out.contains(">=") || out.contains(">;="),
        "attribute close > and = must not merge to >=: {:?}",
        out
    );
}

#[test]
fn adjacency_until_not() {
    let out = audit_reparse("repeat until not done", LuaVersion::Lua51);
    assert!(out.contains("until not"), "until/not need space: {:?}", out);
}

#[test]
fn adjacency_else_return() {
    let out = audit_reparse("if true then return 1 else return 2 end", LuaVersion::Lua51);
    assert!(out.contains("else return"), "else/return: {:?}", out);
}

#[test]
fn adjacency_paren_close_do() {
    // `pairs(t)do` - `)` then `do` - no space needed since `)` is not a word
    let out = audit_reparse("for k,v in pairs(t) do end", LuaVersion::Lua51);
    assert!(out.contains(")do"), "paren/do no space needed: {:?}", out);
}

#[test]
fn adjacency_hash_string() {
    audit_reparse("x = #'hello'", LuaVersion::Lua51);
}

#[test]
fn adjacency_hash_table() {
    audit_reparse("x = #{1,2,3}", LuaVersion::Lua51);
}

#[test]
fn adjacency_hash_paren() {
    audit_reparse("x = #(t)", LuaVersion::Lua51);
}

#[test]
fn adjacency_tilde_tilde() {
    // bitwise not: ~(~x) in Lua 5.3+
    audit_reparse("x = ~(~y)", LuaVersion::Lua53);
}

#[test]
fn adjacency_double_colon_identifier() {
    audit_reparse("::lbl:: x = 1", LuaVersion::Lua52);
}

/// Parse, compact, reparse, compact again - the two compact outputs must be identical.
/// This catches cases where the output parses but with DIFFERENT semantics.
fn verify_semantic_identity(source: &str, version: LuaVersion) {
    let first_compact = roundtrip_compact(source, version);
    let second_compact = roundtrip_compact(&first_compact, version);
    assert_eq!(
        first_compact, second_compact,
        "SEMANTIC BUG: compact output changed on re-compact.\n  input:   {:?}\n  first:   {:?}\n  second:  {:?}",
        source, first_compact, second_compact
    );
}

#[test]
fn semantic_number_concat() {
    // `1 ..2` must not reparse as `1.` followed by `.2` (two numbers)
    verify_semantic_identity("return 1 .. 2", LuaVersion::Lua51);
}

#[test]
fn semantic_minus_minus() {
    // `a- -b` must not reparse as `a--b` (comment)
    verify_semantic_identity("x = a - -b", LuaVersion::Lua51);
}

#[test]
fn semantic_end_paren_call() {
    // After `end`, a `(` starting a new statement must not fuse
    verify_semantic_identity("do end\n;(f)()", LuaVersion::Lua51);
}

#[test]
fn semantic_attribute_equal() {
    // `>` `=` must not become `>=`
    verify_semantic_identity("local x <const> = 1", LuaVersion::Lua54);
}

#[test]
fn semantic_floor_div_vs_two_slashes() {
    verify_semantic_identity("x = a // b", LuaVersion::Lua53);
}

#[test]
fn semantic_chained_calls() {
    verify_semantic_identity("f(x)(g)(y)", LuaVersion::Lua51);
}

#[test]
fn semantic_all_binary_ops() {
    let cases = [
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
    ];
    for source in &cases {
        verify_semantic_identity(source, LuaVersion::Lua51);
    }
}

#[test]
fn semantic_all_binary_ops_53() {
    let cases = [
        "x = a // b",
        "x = a & b",
        "x = a | b",
        "x = a ~ b",
        "x = a << b",
        "x = a >> b",
    ];
    for source in &cases {
        verify_semantic_identity(source, LuaVersion::Lua53);
    }
}

#[test]
fn semantic_all_unary_ops() {
    let cases = ["x = -a", "x = not a", "x = #a"];
    for source in &cases {
        verify_semantic_identity(source, LuaVersion::Lua51);
    }
}

#[test]
fn semantic_unary_bitwise_not_53() {
    verify_semantic_identity("x = ~a", LuaVersion::Lua53);
}

#[test]
fn semantic_nested_unary() {
    verify_semantic_identity("x = - -a", LuaVersion::Lua51);
    verify_semantic_identity("x = not not a", LuaVersion::Lua51);
    verify_semantic_identity("x = ##t", LuaVersion::Lua51);
}

#[test]
fn semantic_complex_nested() {
    let cases = [
        "if not a and not b then return -c .. d end",
        "for i = 1, #t do x = t[i] .. 'suffix' end",
        "local f = function(a, b, ...) return a + b, ... end",
        "x = {a = 1, b = {c = 2}, [3] = function() end}",
        "repeat local x = f() until not x",
    ];
    for source in &cases {
        verify_semantic_identity(source, LuaVersion::Lua51);
    }
}

#[test]
fn exact_output_number_dotdot() {
    // 1.. would be parsed as 1. (float) then . (error), so space is required
    v51("return 1 .. 2", "return 1 ..2");
}

#[test]
fn exact_output_float_dotdot() {
    v51("return 1.0 .. 'x'", "return 1.0 ..'x'");
}

#[test]
fn exact_output_dotnum_dotdot() {
    // `return` is a word, `.5` is a number (word) - needs space between them
    v51("return .5 .. 'x'", "return .5 ..'x'");
}

#[test]
fn exact_output_minus_minus() {
    v51("x = a - -b", "x=a- -b");
}

#[test]
fn exact_output_end_if() {
    v51("do end\nif true then end", "do end if true then end");
}

#[test]
fn exact_output_return_string() {
    // return followed by string literal - return is a word, "hello" starts with quote
    // quote is not a word, so no space needed between return and "
    v51(
        "function f() return 'hello' end",
        "function f()return'hello'end",
    );
}

#[test]
fn exact_output_return_table() {
    v51("function f() return {} end", "function f()return{}end");
}

#[test]
fn exact_output_return_paren() {
    v51("function f() return (1) end", "function f()return(1)end");
}

#[test]
fn exact_output_return_negative() {
    v51("function f() return -1 end", "function f()return-1 end");
}

#[test]
fn exact_output_return_hash() {
    v51("function f() return #t end", "function f()return#t end");
}

#[test]
fn exact_output_attribute_equal() {
    v54("local x <const> = 1", "local x<const> =1");
}

#[test]
fn exact_output_hash_hash() {
    // ## - hash followed by hash. Hash is not a "word", so no space needed
    v51("x = ##t", "x=##t");
}

#[test]
fn exact_output_label_label() {
    verify_roundtrip("::a:: ::b::", "::a::::b::", LuaVersion::Lua52);
}

#[test]
fn exact_output_end_paren() {
    // end followed by ; then (f)() - the semicolon (EmptyStatement) is preserved
    // In Lua 5.1, empty statements aren't supported, so this uses 5.2+
    verify_roundtrip("do end\n;(f)()", "do end;(f)()", LuaVersion::Lua52);
    // In Lua 5.1 (no empty stmts), the parser treats ; differently -
    // the input `do end\n;(f)()` may not parse the same way.
    // Semicolon needed: `do end;(f)()` - without it, `end(f)` would be ambiguous
    verify_roundtrip("do end\n(f)()", "do end;(f)()", LuaVersion::Lua51);
}

#[test]
fn semicolon_preserved_between_calls() {
    // f(x);(g)(y) - the semicolon separates two call statements.
    // Without it, it would be f(x)(g)(y) - a chained call.
    // The semicolon appears as EmptyStatement in the AST.
    verify_roundtrip("f(x);(g)(y)", "f(x);(g)(y)", LuaVersion::Lua52);
}

#[test]
fn semicolon_identity_call_paren() {
    verify_semantic_identity("f(x);(g)(y)", LuaVersion::Lua52);
}

#[test]
fn no_semicolon_chained() {
    // f(x)(g)(y) without semicolon - single chained call
    verify_semantic_identity("f(x)(g)(y)", LuaVersion::Lua51);
}

#[test]
fn exact_output_not_paren() {
    // `not` is a word, `(` is not - no space needed
    v51("x = not(y)", "x=not(y)");
}

#[test]
fn exact_output_not_string() {
    // `not` followed by string literal - word then quote, no space needed
    v51("x = not 'hello'", "x=not'hello'");
}

#[test]
fn exact_output_not_table() {
    v51("x = not {}", "x=not{}");
}

#[test]
fn adjacency_end_left_paren_call() {
    // (f)() after end - this is a valid separate statement
    audit_reparse("do end\n(f)()", LuaVersion::Lua51);
}

// Note: `end"foo"` can't occur in valid ASTs - bare string literals aren't statements,
// and `end` is a keyword that can't be a call prefix.

#[test]
fn adjacency_stress_test() {
    let cases = [
        "local a,b,c=1,2,3",
        "x=a+b-c*d/e%f^g",
        "x=a and b or c",
        "x=not not not true",
        "x=#t+#s",
        "return 1,2,3",
        "return 'a'..'b'..'c'",
        "f()()()",
        "a.b.c:d()()",
        "x={[1]=2,[3]=4}",
        "for i=1,#t do end",
    ];
    for source in &cases {
        audit_reparse(source, LuaVersion::Lua51);
    }
}

#[test]
fn semicolon_after_number_before_paren_call() {
    // `x = 1\n(f)()` - two statements; without semicolon, `1(f)` looks like a call
    v51("x = 1\n(f)()", "x=1;(f)()");
}

#[test]
fn bug_dotdot_before_dot_number() {
    // ".." followed by ".5" must not produce "...5" (parsed as vararg + 5)
    audit_reparse(r#"x = "a" .. .5"#, LuaVersion::Lua51);
}

#[test]
fn bug_dotdot_before_dot_number_semantic() {
    verify_semantic_identity(r#"x = "a" .. .5"#, LuaVersion::Lua51);
}

#[test]
fn exact_output_dotdot_dot_number() {
    // ".." followed by ".5" - needs space to prevent "...5"
    v51(r#"x = "a" .. .5"#, r#"x="a".. .5"#);
}

#[test]
fn bug_number_dotdot_dot_number() {
    // number .. .5 - both number-before-dotdot and dotdot-before-dotnumber apply
    audit_reparse("x = 1 .. .5", LuaVersion::Lua51);
}

#[test]
fn semantic_number_dotdot_dot_number() {
    verify_semantic_identity("x = 1 .. .5", LuaVersion::Lua51);
}

#[test]
fn exact_output_number_dotdot_dot_number() {
    v51("x = 1 .. .5", "x=1 .. .5");
}

#[test]
fn audit_bracket_long_string_key() {
    // {[ [[key]] ] = 1} - bracket field with long string key
    audit_reparse("x = {[ [[key]] ] = 1}", LuaVersion::Lua51);
}

#[test]
fn semantic_bracket_long_string_key() {
    verify_semantic_identity("x = {[ [[key]] ] = 1}", LuaVersion::Lua51);
}
