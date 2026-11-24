#![expect(
    clippy::missing_panics_doc,
    reason = "a test panics to fail, so a Panics section says nothing"
)]

use std::fs;
use std::path::Path;

use luck_allocator::Allocator;
use luck_ast::{SuffixedExpression, Visit};
use luck_bundler::{BundleOptions, BundleResult, DynamicRequire, LineMapEntry};
use luck_diagnostics::FileDiagnostics;
use luck_semantic::{ScopeId, ScopeKind, Scoping, SemanticBuilder, SymbolId};
use luck_token::LuaVersion;
use rustc_hash::{FxHashMap, FxHashSet};
use tempfile::TempDir;

use crate::common::{bundle_both, text};

const LUA_REQUIRE_UTIL: &[u8] = b"\
local util = require(\"util\")
print(util)
";
const LUAU_REQUIRE_UTIL: &[u8] = b"\
local util = require(\"./util\")
print(util)
";
const RETURN_ONE: &[u8] = b"return 1\n";

fn write_project(files: &[(&str, &[u8])]) -> TempDir {
    let directory = tempfile::tempdir().expect("tempdir");
    for (name, contents) in files {
        fs::write(directory.path().join(name), contents).expect("write project file");
    }
    directory
}

/// # Errors
///
/// Returns the diagnostics that stopped the project at `directory` from
/// bundling.
fn bundle_project(
    directory: &Path,
    entry: &str,
    version: LuaVersion,
) -> Result<BundleResult, Vec<FileDiagnostics>> {
    let search_paths = if version.is_luau() { vec![] } else { vec!["?.lua".to_owned()] };
    bundle_both(
        &directory.join(entry),
        &BundleOptions {
            version,
            search_paths: &search_paths,
            project_root: directory,
            dynamic_require: DynamicRequire::default(),
        },
    )
}

fn bundle_files(files: &[(&str, &[u8])], entry: &str, version: LuaVersion) -> BundleResult {
    let directory = write_project(files);
    bundle_project(directory.path(), entry, version).expect("bundle failed")
}

fn scoping_of(allocator: &Allocator, output: &str, version: LuaVersion) -> Scoping {
    let parsed = luck_parser::parse(allocator, output.as_bytes(), version);
    assert!(parsed.errors.is_empty(), "bundle must reparse: {:?}\n{output}", parsed.errors);
    let chunk = allocator.alloc(parsed.chunk);
    SemanticBuilder::new().with_counts(parsed.counts).build(chunk).semantic.into_scoping()
}

fn assert_reparses(output: &str, version: LuaVersion) {
    let allocator = Allocator::new();
    let parsed = luck_parser::parse(&allocator, output.as_bytes(), version);
    assert!(parsed.errors.is_empty(), "bundle must reparse: {:?}\n{output}", parsed.errors);
}

fn line_map_entry<'map>(result: &'map BundleResult, file_name: &str) -> &'map LineMapEntry {
    result
        .line_map
        .iter()
        .find(|entry| entry.path.ends_with(file_name))
        .unwrap_or_else(|| panic!("no line map entry for {file_name}"))
}

// The most upvalues any function in `output` captures, counting a capture on
// every function between the use and the declaring scope, as Lua's compiler
// does.
fn max_function_upvalues(output: &str, version: LuaVersion) -> usize {
    let allocator = Allocator::new();
    let tree = scoping_of(&allocator, output, version);
    let mut upvalues: FxHashMap<ScopeId, FxHashSet<SymbolId>> = FxHashMap::default();
    for reference in tree.references() {
        let Some(symbol) = reference.resolved else {
            continue;
        };
        let home = tree.symbol(symbol).scope;
        let mut current = Some(reference.scope);
        while let Some(scope_id) = current
            && scope_id != home
        {
            let scope = tree.scope(scope_id);
            if scope.kind == ScopeKind::Function {
                upvalues.entry(scope_id).or_default().insert(symbol);
            }
            current = scope.parent;
        }
    }
    upvalues.values().map(FxHashSet::len).max().unwrap_or(0)
}

