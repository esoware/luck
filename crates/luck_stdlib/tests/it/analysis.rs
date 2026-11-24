use luck_allocator::Allocator;
use luck_ast::{Expression, Statement, TreeCounts};
use luck_stdlib::{SemanticAnalysis, analyze, library_for};
use luck_token::{LuaTarget, LuaVersion};

use crate::common::parse;

fn analyze_source<'ast>(
    allocator: &'ast Allocator,
    source: &'ast str,
    target: LuaTarget,
) -> SemanticAnalysis<'ast> {
    let chunk = parse(allocator, source, target.version());
    analyze(chunk, TreeCounts::default(), library_for(target)).0
}

#[test]
fn is_known_global_finds_stdlib_globals_and_not_locals() {
    let allocator = Allocator::new();
    let analysis = analyze_source(&allocator, "local x = print", LuaTarget::Lua54);
    assert!(analysis.is_known_global("print"));
    assert!(analysis.is_known_global("table"));
    assert!(!analysis.is_known_global("not_a_real_name"));
    assert!(!analysis.is_known_global("x"));
}

#[test]
fn is_defined_global_counts_a_global_star_in_scope() {
    let allocator = Allocator::new();
    let analysis = analyze_source(
        &allocator,
        "\
global *
return y",
        LuaTarget::Lua55,
    );
    let y = &analysis.scoping().references()[0];
    assert!(analysis.is_defined_global(y));
    assert!(!analysis.is_known_global("y"), "rename still treats `y` as a free name");
}

#[test]
fn extra_globals_are_known_globals() {
    let allocator = Allocator::new();
    let mut analysis = analyze_source(&allocator, "vim.cmd('x')", LuaTarget::Lua54);
    let vim = analysis.scoping().name_id("vim").expect("the chunk names vim");
    assert!(!analysis.is_known_global("vim"));
    analysis.add_extra_globals(["vim", "absent"]);
    assert!(analysis.is_known_global("vim"));
    assert!(analysis.is_extra_global(vim));
    assert!(analysis.is_defined_global(&analysis.scoping().references()[0]));
}

#[test]
fn roblox_globals_are_known_only_on_roblox() {
    let allocator = Allocator::new();
    let chunk = parse(&allocator, "print(game, bit32)", LuaVersion::Luau);
    let (standalone, _) = analyze(chunk, TreeCounts::default(), library_for(LuaTarget::Luau));
    let (roblox, _) = analyze(chunk, TreeCounts::default(), library_for(LuaTarget::LuauRoblox));
    assert!(!standalone.is_known_global("game"));
    assert!(roblox.is_known_global("game"));
    assert!(standalone.is_known_global("bit32"));
    assert!(roblox.is_known_global("bit32"));
}

#[test]
fn analysis_reports_the_checker_s_errors() {
    let allocator = Allocator::new();
    let chunk = parse(
        &allocator,
        "\
local x <const> = 1
x = 2",
        LuaVersion::Lua54,
    );
    let (_, diagnostics) = analyze(chunk, TreeCounts::default(), library_for(LuaTarget::Lua54));
    let messages: Vec<&str> = diagnostics.iter().map(|diag| &*diag.message).collect();
    assert_eq!(messages, ["attempt to assign to const variable `x`"]);
}

fn unknown_globals(source: &str) -> Vec<String> {
    let allocator = Allocator::new();
    let analysis = analyze_source(&allocator, source, LuaTarget::Luau);
    let tree = analysis.scoping();
    tree.global_references()
        .filter(|reference| !analysis.is_defined_global(reference))
        .map(|reference| tree.name(reference.name).to_owned())
        .collect()
}

#[test]
fn type_function_body_knows_the_types_library() {
    let unknown = unknown_globals(
        "\
type function f(t)
  return types.number
end",
    );
    assert!(
        unknown.is_empty(),
        "type functions run with the `types` library, so the body has no unknown global: \
         {unknown:?}"
    );
}

#[test]
fn type_function_body_knows_sibling_type_functions() {
    let unknown = unknown_globals(
        "\
type function f(t)
  return t
end
type function g(t)
  return f(t)
end",
    );
    assert!(
        unknown.is_empty(),
        "a type function can call a type function declared before it, as luau-analyze \
         accepts, so the body has no unknown global: {unknown:?}"
    );
}

#[test]
fn resolve_stdlib_path_reads_a_shaped_local_or_the_host_global() {
    let allocator = Allocator::new();
    let analysis = analyze_source(
        &allocator,
        "\
local f = io.open('x')
f:read()
local table = {}
table.insert(f)",
        LuaTarget::Lua54,
    );
    let tree = analysis.scoping();
    let reference_to = |name: &str| {
        tree.references()
            .iter()
            .find(|reference| tree.name(reference.name) == name)
            .expect("the source references the name")
    };
    assert!(analysis.resolve_stdlib_path(&["f", "read"], reference_to("f")).is_some());
    assert!(analysis.resolve_stdlib_path(&["io", "open"], reference_to("io")).is_some());
    assert!(analysis.resolve_stdlib_path(&["table", "insert"], reference_to("table")).is_none());
}

/// The display path of what the last statement's call calls, where the last
/// statement is a call or a local assignment whose last value is one.
fn last_call_callee(source: &str, target: LuaTarget) -> Option<String> {
    let allocator = Allocator::new();
    let chunk = parse(&allocator, source, target.version());
    let (analysis, _) = analyze(chunk, TreeCounts::default(), library_for(target));
    let last_stmt = chunk.block.stmts.last();
    let chain = if let Some(Statement::FunctionCall(chain)) = last_stmt {
        chain
    } else if let Some(Statement::LocalAssignment(local)) = last_stmt
        && let Some(Expression::Suffixed(chain)) = local.exprs.last()
    {
        chain
    } else {
        panic!("the last statement must be a call or a local assignment of one");
    };
    Some(analysis.resolve_callee(chain.last_call()?)?.name())
}

