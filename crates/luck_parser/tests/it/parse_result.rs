use core::ptr;

use luck_allocator::Allocator;
use luck_ast::{Call, Expression, FunctionArgs, Statement, Visitor};
use luck_parser::{parse, parse_with_tokens};
use luck_testgen::{broken_sources, sources_past_the_nesting_cap, with_the_smallest_stack};
use luck_token::LuaVersion;

/// The parse reads every token once, in order, so what it hands back is the
/// lexer's stream, `Eof` once at its end, however the parse stops.
#[test]
fn parse_with_tokens_hands_back_the_lexed_stream() {
    with_the_smallest_stack(|| {
        let allocator = Allocator::new();
        let handwritten = [
            (LuaVersion::Lua54, String::new()),
            (LuaVersion::Lua54, "local x = \"open".to_owned()),
            (LuaVersion::Lua54, "--[[ never closed".to_owned()),
            (LuaVersion::Luau, "local s = `a{b}c` .. `d`".to_owned()),
            (LuaVersion::Lua51, "x = = 1 end )".to_owned()),
        ];
        for (version, source) in
            handwritten.into_iter().chain(broken_sources()).chain(sources_past_the_nesting_cap())
        {
            let (parse, tokens) = parse_with_tokens(&allocator, &source, version);
            let lexed = luck_lexer::lex(&source, version);
            assert_eq!(tokens, lexed.tokens, "{source:.80}");
            assert_eq!(parse.comments, lexed.comments, "{source:.80}");
        }
    });
}

/// Names and literals are slices of the source, not copies of it.
#[test]
fn the_tree_borrows_its_leaves_from_the_source() {
    let allocator = Allocator::new();
    let source = String::from("local name = 'text' .. 42");
    let result = parse(&allocator, &source, LuaVersion::Lua54);
    assert!(result.errors.is_empty(), "{:?}", result.errors);
    assert!(ptr::eq(result.source, source.as_str()));
    let Some(Statement::LocalAssignment(local)) = result.block.stmts.first() else {
        panic!("{:?}", result.block.stmts);
    };
    let Some(Expression::BinaryOp(concat)) = local.exprs.first() else {
        panic!("{:?}", local.exprs);
    };
    let (Expression::StringLiteral(text), Expression::Number(number)) =
        (&concat.first, &concat.links.first().operand)
    else {
        panic!("{concat:?}");
    };
    for (leaf, span) in [
        (local.names[0].name.text, local.names[0].name.span),
        (text.text, text.span),
        (number.text, number.span),
    ] {
        assert!(ptr::eq(leaf, span.text(&source)), "{leaf}");
    }
}

#[test]
fn declaration_without_values_spans_its_annotation_and_attribute() {
    let allocator = Allocator::new();
    for (source, version) in [
        ("local x <const>", LuaVersion::Lua54),
        ("local a, b <close>", LuaVersion::Lua54),
        ("global x <const>", LuaVersion::Lua55),
        ("local x: number", LuaVersion::Luau),
    ] {
        let result = parse(&allocator, source, version);
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
            self.lists.push((texts, args.capacity()));
        }
        self.walk_call(call);
    }
}

#[test]
fn an_expression_list_nested_in_another_fills_a_list_of_exactly_its_length() {
    let allocator = Allocator::new();
    let source = "f(a, g(b, h(c, d, e, i, j), k), l)(m)()";
    let result = parse(&allocator, source, LuaVersion::Lua54);
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
