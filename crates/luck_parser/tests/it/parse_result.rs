use luck_ast::{Call, FunctionArgs, Visitor};
use luck_parser::parse;
use luck_token::LuaVersion;

#[test]
fn owned_source_moves_into_the_result_without_a_copy() {
    for text in ["return 1", "local = 5"] {
        let mut source = String::with_capacity(256);
        source.push_str(text);
        let pointer = source.as_ptr();
        let capacity = source.capacity();
        for _ in 0u8..3 {
            let parsed = parse(source, LuaVersion::Lua54);
            assert_eq!(parsed.source.as_ptr(), pointer);
            assert_eq!(parsed.source.capacity(), capacity);
            assert_eq!(parsed.source, text);
            assert_eq!(parsed.errors.is_empty(), text == "return 1", "{:?}", parsed.errors);
            source = parsed.source;
        }
    }
}

#[test]
fn declaration_without_values_spans_its_annotation_and_attribute() {
    for (source, version) in [
        ("local x <const>", LuaVersion::Lua54),
        ("local a, b <close>", LuaVersion::Lua54),
        ("global x <const>", LuaVersion::Lua55),
        ("local x: number", LuaVersion::Luau),
    ] {
        let result = parse(source, version);
        assert!(result.errors.is_empty(), "{source}: {:?}", result.errors);
        let statement_span = result.block.stmts[0].span();
        assert_eq!(statement_span.text(source), source, "the statement must span all of {source}");
    }
}

/// The text of each call's parenthesized arguments, and the capacity of the
/// list that holds them.
struct ArgLists<'ast> {
    source: &'ast str,
    lists: Vec<(Vec<&'ast str>, usize)>,
}

impl<'ast> Visitor<'ast> for ArgLists<'ast> {
    fn visit_call(&mut self, call: Call<'ast>) {
        if let FunctionArgs::Parenthesized { args, .. } = call.args {
            let texts = args.iter().map(|arg| arg.span().text(self.source)).collect();
            self.lists.push((texts, args.items.capacity()));
        }
        self.walk_call(call);
    }
}

#[test]
fn an_expression_list_nested_in_another_fills_a_list_of_exactly_its_length() {
    let source = "f(a, g(b, h(c, d, e, i, j), k), l)(m)()";
    let result = parse(source, LuaVersion::Lua54);
    assert!(result.errors.is_empty(), "{:?}", result.errors);
    let mut calls = ArgLists { source, lists: Vec::new() };
    calls.visit_block(&result.block);
    assert_eq!(
        calls.lists,
        [
            (vec!["a", "g(b, h(c, d, e, i, j), k)", "l"], 3),
            (vec!["b", "h(c, d, e, i, j)", "k"], 3),
            (vec!["c", "d", "e", "i", "j"], 5),
            (vec!["m"], 1),
            (vec![], 0),
        ]
    );
}
