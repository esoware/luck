use luck_ast::{AttributedName, Call, FunctionArgs, Punctuated, Statement, Type, Visitor};
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

/// The text of each item in every list of names, assignment targets, and
/// union or intersection members, and the capacity of the list.
struct ItemLists<'source> {
    source: &'source str,
    lists: Vec<(Vec<&'source str>, usize)>,
}

impl ItemLists<'_> {
    fn push_names(&mut self, names: &Punctuated<AttributedName>) {
        let source = self.source;
        let texts = names.iter().map(|attributed| attributed.name.span.text(source)).collect();
        self.lists.push((texts, names.items.capacity()));
    }
}

impl<'ast> Visitor<'ast> for ItemLists<'_> {
    fn visit_stmt(&mut self, stmt: &'ast Statement) {
        let source = self.source;
        if let Statement::FunctionDecl(node) = stmt {
            let segments = node.name.names.iter().map(|name| name.span.text(source)).collect();
            self.lists.push((segments, node.name.names.capacity()));
        }
        if let Statement::GenericFor(node) = stmt {
            let names = node.names.iter().map(|param| param.name.span.text(source)).collect();
            self.lists.push((names, node.names.items.capacity()));
        }
        if let Statement::LocalAssignment(node) = stmt {
            self.push_names(&node.names);
        }
        if let Statement::GlobalDeclaration(node) = stmt {
            self.push_names(&node.names);
        }
        if let Statement::Assignment(node) = stmt {
            let targets = node.targets.iter().map(|target| target.span().text(source)).collect();
            self.lists.push((targets, node.targets.items.capacity()));
        }
        self.walk_stmt(stmt);
    }

    fn visit_type(&mut self, type_value: &'ast Type) {
        let member_list = match type_value {
            Type::Union(union) => Some(&union.types),
            Type::Intersection(intersection) => Some(&intersection.types),
            Type::Name { .. }
            | Type::Named(_)
            | Type::Typeof(_)
            | Type::Table(_)
            | Type::Function(_)
            | Type::Optional(_)
            | Type::Parenthesized(_)
            | Type::Pack(_)
            | Type::Variadic(_)
            | Type::Singleton(_)
            | Type::GenericPack(_)
            | Type::Error(_) => None,
        };
        if let Some(members) = member_list {
            let texts = members.iter().map(|member| member.span().text(self.source)).collect();
            self.lists.push((texts, members.items.capacity()));
        }
        self.walk_type(type_value);
    }
}

fn item_lists(source: &str, version: LuaVersion) -> Vec<(Vec<&str>, usize)> {
    let result = parse(source, version);
    assert!(result.errors.is_empty(), "{:?}", result.errors);
    let mut items = ItemLists { source, lists: Vec::new() };
    items.visit_block(&result.block);
    items.lists
}

#[test]
fn a_list_of_names_or_targets_fills_a_list_of_exactly_its_length() {
    let source = "\
function a.b.c() end
function d() end
function e.f:g() end
for h in i do end
for j, k, l in m do
  for n, o in p do end
end
local q <const>, r = 1, 2
local s
t, u.v, w[1] = function() x, y = 1, 2 end, 3, 4
z = 5
";
    assert_eq!(
        item_lists(source, LuaVersion::Lua54),
        [
            (vec!["a", "b", "c"], 3),
            (vec!["d"], 1),
            (vec!["e", "f"], 2),
            (vec!["h"], 1),
            (vec!["j", "k", "l"], 3),
            (vec!["n", "o"], 2),
            (vec!["q", "r"], 2),
            (vec!["s"], 1),
            (vec!["t", "u.v", "w[1]"], 3),
            (vec!["x", "y"], 2),
            (vec!["z"], 1),
        ]
    );
    let globals = "\
global a, b, c
global <const> d, e = 1, 2
";
    assert_eq!(
        item_lists(globals, LuaVersion::Lua55),
        [(vec!["a", "b", "c"], 3), (vec!["d", "e"], 2)]
    );
}

#[test]
fn a_list_nested_in_a_type_fills_a_list_of_exactly_its_length() {
    let source = "\
local a: typeof(function() local b, c = 1, 2 end), d = 1, 2
type T = A | B | (C | D | E) | F
type U = G & H & (I & J)
type V = | K
for x: A | B, y in z do end
";
    assert_eq!(
        item_lists(source, LuaVersion::Luau),
        [
            (vec!["a", "d"], 2),
            (vec!["b", "c"], 2),
            (vec!["A", "B", "(C | D | E)", "F"], 4),
            (vec!["C", "D", "E"], 3),
            (vec!["G", "H", "(I & J)"], 3),
            (vec!["I", "J"], 2),
            (vec!["K"], 1),
            (vec!["x", "y"], 2),
            (vec!["A", "B"], 2),
        ]
    );
}