#[test]
fn resolve_callee_bails_under_a_local_env() {
    let source = "\
local _ENV = { print = function() end }
print(1, 2)";
    assert_eq!(
        last_call_callee(source, LuaTarget::Lua54),
        None,
        "with a local _ENV, print is _ENV.print on the user's table, not the stdlib print"
    );
}

#[test]
fn resolve_callee_bails_on_a_global_the_file_rebinds() {
    let source = "\
function tostring(a, b) return a end
tostring(1, 2)";
    assert_eq!(
        last_call_callee(source, LuaTarget::Lua54),
        None,
        "the chunk replaced the global tostring, so the call must not match the stdlib entry"
    );
}

/// The callee of each call in the last statement's chain, from one
/// `chain_callees`. Each must match what `resolve_callee` gives the call
/// alone.
fn chain_callee_names(source: &str) -> Vec<Option<String>> {
    let allocator = Allocator::new();
    let chunk = parse(&allocator, source, LuaVersion::Lua54);
    let (analysis, _) = analyze(chunk, TreeCounts::default(), library_for(LuaTarget::Lua54));
    let Some(Statement::FunctionCall(chain)) = chunk.block.stmts.last() else {
        panic!("the last statement must be a call");
    };
    let mut callees = analysis.chain_callees(chain);
    let mut names = Vec::new();
    for (index, call) in chain.calls() {
        let name = callees.callee(index).map(|callee| callee.name());
        assert_eq!(name, analysis.resolve_callee(call).map(|callee| callee.name()));
        names.push(name);
    }
    names
}

#[test]
fn chain_callees_resolve_each_call_of_a_chain() {
    for (source, expected) in [
        (
            "string.rep('x', 2):upper():len()",
            &[Some("string.rep"), Some("string:upper"), Some("string:len")][..],
        ),
        ("('x'):rep(2).x:upper()", &[Some("string:rep"), None]),
        ("math.floor(1.5):upper()", &[Some("math.floor"), None]),
        (
            "('x'):rep(2):upper():len()",
            &[Some("string:rep"), Some("string:upper"), Some("string:len")],
        ),
    ] {
        let names: Vec<Option<String>> =
            expected.iter().map(|name| name.map(str::to_owned)).collect();
        assert_eq!(chain_callee_names(source), names, "{source}");
    }
}

#[test]
fn chain_callees_resolve_a_long_method_chain() {
    let source = format!("('x'){}:len()", ":upper()".repeat(2000));
    let names = chain_callee_names(&source);
    assert_eq!(names.len(), 2001);
    assert!(names.iter().take(2000).all(|name| name.as_deref() == Some("string:upper")));
    assert_eq!(names.last().cloned().flatten().as_deref(), Some("string:len"));
}

#[test]
fn get_service_call_resolves() {
    let name = last_call_callee("game:GetService('Players')", LuaTarget::LuauRoblox)
        .expect("game:GetService resolves");
    assert_eq!(name, "game:GetService");
}

#[test]
fn get_service_chain_resolves() {
    let name = last_call_callee(
        "\
local players = game:GetService('Players')
local x = players:FindFirstChild('x')",
        LuaTarget::LuauRoblox,
    )
    .expect("chained FindFirstChild resolves");
    assert_eq!(name, "players:FindFirstChild");
}

#[test]
fn script_parent_chain_resolves() {
    let name = last_call_callee(
        "\
local container = script.Parent
local x = container:WaitForChild('Config')",
        LuaTarget::LuauRoblox,
    )
    .expect("script.Parent chain resolves");
    assert_eq!(name, "container:WaitForChild");
}

#[test]
fn user_settings_call_chain_resolves() {
    let name =
        last_call_callee("UserSettings():GetService('UserGameSettings')", LuaTarget::LuauRoblox)
            .expect("UserSettings():GetService resolves");
    assert_eq!(name, "UserSettings:GetService");
}

#[test]
fn workspace_raycast_resolves() {
    let name = last_call_callee("workspace:Raycast(origin, direction)", LuaTarget::LuauRoblox)
        .expect("workspace:Raycast resolves");
    assert_eq!(name, "workspace:Raycast");
}

#[test]
fn enum_get_enum_items_resolves() {
    let name =
        last_call_callee("local items = Enum.Material:GetEnumItems()", LuaTarget::LuauRoblox)
            .expect("Enum.Material:GetEnumItems resolves");
    assert_eq!(name, "Enum:GetEnumItems");
}

#[test]
fn enum_from_name_resolves() {
    let name =
        last_call_callee("local item = Enum.Material:FromName('Grass')", LuaTarget::LuauRoblox)
            .expect("Enum.Material:FromName resolves");
    assert_eq!(name, "Enum:FromName");
}

#[test]
fn enum_global_get_enums_resolves() {
    let name = last_call_callee("local all = Enum:GetEnums()", LuaTarget::LuauRoblox)
        .expect("Enum:GetEnums resolves");
    assert_eq!(name, "Enum:GetEnums");
}

#[test]
fn enum_item_name_is_string_shaped() {
    // EnumItem.Name carries the string shape, so string methods
    // chain off it.
    let name =
        last_call_callee("local upper = Enum.Material.Grass.Name:upper()", LuaTarget::LuauRoblox)
            .expect("Name:upper() resolves through the string shape");
    assert_eq!(name, "string:upper");
}
