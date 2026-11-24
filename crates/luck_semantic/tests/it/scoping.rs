use luck_allocator::Allocator;
use luck_ast::{AstKind, Name};
use luck_semantic::{
    LabelId, Reference, ReferenceKind, ScopeKind, Scoping, Semantic, SemanticBuilder, Symbol,
    SymbolId, SymbolKind,
};
use luck_token::{LuaVersion, Span};

use crate::common::{NAMES_OF_EVERY_KIND, parse, semantic_for};

fn scoping_in(allocator: &Allocator, source: &str, version: LuaVersion) -> Scoping {
    SemanticBuilder::new().build(parse(allocator, source, version)).semantic.into_scoping()
}

fn scoping_of(allocator: &Allocator, source: &str) -> Scoping {
    scoping_in(allocator, source, LuaVersion::Lua54)
}

fn symbol<'scoping>(scoping: &'scoping Scoping, name: &str) -> &'scoping Symbol {
    scoping
        .symbols()
        .iter()
        .find(|symbol| scoping.name(symbol.name) == name)
        .unwrap_or_else(|| panic!("{name} must be declared"))
}

fn references<'scoping>(scoping: &'scoping Scoping, name: &str) -> Vec<&'scoping Reference> {
    scoping.references().iter().filter(|reference| scoping.name(reference.name) == name).collect()
}

fn global_names(scoping: &Scoping) -> Vec<&str> {
    scoping.global_references().map(|reference| scoping.name(reference.name)).collect()
}

fn is_assigned_global(scoping: &Scoping, name: &str) -> bool {
    scoping.name_id(name).is_some_and(|id| scoping.is_assigned_global(id))
}

fn is_declared_global(scoping: &Scoping, name: &str) -> bool {
    scoping.name_id(name).is_some_and(|id| scoping.is_declared_global(id))
}

/// Every name node of `semantic`'s tree whose text is `text`, in source
/// order.
fn names_of<'ast>(semantic: &Semantic<'ast>, text: &str) -> Vec<&'ast Name<'ast>> {
    semantic
        .nodes()
        .iter()
        .filter_map(|node| node.kind().as_name())
        .filter(|name| name.text == text)
        .collect()
}

#[test]
fn local_declares_a_symbol() {
    let allocator = Allocator::new();
    let scoping = scoping_of(&allocator, "local x = 1");
    assert_eq!(scoping.symbols().len(), 1);
    assert_eq!(scoping.name(scoping.symbols()[0].name), "x");
}

#[test]
fn reference_resolves_to_local() {
    let allocator = Allocator::new();
    let scoping = scoping_of(
        &allocator,
        "\
local x = 1
print(x)",
    );
    let x_references = references(&scoping, "x");
    assert_eq!(x_references.len(), 1);
    assert_eq!(x_references[0].resolved, Some(symbol(&scoping, "x").id));
}

#[test]
fn global_reference_is_unresolved() {
    let allocator = Allocator::new();
    let scoping = scoping_of(&allocator, "print(hello)");
    let names = global_names(&scoping);
    assert!(names.contains(&"print"));
    assert!(names.contains(&"hello"));
}

#[test]
fn function_body_opens_the_scope_of_its_parameters() {
    let allocator = Allocator::new();
    let scoping = scoping_of(&allocator, "local function f(a, b) return a + b end");
    assert_eq!(scoping.scopes().len(), 2);
    assert_eq!(scoping.scopes()[1].kind, ScopeKind::Function);
    assert_eq!(scoping.symbols().len(), 3);
    assert_eq!(symbol(&scoping, "a").scope.index(), 1);
}

#[test]
fn local_read_from_nested_function_is_upvalue() {
    let allocator = Allocator::new();
    let scoping = scoping_of(
        &allocator,
        "\
local x = 1
local function f() return x end",
    );
    let x_symbol = &scoping.symbols()[0];
    assert_eq!(scoping.name(x_symbol.name), "x");
    assert!(x_symbol.is_upvalue);
}

#[test]
fn local_read_two_functions_deep_is_upvalue() {
    let allocator = Allocator::new();
    let scoping = scoping_of(
        &allocator,
        "\
local x = 1
local function outer()
  local function inner() return x end
end",
    );
    assert!(
        symbol(&scoping, "x").is_upvalue,
        "x should be detected as upvalue when captured by nested inner function"
    );
}

