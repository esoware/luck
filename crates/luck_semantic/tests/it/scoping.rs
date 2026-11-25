use luck_allocator::Allocator;
use luck_ast::Name;
use luck_semantic::{
    Reference, ReferenceKind, ScopeKind, Scoping, Semantic, SemanticBuilder, Symbol, SymbolId,
    SymbolKind,
};
use luck_token::Span;

use crate::common::{NAMES_OF_EVERY_KIND, parse, semantic_for};

fn scoping_of(allocator: &Allocator, source: &str) -> Scoping {
    SemanticBuilder::new().build(parse(allocator, source)).semantic.into_scoping()
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

fn is_host_print(source: &str) -> bool {
    let allocator = Allocator::new();
    let scoping = scoping_of(&allocator, source);
    let print = *references(&scoping, "print").last().expect("the source references print");
    scoping.is_host_global(print)
}

#[test]
fn is_host_global_requires_a_global_the_chunk_leaves_alone() {
    assert!(is_host_print("print(1)"));
    assert!(!is_host_print(
        "\
function print() end
print(1)"
    ));
}

#[test]
fn is_host_global_is_false_for_a_local() {
    assert!(!is_host_print(
        "\
local print = 1
print(1)"
    ));
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
    let scoping = scoping_of(
        &allocator,
        "\
type function f(t) return g(t) end
type function g(t) return t end",
    );
    assert_eq!(references(&scoping, "g").len(), 0);
}

#[test]
fn type_function_body_sees_no_type_of_a_closed_block() {
    let allocator = Allocator::new();
    let scoping = scoping_of(
        &allocator,
        "\
do type Closed = number end
do
type Open = number
type function f() return Closed, Open end
end",
    );
    assert_eq!(references(&scoping, "Closed").len(), 1);
    assert_eq!(references(&scoping, "Open").len(), 0);
}

#[test]
fn type_runtime_names_are_only_known_in_type_functions() {
    let allocator = Allocator::new();
    let scoping = scoping_of(
        &allocator,
        "\
type T = number
print(types, T)",
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
    let scoping = scoping_of(
        &allocator,
        "\
local x = 1
type T = typeof(x)",
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
fn param_annotation_typeof_does_not_see_parameters() {
    let allocator = Allocator::new();
    let scoping = scoping_of(
        &allocator,
        "\
local a = 1
local function f(a: number, b: typeof(a)) return b end",
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
    let scoping = scoping_of(
        &allocator,
        "\
local a = 1
local function f(a: number): typeof(a) return a end",
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
    let scoping = scoping_of(
        &allocator,
        "\
local a = 1
do
  local a, b: typeof(a) = 2, 3
  print(a, b)
end",
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
    let scoping = scoping_of(
        &allocator,
        "\
local i = 1
for i: typeof(i) = 1, 2 do print(i) end",
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
    let scoping = scoping_of(
        &allocator,
        "\
local k = 1
for k, v: typeof(k) in pairs({}) do print(k, v) end",
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
    let scoping = scoping_of(
        &allocator,
        "\
local g = 1
local function g(z: typeof(g)) return z end",
    );
    assert_eq!(
        references(&scoping, "g")[0].resolved,
        Some(symbol(&scoping, "g").id),
        "Luau binds a local function's name after parsing its signature, so typeof(g) \
         in a parameter annotation names the outer local"
    );
}

#[test]
fn every_name_finds_what_it_declares_or_refers_to() {
    let allocator = Allocator::new();
    let semantic = semantic_for(&allocator, NAMES_OF_EVERY_KIND);
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
    }
    assert!(found.iter().all(|&count| count == 1), "{found:?}");
    let spelled: Vec<SymbolId> = scoping
        .symbols()
        .iter()
        .filter(|symbol| !symbol.kind.is_implicit())
        .map(|symbol| symbol.id)
        .collect();
    assert_eq!(declared, spelled);
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
fn a_typeof_in_a_nested_function_captures_nothing() {
    let allocator = Allocator::new();
    let scoping = scoping_of(
        &allocator,
        "\
local x = 1
local function f(y: typeof(x)) local z: typeof(x) = y end",
    );
    assert_eq!(references(&scoping, "x").len(), 2);
    assert!(!symbol(&scoping, "x").is_upvalue, "types never run");
}

#[test]
fn a_reference_in_a_type_is_in_a_type() {
    let allocator = Allocator::new();
    let scoping = scoping_of(
        &allocator,
        "local x = 1 local y: typeof(x) = x return function(): typeof(x) end",
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