#[test]
fn lua51_loading_sentinel_is_not_a_table() {
    let result = bundle_files(
        &[
            (
                "main.lua",
                b"\
local a = require(\"a\")
print(a.name)
",
            ),
            (
                "a.lua",
                b"\
module(\"a\", package.seeall)
name = \"A\"
",
            ),
        ],
        "main.lua",
        LuaVersion::Lua51,
    );
    assert!(
        !text(&result.output).contains("__luck_loading={}"),
        "the 5.1 in-progress sentinel must not be a table, or module() adopts it:\n{}",
        text(&result.output)
    );
}

#[test]
fn lua51_wrapper_does_not_shadow_the_global_arg() {
    let result = bundle_files(
        &[
            (
                "main.lua",
                b"\
local util = require(\"util\")
print(arg[0], util)
",
            ),
            ("util.lua", RETURN_ONE),
        ],
        "main.lua",
        LuaVersion::Lua51,
    );
    assert!(
        !text(&result.output).contains(
            "\
=function(...)
local util = __luck_require(\"util\")
print(arg[0]"
        ),
        "the entry's global `arg` must not resolve to the wrapper's hidden 5.1 local:\n{}",
        text(&result.output)
    );
}

#[test]
fn entry_keeps_its_200_local_budget_on_vararg_slot_versions() {
    let locals = (0..199usize).map(|i| format!("local v{i} = {i}\n")).collect::<Vec<_>>().concat();
    let main = format!(
        "\
{locals}local d = require(\"util\")
print(d)
"
    );
    for version in [LuaVersion::Lua51, LuaVersion::Lua55] {
        let result = bundle_files(
            &[("main.lua", main.as_bytes()), ("util.lua", RETURN_ONE)],
            "main.lua",
            version,
        );
        assert!(
            !text(&result.output).contains(
                "\
=function(...)
local v0 = 0
"
            ),
            "{version:?}: 200 entry locals must not share a function with a vararg slot"
        );
    }
}

#[test]
fn lua51_rewritten_require_stays_within_the_upvalue_limit() {
    let locals = (0..60usize).map(|i| format!("local u{i} = {i}\n")).collect::<Vec<_>>().concat();
    let sum = (0..60usize).map(|i| format!("u{i}")).collect::<Vec<_>>().join(" + ");
    let main = format!(
        "\
{locals}local function f()
    local d = require(\"util\")
    return {sum} + d
end
print(f())
"
    );
    let result = bundle_files(
        &[("main.lua", main.as_bytes()), ("util.lua", RETURN_ONE)],
        "main.lua",
        LuaVersion::Lua51,
    );
    let upvalues = max_function_upvalues(text(&result.output), LuaVersion::Lua51);
    assert!(upvalues <= 60, "Lua 5.1 allows 60 upvalues per function, the bundle has {upvalues}");
}

#[test]
fn module_assigning_env_rebinds_only_its_own_env() {
    let result = bundle_files(
        &[
            ("main.lua", LUA_REQUIRE_UTIL),
            (
                "util.lua",
                b"\
_ENV = {}
return 1
",
            ),
        ],
        "main.lua",
        LuaVersion::Lua54,
    );
    let allocator = Allocator::new();
    let tree = scoping_of(&allocator, text(&result.output), LuaVersion::Lua54);
    let shared_writes = tree
        .references()
        .iter()
        .filter(|reference| tree.name(reference.name) == "_ENV" && reference.kind.is_write())
        .filter(|reference| reference.resolved.is_none())
        .count();
    assert_eq!(
        shared_writes,
        0,
        "`_ENV = ...` must write a binding private to the module:\n{}",
        text(&result.output)
    );
}

#[test]
fn lua51_arg_restore_ends_before_a_body_opening_with_a_parenthesis() {
    let result = bundle_files(
        &[("main.lua", LUA_REQUIRE_UTIL), ("util.lua", b"(print)(..., arg)\n")],
        "main.lua",
        LuaVersion::Lua51,
    );
    let allocator = Allocator::new();
    let parsed = luck_parser::parse(&allocator, &result.output, LuaVersion::Lua51);
    assert!(
        parsed.errors.is_empty(),
        "restoring `arg` must not run into the body's `(`: {:?}\n{}",
        parsed.errors,
        text(&result.output)
    );
}

/// Counts calls of a function named `_ENV`.
#[derive(Default)]
struct EnvCalls(usize);

impl<'ast> Visit<'ast> for EnvCalls {
    fn visit_suffixed_expr(&mut self, suffixed_expr: &'ast SuffixedExpression<'ast>) {
        for call in suffixed_expr.calls() {
            if call.callee.as_name().is_some_and(|name| name.text == "_ENV") {
                self.0 = self.0.saturating_add(1);
            }
        }
        self.walk_suffixed_expr(suffixed_expr);
    }
}

#[test]
fn private_env_ends_before_a_body_opening_with_a_parenthesis() {
    let result = bundle_files(
        &[
            ("main.lua", LUA_REQUIRE_UTIL),
            (
                "util.lua",
                b"\
(print)(\"x\")
_ENV = {}
",
            ),
        ],
        "main.lua",
        LuaVersion::Lua54,
    );
    let allocator = Allocator::new();
    let parsed = luck_parser::parse(&allocator, &result.output, LuaVersion::Lua54);
    assert!(parsed.errors.is_empty(), "{:?}\n{}", parsed.errors, text(&result.output));
    let mut env_calls = EnvCalls::default();
    env_calls.visit_block(&parsed.chunk.block);
    assert_eq!(
        env_calls.0,
        0,
        "`_ENV=_ENV` must not call `_ENV` with `(print)`:\n{}",
        text(&result.output)
    );
}

#[test]
fn lua52_module_call_does_not_replace_the_loader_upvalue() {
    let result = bundle_files(
        &[
            (
                "main.lua",
                b"\
local b = require(\"b\")
print(b.name)
",
            ),
            (
                "b.lua",
                b"\
local c = require(\"c\")
module(\"b\", package.seeall)
name = \"B\"
",
            ),
            ("c.lua", RETURN_ONE),
        ],
        "main.lua",
        LuaVersion::Lua52,
    );
    let allocator = Allocator::new();
    let tree = scoping_of(&allocator, text(&result.output), LuaVersion::Lua52);
    let wrapper = tree
        .references()
        .iter()
        .find(|reference| tree.name(reference.name) == "module")
        .expect("module() stays in the bundle")
        .scope;
    let mut captures: Vec<_> = tree
        .references()
        .iter()
        .filter(|reference| reference.scope == wrapper)
        .filter(|reference| {
            reference.resolved.is_none_or(|symbol| tree.symbol(symbol).scope != wrapper)
        })
        .collect();
    captures.sort_by_key(|reference| reference.span.start);
    let first = captures.first().expect("the wrapper captures something");
    assert!(
        !tree.name(first.name).starts_with("__luck"),
        "upvalue 1 of a wrapper calling module() must not be `{}`:\n{}",
        tree.name(first.name),
        text(&result.output)
    );
}

#[test]
fn ignores_require_under_a_local_env_binding() {
    let main = b"\
local _ENV = { require = function(name) return name end, print = print }
print(require(\"util\"))
";
    let result = bundle_files(
        &[("main.lua", main), ("util.lua", RETURN_ONE)],
        "main.lua",
        LuaVersion::Lua54,
    );
    assert!(
        !text(&result.output).contains("__luck_require(\"util\")"),
        "a call through a local _ENV must keep its meaning:\n{}",
        text(&result.output)
    );
}

#[test]
fn ignores_require_after_the_file_replaces_the_global() {
    let main = b"\
function require(name) return \"custom:\" .. name end
print(require(\"util\"))
";
    let result = bundle_files(
        &[("main.lua", main), ("util.lua", RETURN_ONE)],
        "main.lua",
        LuaVersion::Lua54,
    );
    assert!(
        !text(&result.output).contains("__luck_require(\"util\")"),
        "a call to the file's own global `require` must not be rewritten:\n{}",
        text(&result.output)
    );
}

#[test]
fn lua55_global_declaration_reaches_require_past_a_local() {
    let main = b"\
local require = print
do
    global require
    local util = require(\"util\")
    _ENV.print(util)
end
";
    let result = bundle_files(
        &[("main.lua", main), ("util.lua", RETURN_ONE)],
        "main.lua",
        LuaVersion::Lua55,
    );
    assert!(
        text(&result.output).contains("__luck_require(\"util\")"),
        "a name declared `global` reaches the global `require`:\n{}",
        text(&result.output)
    );
}

#[test]
fn rewrites_a_call_through_g_require() {
    let result = bundle_files(
        &[
            (
                "main.lua",
                b"\
local util = _G.require(\"util\")
local other = (require)(\"other\")
local name = \"util\"
print(util, other, _ENV.require(name))
",
            ),
            ("util.lua", RETURN_ONE),
            ("other.lua", RETURN_ONE),
        ],
        "main.lua",
        LuaVersion::Lua54,
    );
    let output = text(&result.output);
    for rewritten in [
        "local util = __luck_require(\"util\")",
        "local other = __luck_require(\"other\")",
        "__luck_dynamic(name)",
    ] {
        assert!(output.contains(rewritten), "`{rewritten}` must be bundled:\n{output}");
    }
}

#[test]
fn a_call_through_the_global_table_across_lines_keeps_lines_one_to_one() {
    let main = b"\
local name = \"util\"
local dynamic = _G
    .require(name)
local static = _G
    .require(\"util\")
local marker = 1
print(dynamic, static, marker)
";
    let result = bundle_files(
        &[("main.lua", main), ("util.lua", RETURN_ONE)],
        "main.lua",
        LuaVersion::Lua54,
    );
    let entry = line_map_entry(&result, "main.lua");
    let lines: Vec<&str> = text(&result.output).lines().collect();
    assert_eq!(
        lines[entry.bundle_start_line + 4],
        "local marker = 1",
        "source line 6 must stay 1:1:\n{}",
        text(&result.output)
    );
}

#[test]
fn standard_library_require_is_not_an_unresolved_module() {
    let main = b"\
local text = require(\"string\")
print(text.upper(\"x\"))
";
    let directory = write_project(&[("main.lua", main)]);
    let result = bundle_project(directory.path(), "main.lua", LuaVersion::Lua54);
    assert!(
        result.is_ok(),
        "require(\"string\") must bundle, as the real require serves it: {result:?}"
    );
}

#[test]
fn lua52_preload_loader_receives_two_arguments() {
    for version in [LuaVersion::Lua52, LuaVersion::Lua53] {
        let result = bundle_files(
            &[("main.lua", LUA_REQUIRE_UTIL), ("util.lua", RETURN_ONE)],
            "main.lua",
            version,
        );
        assert!(
            !text(&result.output).contains("value=preloader(name)\n"),
            "{version:?}: a preload loader must be called with (name, nil):\n{}",
            text(&result.output)
        );
    }
}

#[test]
fn luau_value_export_cycle_does_not_raise() {
    let result = bundle_files(
        &[
            (
                "main.luau",
                b"\
local a = require(\"./a\")
print(a.fa())
",
            ),
            (
                "a.luau",
                b"\
local b = require(\"./b\")
export function fa() return b.fb() end
export function name() return \"A\" end
",
            ),
            (
                "b.luau",
                b"\
local a = require(\"./a\")
export function fb() return a.name() end
",
            ),
        ],
        "main.luau",
        LuaVersion::Luau,
    );
    assert!(
        !text(&result.output).contains("require cycle hit"),
        "a cycle of value-export modules must load:\n{}",
        text(&result.output)
    );
}

#[test]
fn luau_failed_load_does_not_leave_the_cycle_marker() {
    let result = bundle_files(
        &[("main.luau", LUAU_REQUIRE_UTIL), ("util.luau", b"error(\"boom\")\n")],
        "main.luau",
        LuaVersion::Luau,
    );
    let has_in_progress_marker = text(&result.output).contains("__luck_loaded[id]=__luck_loading");
    assert!(
        !has_in_progress_marker || text(&result.output).contains("pcall("),
        "a raising module must not stay marked as loading:\n{}",
        text(&result.output)
    );
}

#[test]
fn luau_module_returning_nil_is_accepted() {
    let result = bundle_files(
        &[("main.luau", LUAU_REQUIRE_UTIL), ("util.luau", b"return nil\n")],
        "main.luau",
        LuaVersion::Luau,
    );
    assert!(
        !text(&result.output).contains("(...)==nil"),
        "`return nil` is exactly one result and must load:\n{}",
        text(&result.output)
    );
}

#[test]
fn luau_export_table_reads_the_export_not_a_later_shadow() {
    let util = b"\
export local animal = \"dog\"
local animal = \"cat\"
animal = \"bird\"
";
    let result = bundle_files(
        &[("main.luau", LUAU_REQUIRE_UTIL), ("util.luau", util)],
        "main.luau",
        LuaVersion::Luau,
    );
    let allocator = Allocator::new();
    let tree = scoping_of(&allocator, text(&result.output), LuaVersion::Luau);
    let shadow_start =
        text(&result.output).find("\"cat\"").expect("the shadow stays in the bundle");
    let has_shadow_read = tree
        .references()
        .iter()
        .filter(|reference| tree.name(reference.name) == "animal" && reference.kind.is_read())
        .filter_map(|reference| reference.resolved)
        .any(|symbol| {
            luck_token::offset_to_index(tree.symbol(symbol).definition_span.start)
                > shadow_start.saturating_sub("local animal = ".len())
        });
    assert!(
        !has_shadow_read,
        "the export must read the exported binding:\n{}",
        text(&result.output)
    );
}

#[test]
fn luau_export_table_survives_a_module_local_named_table() {
    let util = b"\
local table = 5
export local value = table
";
    let result = bundle_files(
        &[("main.luau", LUAU_REQUIRE_UTIL), ("util.luau", util)],
        "main.luau",
        LuaVersion::Luau,
    );
    let allocator = Allocator::new();
    let tree = scoping_of(&allocator, text(&result.output), LuaVersion::Luau);
    let freeze_start = text(&result.output).find("table.freeze(").unwrap_or(usize::MAX);
    let is_captured = tree.references().iter().any(|reference| {
        tree.name(reference.name) == "table"
            && luck_token::offset_to_index(reference.span.start) == freeze_start
            && reference.resolved.is_some()
    });
    assert!(!is_captured, "freeze must reach the library table:\n{}", text(&result.output));
}

#[test]
fn luau_closure_write_to_an_export_does_not_silently_succeed() {
    let util = b"\
export local count = 0
export function increment() count += 1 end
";
    let result = bundle_files(
        &[("main.luau", LUAU_REQUIRE_UTIL), ("util.luau", util)],
        "main.luau",
        LuaVersion::Luau,
    );
    let allocator = Allocator::new();
    let tree = scoping_of(&allocator, text(&result.output), LuaVersion::Luau);
    let has_local_write = tree.references().iter().any(|reference| {
        tree.name(reference.name) == "count"
            && reference.kind.is_write()
            && reference.resolved.is_some()
    });
    assert!(
        !has_local_write,
        "a write to an export must not hit a plain local:\n{}",
        text(&result.output)
    );
}

#[test]
fn luau_bracketed_attribute_on_an_export_function_survives_lowering() {
    let result = bundle_files(
        &[
            ("main.luau", LUAU_REQUIRE_UTIL),
            ("util.luau", b"@[native] export function f() return 1 end\n"),
        ],
        "main.luau",
        LuaVersion::Luau,
    );
    assert_reparses(text(&result.output), LuaVersion::Luau);
}

#[test]
fn luau_export_function_attribute_lowering_keeps_lines_one_to_one() {
    for (util, marker_line) in [
        (
            "\
@native export function f() return 1 end
local marker = 1
",
            2usize,
        ),
        (
            "\
@native
-- note

export function f() return 1 end
local marker = 1
",
            5,
        ),
    ] {
        let result = bundle_files(
            &[("main.luau", LUAU_REQUIRE_UTIL), ("util.luau", util.as_bytes())],
            "main.luau",
            LuaVersion::Luau,
        );
        let entry = line_map_entry(&result, "util.luau");
        let lines: Vec<&str> = text(&result.output).lines().collect();
        assert_eq!(
            lines[entry.bundle_start_line + marker_line - 2],
            "local marker = 1",
            "source line {marker_line} must stay 1:1 for {util:?}:\n{}",
            text(&result.output)
        );
    }
}

#[test]
fn luau_export_table_storage_handles_every_declaration_shape() {
    let util = b"\
export local a, b: number = 1, 2
export local blank;
local function poke() a = 3 blank = 4 end
@native
-- export mention
export function keep() return a end
local keep = 5
print(keep, b)
";
    let result = bundle_files(
        &[("main.luau", LUAU_REQUIRE_UTIL), ("util.luau", util)],
        "main.luau",
        LuaVersion::Luau,
    );
    let output = text(&result.output);
    assert_reparses(output, LuaVersion::Luau);
    for needle in [
        "__luck_exports.a,__luck_exports.b = 1, 2\n",
        "__luck_exports.blank = nil;\n",
        "local function poke() __luck_exports.a = 3 __luck_exports.blank = 4 end\n",
        "\
@native
-- export mention
function __luck_exports.keep() return __luck_exports.a end
",
        "print(keep, __luck_exports.b)\n",
        "return __luck_freeze(__luck_exports)\n",
    ] {
        assert!(output.contains(needle), "expected {needle:?}:\n{output}");
    }
    let entry = line_map_entry(&result, "util.luau");
    let lines: Vec<&str> = output.lines().collect();
    assert_eq!(lines[entry.bundle_start_line + 5], "local keep = 5", "{output}");
}

#[test]
fn line_map_counts_lone_cr_line_breaks_in_a_wrapped_module() {
    let result = bundle_files(
        &[("main.lua", LUA_REQUIRE_UTIL), ("util.lua", b"local x = 1\rlocal y = 2\rreturn y\r")],
        "main.lua",
        LuaVersion::Lua54,
    );
    let entry = line_map_entry(&result, "util.lua");
    assert_eq!(
        entry.bundle_end_line - entry.bundle_start_line + 1,
        3,
        "a three-line CR module must map three lines: {entry:?}"
    );
}

#[test]
fn line_map_counts_lone_cr_line_breaks_in_a_lone_module() {
    let result =
        bundle_files(&[("main.lua", b"print(1)\rprint(2)\r")], "main.lua", LuaVersion::Lua54);
    let entry = line_map_entry(&result, "main.lua");
    assert_eq!(entry.bundle_end_line, 2, "a two-line CR file must map two lines: {entry:?}");
}

#[test]
fn multiline_require_keeps_its_lone_cr_line_breaks() {
    let result = bundle_files(
        &[("main.lua", b"local d = require(\r\"util\"\r)\rprint(d)\r"), ("util.lua", RETURN_ONE)],
        "main.lua",
        LuaVersion::Lua54,
    );
    let (_, after_callee) =
        text(&result.output).split_once("__luck_require(\"util\"").expect("rewritten call");
    let (call_tail, _) = after_callee.split_once(')').expect("closing parenthesis");
    assert_eq!(
        call_tail.matches(['\r', '\n']).count(),
        2,
        "the call spanned two extra lines:\n{}",
        text(&result.output)
    );
}

/// A module that holds the default prefix moves every generated name to
/// another, in the tree as in the text.
#[test]
fn a_module_holding_the_default_prefix_moves_the_loader_in_both_forms() {
    let lua = bundle_files(
        &[
            (
                "main.lua",
                b"\
local __luck_require = 1
local util = require(\"util\")
local name = \"util\"
print(__luck_require, util, require(name))
",
            ),
            ("util.lua", RETURN_ONE),
        ],
        "main.lua",
        LuaVersion::Lua51,
    );
    for loader_function in ["require", "dynamic"] {
        let declaration = format!("local function __luck1_{loader_function}(name)");
        assert!(text(&lua.output).contains(&declaration), "{}", text(&lua.output));
    }

    let luau = bundle_files(
        &[
            (
                "main.luau",
                b"\
local __luck_modules = 1
local util = require(\"./util\")
print(__luck_modules, util.value)
",
            ),
            ("util.luau", b"export local value = 1\n"),
        ],
        "main.luau",
        LuaVersion::Luau,
    );
    assert!(
        text(&luau.output).contains("local function __luck1_exporting(id,body)"),
        "{}",
        text(&luau.output)
    );
}