#[test]
fn local_read_from_a_nested_block_is_no_upvalue() {
    let allocator = Allocator::new();
    let scoping = scoping_of(
        &allocator,
        "\
local function f(a)
  do while a do print(a) end end
end",
    );
    assert!(!symbol(&scoping, "a").is_upvalue);
}

#[test]
fn inner_local_shadows_outer_local() {
    let allocator = Allocator::new();
    let scoping = scoping_of(
        &allocator,
        "\
local x = 1
do
  local x = 2
end",
    );
    assert_eq!(scoping.symbols().len(), 2);
    let inner_x = &scoping.symbols()[1];
    assert_eq!(scoping.name(inner_x.name), "x");
    assert_eq!(inner_x.shadows, Some(scoping.symbols()[0].id));
}

#[test]
fn redeclaring_a_local_in_one_scope_restores_the_outer_one_on_exit() {
    let allocator = Allocator::new();
    let scoping = scoping_of(
        &allocator,
        "\
local x = 1
do
  local x = 2
  local x = 3
end
print(x)",
    );
    let last_reference = *references(&scoping, "x").last().expect("print(x) references x");
    assert_eq!(last_reference.resolved, Some(scoping.symbols()[0].id));
    assert_eq!(scoping.symbols()[2].shadows, Some(scoping.symbols()[1].id));
}

#[test]
fn numeric_for_declares_loop_variable() {
    let allocator = Allocator::new();
    let scoping = scoping_of(&allocator, "for i = 1, 10 do print(i) end");
    assert_eq!(symbol(&scoping, "i").kind, SymbolKind::NumericForVariable);
}

#[test]
fn generic_for_declares_iterator_variables() {
    let allocator = Allocator::new();
    let scoping = scoping_of(&allocator, "for k, v in pairs(t) do print(k, v) end");
    let iterator_variable_count = scoping
        .symbols()
        .iter()
        .filter(|symbol| symbol.kind == SymbolKind::IteratorVariable)
        .count();
    assert_eq!(iterator_variable_count, 2);
}

#[test]
fn numeric_for_variable_not_visible_after_loop() {
    let allocator = Allocator::new();
    let scoping = scoping_of(
        &allocator,
        "\
for i = 1, 10 do end
print(i)",
    );
    assert!(
        references(&scoping, "i").iter().any(|reference| reference.resolved.is_none()),
        "for-loop variable i should not be visible outside the loop"
    );
}

#[test]
fn generic_for_variables_not_visible_after_loop() {
    let allocator = Allocator::new();
    let scoping = scoping_of(
        &allocator,
        "\
for k, v in pairs(t) do end
print(k)",
    );
    assert!(
        references(&scoping, "k").iter().any(|reference| reference.resolved.is_none()),
        "generic for variables should not be visible outside the loop"
    );
}

#[test]
fn named_vararg_declares_symbol() {
    // Lua 5.5: `...name` binds the vararg table to a name.
    let allocator = Allocator::new();
    let scoping =
        scoping_in(&allocator, "local function f(...args) return args end", LuaVersion::Lua55);
    let args_symbol = symbol(&scoping, "args");
    assert_eq!(args_symbol.kind, SymbolKind::Parameter);
    let args_references = references(&scoping, "args");
    assert_eq!(args_references.len(), 1);
    assert!(
        args_references[0].resolved == Some(args_symbol.id),
        "reference must resolve to the vararg binding"
    );
}

#[test]
fn dot_function_declares_no_self() {
    let allocator = Allocator::new();
    let scoping = scoping_of(
        &allocator,
        "\
local t = {}
function t.method() end",
    );
    assert!(
        scoping.name_id("self").is_none(),
        "self should not be declared in non-method function"
    );
}

#[test]
fn repeat_until_condition_sees_block_locals() {
    let allocator = Allocator::new();
    let scoping = scoping_of(
        &allocator,
        "\
repeat
  local done = true
until done",
    );
    let done_references = references(&scoping, "done");
    assert_eq!(done_references.len(), 1);
    assert!(
        done_references[0].resolved.is_some(),
        "repeat-until condition should resolve locals from the loop body"
    );
}

#[test]
fn global_function_body_captures_enclosing_local() {
    let allocator = Allocator::new();
    let scoping = scoping_in(
        &allocator,
        "\
local x = 1
global function f() return x end",
        LuaVersion::Lua55,
    );
    assert!(symbol(&scoping, "x").is_upvalue);
}

