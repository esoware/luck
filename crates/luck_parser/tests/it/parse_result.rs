use core::ptr;

use luck_allocator::Allocator;
use luck_ast::{Expression, FunctionArgs, GetSpan as _, Statement, Visit};
use luck_parser::{parse, parse_with_tokens};
use luck_testgen::{broken_sources, sources_past_the_nesting_cap, with_the_smallest_stack};
use luck_token::{LuaVersion, Span};

/// The parse reads every token once, in order, so what it keeps is the
/// lexer's stream, `Eof` once at its end, however the parse stops, and a
/// parse that doesn't record them keeps none.
#[test]
fn parse_with_tokens_keeps_the_lexed_stream() {
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
            let recorded = parse_with_tokens(&allocator, source.as_bytes(), version);
            let lexed = luck_lexer::lex(source.as_bytes(), version);
            assert_eq!(recorded.tokens, lexed.tokens, "{source:.80}");
            assert_eq!(recorded.comments, lexed.comments, "{source:.80}");
            assert_eq!(parse(&allocator, source.as_bytes(), version).tokens, []);
        }
    });
}

/// Names and literals are slices of the source, not copies of it, and a
/// string's bytes need not be UTF-8.
#[test]
fn the_tree_borrows_its_leaves_from_the_source() {
    let allocator = Allocator::new();
    let source = b"local name = 'caf\xE9' .. 42".to_vec();
    let result = parse(&allocator, &source, LuaVersion::Lua54);
    assert!(result.errors.is_empty(), "{:?}", result.errors);
    assert!(ptr::eq(result.source, source.as_slice()));
    let Some(Statement::LocalAssignment(local)) = result.chunk.block.stmts.first() else {
        panic!("{:?}", result.chunk.block.stmts);
    };
    let Some(Expression::BinaryOp(concat)) = local.exprs.first() else {
        panic!("{:?}", local.exprs);
    };
    let (Expression::StringLiteral(text), Expression::Number(number)) =
        (&concat.first, &concat.links.first().operand)
    else {
        panic!("{concat:?}");
    };
    let body = Span::new(text.span.start + 1, text.span.end - 1);
    for (leaf, span) in [
        (local.names[0].name.text.as_bytes(), local.names[0].name.span),
        (text.raw.expect("a parsed literal").as_bytes(), text.span),
        (text.value.as_bytes(), body),
        (number.raw.expect("a parsed literal").as_bytes(), number.span),
    ] {
        assert!(ptr::eq(leaf, span.text(&source)), "{leaf:?}");
    }
    assert_eq!(text.value, &b"caf\xE9"[..]);
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
        let result = parse(&allocator, source.as_bytes(), version);
        assert!(result.errors.is_empty(), "{source}: {:?}", result.errors);
        let statement_span = result.chunk.block.stmts[0].span();
        assert_eq!(
            statement_span.text(source.as_bytes()),
            source.as_bytes(),
            "the statement must span all of {source}"
        );
    }
}

/// The text of each call's parenthesized arguments, and the capacity of the
/// list that holds them.
struct ArgLists<'ast> {
    source: &'ast [u8],
    lists: Vec<(Vec<&'ast str>, usize)>,
}

impl<'ast> Visit<'ast> for ArgLists<'ast> {
    fn visit_function_args(&mut self, function_args: &'ast FunctionArgs<'ast>) {
        if let FunctionArgs::Parenthesized { args, .. } = function_args {
            let texts = args
                .iter()
                .map(|arg| str::from_utf8(arg.span().text(self.source)).unwrap())
                .collect();
            self.lists.push((texts, args.capacity()));
        }
        self.walk_function_args(function_args);
    }
}

#[test]
fn an_expression_list_nested_in_another_fills_a_list_of_exactly_its_length() {
    let allocator = Allocator::new();
    let source = "f(a, g(b, h(c, d, e, i, j), k), l)(m)()";
    let result = parse(&allocator, source.as_bytes(), LuaVersion::Lua54);
    assert!(result.errors.is_empty(), "{:?}", result.errors);
    let mut calls = ArgLists { source: source.as_bytes(), lists: Vec::new() };
    calls.visit_block(&result.chunk.block);
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
