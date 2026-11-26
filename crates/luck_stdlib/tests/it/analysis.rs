use luck_allocator::Allocator;
use luck_ast::{Expression, Statement, TreeCounts};
use luck_stdlib::{SemanticAnalysis, analyze};

use crate::common::{library, parse};

fn analyze_source<'ast>(allocator: &'ast Allocator, source: &'ast str) -> SemanticAnalysis<'ast> {
    let chunk = parse(allocator, source);
    analyze(chunk, TreeCounts::default(), library(false)).0
}

#[test]
fn is_known_global_finds_stdlib_globals_and_not_locals() {
    let allocator = Allocator::new();
    let analysis = analyze_source(&allocator, "local x = print");
    assert!(analysis.is_known_global("print"));
    assert!(analysis.is_known_global("table"));
    assert!(!analysis.is_known_global("not_a_real_name"));
    assert!(!analysis.is_known_global("x"));
}

#[test]
fn extra_globals_are_known_globals() {
    let allocator = Allocator::new();
    let mut analysis = analyze_source(&allocator, "app.cmd('x')");
    let app = analysis.scoping().name_id("app").expect("the chunk names app");
    assert!(!analysis.is_known_global("app"));
    analysis.add_extra_globals(["app", "absent"]);
    assert!(analysis.is_known_global("app"));
    assert!(analysis.is_extra_global(app));
    assert!(analysis.is_known_global_name(analysis.scoping().references()[0].name));
}

#[test]
fn roblox_globals_are_known_only_with_roblox_s_api() {
    let allocator = Allocator::new();
    let chunk = parse(&allocator, "print(game, bit32)");
    let (standalone, _) = analyze(chunk, TreeCounts::default(), library(false));
    let (roblox, _) = analyze(chunk, TreeCounts::default(), library(true));
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
const x = 1
x = 2",
    );
    let (_, diagnostics) = analyze(chunk, TreeCounts::default(), library(false));
    let messages: Vec<&str> = diagnostics.iter().map(|diag| &*diag.message).collect();
    assert_eq!(messages, ["attempt to assign to const variable `x`"]);
}

fn unknown_globals(source: &str) -> Vec<String> {
    let allocator = Allocator::new();
    let analysis = analyze_source(&allocator, source);
    let tree = analysis.scoping();
    tree.global_references()
        .filter(|reference| !analysis.is_known_global_name(reference.name))
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
local s = string.rep('x', 2)
s:upper()
local table = {}
table.insert(s)",
    );
    let tree = analysis.scoping();
    let reference_to = |name: &str| {
        tree.references()
            .iter()
            .find(|reference| tree.name(reference.name) == name)
            .expect("the source references the name")
    };
    assert!(analysis.resolve_stdlib_path(&["s", "upper"], reference_to("s")).is_some());
    assert!(analysis.resolve_stdlib_path(&["string", "rep"], reference_to("string")).is_some());
    assert!(analysis.resolve_stdlib_path(&["table", "insert"], reference_to("table")).is_none());
}

/// The display path of what the last statement's call calls, where the last
/// statement is a call or a local assignment whose last value is one.
fn last_call_callee(source: &str, has_roblox: bool) -> Option<String> {
    let allocator = Allocator::new();
    let chunk = parse(&allocator, source);
    let (analysis, _) = analyze(chunk, TreeCounts::default(), library(has_roblox));
    let last_stmt = chunk.stmts.last();
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
fn resolve_callee_bails_on_a_global_the_file_rebinds() {
    let source = "\
function tostring(a, b) return a end
tostring(1, 2)";
    assert_eq!(
        last_call_callee(source, false),
        None,
        "the chunk replaced the global tostring, so the call must not match the stdlib entry"
    );
}

/// The callee of each call in the last statement's chain, from one
/// `chain_callees`. Each must match what `resolve_callee` gives the call
/// alone.
fn chain_callee_names(source: &str) -> Vec<Option<String>> {
    let allocator = Allocator::new();
    let chunk = parse(&allocator, source);
    let (analysis, _) = analyze(chunk, TreeCounts::default(), library(false));
    let Some(Statement::FunctionCall(chain)) = chunk.stmts.last() else {
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
    let name =
        last_call_callee("game:GetService('Players')", true).expect("game:GetService resolves");
    assert_eq!(name, "game:GetService");
}

#[test]
fn get_service_chain_resolves() {
    let name = last_call_callee(
        "\
local players = game:GetService('Players')
local x = players:FindFirstChild('x')",
        true,
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
        true,
    )
    .expect("script.Parent chain resolves");
    assert_eq!(name, "container:WaitForChild");
}

#[test]
fn user_settings_call_chain_resolves() {
    let name = last_call_callee("UserSettings():GetService('UserGameSettings')", true)
        .expect("UserSettings():GetService resolves");
    assert_eq!(name, "UserSettings:GetService");
}

#[test]
fn workspace_raycast_resolves() {
    let name = last_call_callee("workspace:Raycast(origin, direction)", true)
        .expect("workspace:Raycast resolves");
    assert_eq!(name, "workspace:Raycast");
}

#[test]
fn enum_get_enum_items_resolves() {
    let name = last_call_callee("local items = Enum.Material:GetEnumItems()", true)
        .expect("Enum.Material:GetEnumItems resolves");
    assert_eq!(name, "Enum:GetEnumItems");
}

#[test]
fn enum_from_name_resolves() {
    let name = last_call_callee("local item = Enum.Material:FromName('Grass')", true)
        .expect("Enum.Material:FromName resolves");
    assert_eq!(name, "Enum:FromName");
}

#[test]
fn enum_global_get_enums_resolves() {
    let name =
        last_call_callee("local all = Enum:GetEnums()", true).expect("Enum:GetEnums resolves");
    assert_eq!(name, "Enum:GetEnums");
}

#[test]
fn enum_item_name_is_string_shaped() {
    // EnumItem.Name carries the string shape, so string methods
    // chain off it.
    let name = last_call_callee("local upper = Enum.Material.Grass.Name:upper()", true)
        .expect("Name:upper() resolves through the string shape");
    assert_eq!(name, "string:upper");
}