#[test]
fn global_stmts_record_their_declared_names() {
    let allocator = Allocator::new();
    let scoping = scoping_in(
        &allocator,
        "\
global a, b <const> = 1, 2
global c
global function d() end
",
        LuaVersion::Lua55,
    );
    for name in ["a", "b", "c", "d"] {
        assert!(is_declared_global(&scoping, name), "{name}");
    }
}

#[test]
fn declared_globals_record_the_scope_each_declaration_is_in() {
    let allocator = Allocator::new();
    let scoping = scoping_in(
        &allocator,
        "global a do global b, c end local function f() global function d() end end",
        LuaVersion::Lua55,
    );
    let declared: Vec<(&str, usize)> = scoping
        .declared_globals()
        .iter()
        .map(|&(name, scope)| (scoping.name(name), scope.index()))
        .collect();
    assert_eq!(declared, [("a", 0), ("b", 1), ("c", 1), ("d", 2)]);
}

#[test]
fn local_env_makes_free_names_its_fields() {
    let allocator = Allocator::new();
    let scoping = scoping_of(
        &allocator,
        "\
local _ENV = {}
x = 1
print(y)",
    );
    let env_id = symbol(&scoping, "_ENV").id;
    assert_eq!(references(&scoping, "y")[0].env, Some(env_id));
    assert_eq!(scoping.global_references().count(), 0);
    assert!(!is_assigned_global(&scoping, "x"), "`x = 1` writes the local table");
}

#[test]
fn local_env_is_an_ordinary_local_before_52() {
    for version in [LuaVersion::Lua51, LuaVersion::Luau] {
        let allocator = Allocator::new();
        let scoping = scoping_in(
            &allocator,
            "\
local _ENV = {}
print(y)",
            version,
        );
        assert_eq!(references(&scoping, "y")[0].env, None, "{version:?}");
        assert_eq!(scoping.global_references().count(), 2, "{version:?}");
        assert!(!scoping.is_env_binding(symbol(&scoping, "_ENV").id), "{version:?}");
    }
}

#[test]
fn local_env_stops_redirecting_free_names_with_its_block() {
    let allocator = Allocator::new();
    let scoping = scoping_of(
        &allocator,
        "\
do local _ENV = {} end
print(y)",
    );
    assert_eq!(references(&scoping, "y")[0].env, None);
}

#[test]
fn assigning_the_chunk_env_redirects_every_free_name() {
    let allocator = Allocator::new();
    let scoping = scoping_of(
        &allocator,
        "\
_ENV = {}
print(y)",
    );
    assert!(scoping.is_env_assigned());
    assert_eq!(scoping.global_references().count(), 0);

    let lua51 = scoping_in(
        &allocator,
        "\
_ENV = {}
print(y)",
        LuaVersion::Lua51,
    );
    assert!(!lua51.is_env_assigned());
    assert_eq!(lua51.global_references().count(), 3);
}

#[test]
fn assigned_globals_records_free_name_writes_only() {
    let allocator = Allocator::new();
    let scoping = scoping_of(
        &allocator,
        "\
x = 1
function f() end
function t.m() end
local y
y = 2",
    );
    assert!(is_assigned_global(&scoping, "x"));
    assert!(is_assigned_global(&scoping, "f"));
    assert!(!is_assigned_global(&scoping, "t"), "`t.m` only reads `t`");
    assert!(!is_assigned_global(&scoping, "y"), "`y` is a local");
}

#[test]
fn global_declaration_ends_with_its_block() {
    let allocator = Allocator::new();
    let scoping = scoping_in(
        &allocator,
        "\
local x = 1
do global x end
print(x)",
        LuaVersion::Lua55,
    );
    let last_reference = *references(&scoping, "x").last().expect("print(x) references x");
    assert_eq!(last_reference.resolved, Some(symbol(&scoping, "x").id));
}

#[test]
fn global_declaration_initializer_reads_the_enclosing_local() {
    let allocator = Allocator::new();
    let scoping = scoping_in(
        &allocator,
        "\
local x = 1
global x = x
print(x)",
        LuaVersion::Lua55,
    );
    let x_references = references(&scoping, "x");
    assert_eq!(x_references[0].resolved, Some(symbol(&scoping, "x").id));
    assert_eq!(x_references[1].resolved, None);
    assert!(is_assigned_global(&scoping, "x"));
}

#[test]
fn declared_global_without_initializer_is_not_assigned() {
    let allocator = Allocator::new();
    let scoping = scoping_in(
        &allocator,
        "\
global x
global *
print(y)",
        LuaVersion::Lua55,
    );
    assert!(["x", "y", "print"].iter().all(|name| !is_assigned_global(&scoping, name)));
}

#[test]
fn global_star_covers_free_names_until_its_block_ends() {
    let allocator = Allocator::new();
    let scoping = scoping_in(
        &allocator,
        "\
do global *
a = 1 end
b = 2",
        LuaVersion::Lua55,
    );
    assert!(references(&scoping, "a")[0].is_declared_global);
    assert!(!references(&scoping, "b")[0].is_declared_global);
}

#[test]
fn global_star_does_not_void_enclosing_locals() {
    let allocator = Allocator::new();
    let scoping = scoping_in(
        &allocator,
        "\
local x = 1
global *
print(x)",
        LuaVersion::Lua55,
    );
    assert_eq!(references(&scoping, "x")[0].resolved, Some(symbol(&scoping, "x").id));
}

#[test]
fn explicit_self_stays_a_parameter() {
    let allocator = Allocator::new();
    let scoping = scoping_of(
        &allocator,
        "\
local t = {}
function t.m(self) return self end",
    );
    assert_eq!(symbol(&scoping, "self").kind, SymbolKind::Parameter);
}

#[test]
fn method_self_is_implicit_with_an_empty_span() {
    let allocator = Allocator::new();
    let scoping = scoping_of(
        &allocator,
        "\
local t = {}
function t:m() return self end",
    );
    let self_symbol = symbol(&scoping, "self");
    assert_eq!(self_symbol.kind, SymbolKind::ImplicitSelf);
    assert_eq!(self_symbol.definition_span.start, self_symbol.definition_span.end);
}

#[test]
fn type_function_body_sees_a_later_type() {
    let allocator = Allocator::new();
    let scoping = scoping_in(
        &allocator,
        "\
type function f(t) return g(t) end
type function g(t) return t end",
        LuaVersion::Luau,
    );
    assert_eq!(references(&scoping, "g").len(), 0);
}

#[test]
fn type_runtime_names_are_only_known_in_type_functions() {
    let allocator = Allocator::new();
    let scoping = scoping_in(
        &allocator,
        "\
type T = number
print(types, T)",
        LuaVersion::Luau,
    );
    assert_eq!(scoping.global_references().count(), 3);
}

#[test]
fn references_within_lists_a_span_s_names_in_source_order() {
    let allocator = Allocator::new();
    let source = "\
local x = 1
t[x], x = y, function() z = x end
print(x)";
    let scoping = scoping_of(&allocator, source);
    let line = Span::new(12, 45);
    let names: Vec<&str> = scoping
        .references_within(line)
        .map(|id| scoping.name(scoping.reference(id).name))
        .collect();
    assert_eq!(names, ["t", "x", "x", "y", "z", "x"]);
    assert_eq!(scoping.references_within(Span::new(13, 15)).count(), 1, "only `x`, not `t`");
}

#[test]
fn assignment_values_are_read_before_targets_are_written() {
    let allocator = Allocator::new();
    let scoping = scoping_of(
        &allocator,
        "\
local x = 1
x = x + 1",
    );
    let kinds: Vec<ReferenceKind> = scoping
        .resolved_references(symbol(&scoping, "x").id)
        .iter()
        .map(|&id| scoping.reference(id).kind)
        .collect();
    assert_eq!(kinds, [ReferenceKind::Read, ReferenceKind::Write]);
}

#[test]
fn resolved_references_lists_each_symbols_own_in_order() {
    let allocator = Allocator::new();
    let scoping = scoping_of(
        &allocator,
        "\
local a, b = 1, 2
print(b, a, b)
local c",
    );
    let spans = |name: &str| -> Vec<u32> {
        scoping
            .resolved_references(symbol(&scoping, name).id)
            .iter()
            .map(|&id| scoping.reference(id).span.start)
            .collect()
    };
    assert_eq!(spans("a"), [27]);
    assert_eq!(spans("b"), [24, 30]);
    assert_eq!(spans("c"), Vec::<u32>::new());
}

#[test]
fn type_alias_typeof_reads_enclosing_local() {
    let allocator = Allocator::new();
    let scoping = scoping_in(
        &allocator,
        "\
local x = 1
type T = typeof(x)",
        LuaVersion::Luau,
    );
    let x_references = references(&scoping, "x");
    assert_eq!(x_references.len(), 1);
    assert_eq!(x_references[0].resolved, Some(symbol(&scoping, "x").id));
}

#[test]
fn implicit_self_span_does_not_cover_the_method_body() {
    let allocator = Allocator::new();
    let scoping = scoping_of(
        &allocator,
        "\
local t = {}
function t:m()
  local x = 1
  return x
end",
    );
    let self_span = symbol(&scoping, "self").definition_span;
    let x_span = symbol(&scoping, "x").definition_span;
    assert!(
        self_span.end <= x_span.start || x_span.end <= self_span.start,
        "implicit self must not claim the span of the declarations in its method body, \
         or a cursor on any of them resolves to self: self {self_span:?}, x {x_span:?}"
    );
}

#[test]
fn global_declaration_shadows_enclosing_local() {
    let allocator = Allocator::new();
    let scoping = scoping_in(
        &allocator,
        "\
local x = 1
global print, x
print(x)",
        LuaVersion::Lua55,
    );
    assert_eq!(
        references(&scoping, "x")[0].resolved,
        None,
        "after `global x`, the name x refers to the global and voids the enclosing local"
    );
}

#[test]
fn global_function_shadows_enclosing_local() {
    let allocator = Allocator::new();
    let scoping = scoping_in(
        &allocator,
        "\
local f = 1
global function f() end
f()",
        LuaVersion::Lua55,
    );
    let last_reference = *references(&scoping, "f").last().expect("f() references f");
    assert_eq!(
        last_reference.resolved, None,
        "after `global function f`, the name f refers to the global and voids the enclosing local"
    );
}

#[test]
fn global_function_body_refers_to_the_global() {
    let allocator = Allocator::new();
    let scoping = scoping_in(
        &allocator,
        "\
local f = 1
global function f() return f end",
        LuaVersion::Lua55,
    );
    assert!(
        !symbol(&scoping, "f").is_upvalue,
        "the body of `global function f` sees the global f, so it captures no local f"
    );
}

#[test]
fn param_annotation_typeof_does_not_see_parameters() {
    let allocator = Allocator::new();
    let scoping = scoping_in(
        &allocator,
        "\
local a = 1
local function f(a: number, b: typeof(a)) return b end",
        LuaVersion::Luau,
    );
    assert_eq!(
        references(&scoping, "a")[0].resolved,
        Some(symbol(&scoping, "a").id),
        "Luau parses parameter annotations before binding any parameter, so typeof(a) \
         names the outer local"
    );
}

#[test]
fn return_annotation_typeof_does_not_see_parameters() {
    let allocator = Allocator::new();
    let scoping = scoping_in(
        &allocator,
        "\
local a = 1
local function f(a: number): typeof(a) return a end",
        LuaVersion::Luau,
    );
    assert_eq!(
        references(&scoping, "a")[0].resolved,
        Some(symbol(&scoping, "a").id),
        "Luau parses the return annotation before binding the parameters, so typeof(a) \
         names the outer local"
    );
}

#[test]
fn local_annotation_typeof_does_not_see_sibling_name() {
    let allocator = Allocator::new();
    let scoping = scoping_in(
        &allocator,
        "\
local a = 1
do
  local a, b: typeof(a) = 2, 3
  print(a, b)
end",
        LuaVersion::Luau,
    );
    assert_eq!(
        references(&scoping, "a")[0].resolved,
        Some(symbol(&scoping, "a").id),
        "the names of a local statement bind after the statement, so typeof(a) in its \
         annotation names the outer local"
    );
}

#[test]
fn numeric_for_annotation_typeof_does_not_see_loop_variable() {
    let allocator = Allocator::new();
    let scoping = scoping_in(
        &allocator,
        "\
local i = 1
for i: typeof(i) = 1, 2 do print(i) end",
        LuaVersion::Luau,
    );
    assert_eq!(
        references(&scoping, "i")[0].resolved,
        Some(symbol(&scoping, "i").id),
        "the loop variable binds after the loop header, so typeof(i) in its annotation \
         names the outer local"
    );
}

#[test]
fn generic_for_annotation_typeof_does_not_see_loop_variables() {
    let allocator = Allocator::new();
    let scoping = scoping_in(
        &allocator,
        "\
local k = 1
for k, v: typeof(k) in pairs({}) do print(k, v) end",
        LuaVersion::Luau,
    );
    assert_eq!(
        references(&scoping, "k")[0].resolved,
        Some(symbol(&scoping, "k").id),
        "the loop variables bind after the loop header, so typeof(k) in an annotation \
         names the outer local"
    );
}

#[test]
fn local_function_annotation_typeof_does_not_see_its_name() {
    let allocator = Allocator::new();
    let scoping = scoping_in(
        &allocator,
        "\
local g = 1
local function g(z: typeof(g)) return z end",
        LuaVersion::Luau,
    );
    assert_eq!(
        references(&scoping, "g")[0].resolved,
        Some(symbol(&scoping, "g").id),
        "Luau binds a local function's name after parsing its signature, so typeof(g) \
         in a parameter annotation names the outer local"
    );
}

#[test]
fn every_name_finds_what_it_declares_refers_to_or_labels() {
    let allocator = Allocator::new();
    let semantic = semantic_for(&allocator, NAMES_OF_EVERY_KIND, LuaVersion::Lua54);
    let scoping = semantic.scoping();
    let mut found = vec![0usize; scoping.references().len()];
    let mut declared = Vec::new();
    for node in semantic.nodes().iter() {
        let Some(name) = node.kind().as_name() else {
            continue;
        };
        if let Some(id) = scoping.reference_of(name) {
            let reference = scoping.reference(id);
            assert_eq!((reference.node, reference.span), (node.id(), name.span), "{name:?}");
            found[id.index()] += 1;
        }
        if let Some(id) = scoping.symbol_of(name) {
            assert_eq!(scoping.symbol(id).definition_span, name.span, "{name:?}");
            declared.push(id);
        }
        if let Some(id) = scoping.label_of(name) {
            assert_eq!(scoping.name(scoping.label(id).name), name.text.as_str());
        }
    }
    assert!(found.iter().all(|&count| count == 1), "{found:?}");
    let spelled: Vec<SymbolId> = scoping
        .symbols()
        .iter()
        .filter(|symbol| !symbol.kind.is_implicit())
        .map(|symbol| symbol.id)
        .collect();
    assert_eq!(declared, spelled);
    let goto = names_of(&semantic, "done")[0];
    assert_eq!(scoping.label_of(goto).map(LabelId::index), Some(0));
}

#[test]
fn resolves_to_local_reads_the_name_s_own_reference() {
    let allocator = Allocator::new();
    let semantic = semantic_for(
        &allocator,
        "\
local x = 1
print(x)
print(y)",
        LuaVersion::Lua54,
    );
    let scoping = semantic.scoping();
    let [declared, read] = names_of(&semantic, "x")[..] else {
        panic!("the source names x twice");
    };
    assert!(!scoping.resolves_to_local(declared), "a declaration makes no reference");
    assert!(scoping.resolves_to_local(read));
    assert!(!scoping.resolves_to_local(names_of(&semantic, "y")[0]));
    assert!(scoping.reaches_host_global(names_of(&semantic, "print")[0]));
}

#[test]
fn lua51_binds_arg_in_every_vararg_function() {
    let allocator = Allocator::new();
    let source = "\
local arg = 1
local function f(a, ...) return arg end
local function g() return arg end
return arg";
    let semantic = semantic_for(&allocator, source, LuaVersion::Lua51);
    let scoping = semantic.scoping();
    let kinds: Vec<(&str, SymbolKind)> =
        scoping.symbols().iter().map(|symbol| (scoping.name(symbol.name), symbol.kind)).collect();
    assert_eq!(
        kinds,
        [
            ("arg", SymbolKind::Local),
            ("f", SymbolKind::FunctionName),
            ("a", SymbolKind::Parameter),
            ("arg", SymbolKind::ImplicitArg),
            ("g", SymbolKind::FunctionName),
        ]
    );
    let resolved: Vec<Option<usize>> = references(scoping, "arg")
        .iter()
        .map(|reference| reference.resolved.map(SymbolId::index))
        .collect();
    assert_eq!(resolved, [Some(3), Some(0), Some(0)]);
    let implicit = &scoping.symbols()[3];
    let dots = u32::try_from(source.find("...").unwrap()).unwrap();
    assert_eq!(implicit.definition_span, Span::new(dots, dots));
    assert!(matches!(semantic.nodes().kind(implicit.declaration), AstKind::VarArgParam(_)));
    for version in [LuaVersion::Lua52, LuaVersion::Luau] {
        let later = scoping_in(&allocator, source, version);
        assert!(
            references(&later, "arg")
                .iter()
                .all(|reference| reference.resolved.map(SymbolId::index) == Some(0)),
            "{version:?}"
        );
    }
}

#[test]
fn gotos_reach_the_innermost_visible_label() {
    let allocator = Allocator::new();
    let semantic = semantic_for(
        &allocator,
        "\
::a:: ::unused::
do goto a ::a:: goto a end
goto a
local function f() ::a:: goto a end",
        LuaVersion::Lua52,
    );
    let scoping = semantic.scoping();
    let targets: Vec<Option<usize>> = names_of(&semantic, "a")
        .into_iter()
        .map(|name| scoping.label_of(name).map(LabelId::index))
        .collect();
    assert_eq!(
        targets,
        [Some(0), Some(2), Some(2), Some(2), Some(0), Some(3), Some(3)],
        "a label names itself, and each goto takes the innermost label of its function"
    );
    let used: Vec<bool> = scoping.labels().iter().map(|label| label.is_used).collect();
    assert_eq!(used, [true, false, true, true]);
    assert_eq!(scoping.labels()[1].scope.index(), 0);
    assert_eq!(scoping.labels()[2].scope.index(), 1);
}

#[test]
fn a_function_records_whether_its_own_code_reads_its_varargs() {
    let allocator = Allocator::new();
    let scoping = scoping_of(
        &allocator,
        "\
local function f(...) return function(...) return ... end end
local function g(...) if ... then end end
print(...)",
    );
    let reads: Vec<bool> = scoping.scopes().iter().map(|scope| scope.has_vararg_read).collect();
    assert_eq!(reads, [true, false, true, true, false], "the chunk, f, its closure, g, g's if");
}

#[test]
fn a_typeof_in_a_nested_function_captures_nothing() {
    let allocator = Allocator::new();
    let scoping = scoping_in(
        &allocator,
        "\
local x = 1
local function f(y: typeof(x)) local z: typeof(x) = y end",
        LuaVersion::Luau,
    );
    assert_eq!(references(&scoping, "x").len(), 2);
    assert!(!symbol(&scoping, "x").is_upvalue, "types never run");
}

#[test]
fn a_reference_in_a_type_is_in_a_type() {
    let allocator = Allocator::new();
    let scoping = scoping_in(
        &allocator,
        "local x = 1 local y: typeof(x) = x return function(): typeof(x) end",
        LuaVersion::Luau,
    );
    let in_type: Vec<bool> =
        references(&scoping, "x").iter().map(|reference| reference.is_in_type).collect();
    assert_eq!(in_type, [true, false, true]);
}

#[test]
fn a_local_function_s_reads_of_itself_in_its_body_are_recursive() {
    let allocator = Allocator::new();
    let scoping = scoping_of(
        &allocator,
        "\
local function f() return f, function() return f end end
local g = function() return g end
return f",
    );
    let recursive = |name: &str| -> Vec<bool> {
        references(&scoping, name).iter().map(|reference| reference.is_recursive).collect()
    };
    assert_eq!(recursive("f"), [true, true, false]);
    assert_eq!(recursive("g"), [false], "the `g` in its value is a global");
}

#[test]
fn retaining_resolved_references_keeps_the_rest_in_order() {
    fn starts(scoping: &Scoping, symbol: SymbolId) -> Vec<u32> {
        let ids = scoping.resolved_references(symbol);
        ids.iter().map(|&id| scoping.reference(id).span.start).collect()
    }
    let allocator = Allocator::new();
    let mut scoping = scoping_of(
        &allocator,
        "\
local a, b = 1, 2
print(a, b, a, b, a)",
    );
    let (a, b) = (symbol(&scoping, "a").id, symbol(&scoping, "b").id);
    let second_a = scoping.resolved_references(a).get(1).copied();
    scoping.retain_resolved_references(|id| Some(id) != second_a);
    assert_eq!(starts(&scoping, a), [24, 36]);
    assert_eq!(starts(&scoping, b), [27, 33]);
    scoping.retain_resolved_references(|_| false);
    assert_eq!((starts(&scoping, a), starts(&scoping, b)), (Vec::new(), Vec::new()));
}
