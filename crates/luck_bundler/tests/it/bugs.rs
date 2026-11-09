#![expect(
    clippy::missing_panics_doc,
    reason = "a test panics to fail, so a Panics section says nothing"
)]

use std::fs;
use std::path::Path;
use std::thread::Builder;

use luck_bundler::{BundleOptions, BundleResult, LineMapEntry, bundle};
use luck_core::{Diagnostic, DynamicRequire};
use luck_semantic::{ScopeId, ScopeKind, ScopeTree, SymbolId};
use luck_token::LuaVersion;
use rustc_hash::{FxHashMap, FxHashSet};
use tempfile::TempDir;

const LUA_REQUIRE_UTIL: &[u8] = b"local util = require(\"util\")\nprint(util)\n";
const LUAU_REQUIRE_UTIL: &[u8] = b"local util = require(\"./util\")\nprint(util)\n";
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
) -> Result<BundleResult, Vec<Diagnostic>> {
    let search_paths = if version.is_luau() { vec![] } else { vec!["?.lua".to_owned()] };
    bundle(
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

fn scope_tree_of(output: &str, version: LuaVersion) -> ScopeTree {
    let parsed = luck_parser::parse(output, version);
    assert!(parsed.errors.is_empty(), "bundle must reparse: {:?}\n{output}", parsed.errors);
    luck_semantic::build_scope_tree(&parsed.block, version)
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
    let tree = scope_tree_of(output, version);
    let mut upvalues: FxHashMap<ScopeId, FxHashSet<SymbolId>> = FxHashMap::default();
    for reference in &tree.references {
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
            ("main.lua", b"local a = require(\"a\")\nprint(a.name)\n"),
            ("a.lua", b"module(\"a\", package.seeall)\nname = \"A\"\n"),
        ],
        "main.lua",
        LuaVersion::Lua51,
    );
    assert!(
        !result.output.contains("__luck_loading={}"),
        "the 5.1 in-progress sentinel must not be a table, or module() adopts it:\n{}",
        result.output
    );
}

#[test]
fn lua51_wrapper_does_not_shadow_the_global_arg() {
    let result = bundle_files(
        &[
            ("main.lua", b"local util = require(\"util\")\nprint(arg[0], util)\n"),
            ("util.lua", RETURN_ONE),
        ],
        "main.lua",
        LuaVersion::Lua51,
    );
    assert!(
        !result
            .output
            .contains("=function(...)\nlocal util = __luck_require(\"util\")\nprint(arg[0]"),
        "the entry's global `arg` must not resolve to the wrapper's hidden 5.1 local:\n{}",
        result.output
    );
}

#[test]
fn entry_keeps_its_200_local_budget_on_vararg_slot_versions() {
    let locals = (0..199usize).map(|i| format!("local v{i} = {i}\n")).collect::<Vec<_>>().concat();
    let main = format!("{locals}local d = require(\"util\")\nprint(d)\n");
    for version in [LuaVersion::Lua51, LuaVersion::Lua55] {
        let result = bundle_files(
            &[("main.lua", main.as_bytes()), ("util.lua", RETURN_ONE)],
            "main.lua",
            version,
        );
        assert!(
            !result.output.contains("=function(...)\nlocal v0 = 0\n"),
            "{version:?}: 200 entry locals must not share a function with a vararg slot"
        );
    }
}

#[test]
fn lua51_rewritten_require_stays_within_the_upvalue_limit() {
    let locals = (0..60usize).map(|i| format!("local u{i} = {i}\n")).collect::<Vec<_>>().concat();
    let sum = (0..60usize).map(|i| format!("u{i}")).collect::<Vec<_>>().join(" + ");
    let main = format!(
        "{locals}local function f()\n    local d = require(\"util\")\n    return {sum} + d\nend\nprint(f())\n"
    );
    let result = bundle_files(
        &[("main.lua", main.as_bytes()), ("util.lua", RETURN_ONE)],
        "main.lua",
        LuaVersion::Lua51,
    );
    let upvalues = max_function_upvalues(&result.output, LuaVersion::Lua51);
    assert!(upvalues <= 60, "Lua 5.1 allows 60 upvalues per function, the bundle has {upvalues}");
}

#[test]
fn module_assigning_env_rebinds_only_its_own_env() {
    let result = bundle_files(
        &[("main.lua", LUA_REQUIRE_UTIL), ("util.lua", b"_ENV = {}\nreturn 1\n")],
        "main.lua",
        LuaVersion::Lua54,
    );
    let tree = scope_tree_of(&result.output, LuaVersion::Lua54);
    let shared_writes = tree
        .references
        .iter()
        .filter(|reference| reference.name == "_ENV" && reference.kind.is_write())
        .filter(|reference| reference.resolved.is_none())
        .count();
    assert_eq!(
        shared_writes, 0,
        "`_ENV = ...` must write a binding private to the module:\n{}",
        result.output
    );
}

#[test]
fn lua52_module_call_does_not_replace_the_loader_upvalue() {
    let result = bundle_files(
        &[
            ("main.lua", b"local b = require(\"b\")\nprint(b.name)\n"),
            ("b.lua", b"local c = require(\"c\")\nmodule(\"b\", package.seeall)\nname = \"B\"\n"),
            ("c.lua", RETURN_ONE),
        ],
        "main.lua",
        LuaVersion::Lua52,
    );
    let tree = scope_tree_of(&result.output, LuaVersion::Lua52);
    let wrapper = tree
        .references
        .iter()
        .find(|reference| reference.name == "module")
        .expect("module() stays in the bundle")
        .scope;
    let mut captures: Vec<_> = tree
        .references
        .iter()
        .filter(|reference| reference.scope == wrapper)
        .filter(|reference| {
            reference.resolved.is_none_or(|symbol| tree.symbol(symbol).scope != wrapper)
        })
        .collect();
    captures.sort_by_key(|reference| reference.span.start);
    let first = captures.first().expect("the wrapper captures something");
    assert!(
        !first.name.starts_with("__luck"),
        "upvalue 1 of a wrapper calling module() must not be `{}`:\n{}",
        first.name,
        result.output
    );
}

#[test]
fn ignores_require_under_a_local_env_binding() {
    let main = b"local _ENV = { require = function(name) return name end, print = print }\nprint(require(\"util\"))\n";
    let result = bundle_files(
        &[("main.lua", main), ("util.lua", RETURN_ONE)],
        "main.lua",
        LuaVersion::Lua54,
    );
    assert!(
        !result.output.contains("__luck_require(\"util\")"),
        "a call through a local _ENV must keep its meaning:\n{}",
        result.output
    );
}

#[test]
fn ignores_require_after_the_file_replaces_the_global() {
    let main = b"function require(name) return \"custom:\" .. name end\nprint(require(\"util\"))\n";
    let result = bundle_files(
        &[("main.lua", main), ("util.lua", RETURN_ONE)],
        "main.lua",
        LuaVersion::Lua54,
    );
    assert!(
        !result.output.contains("__luck_require(\"util\")"),
        "a call to the file's own global `require` must not be rewritten:\n{}",
        result.output
    );
}

#[test]
fn lua55_global_declaration_reaches_require_past_a_local() {
    let main = b"local require = print\ndo\n    global require\n    local util = require(\"util\")\n    _ENV.print(util)\nend\n";
    let result = bundle_files(
        &[("main.lua", main), ("util.lua", RETURN_ONE)],
        "main.lua",
        LuaVersion::Lua55,
    );
    assert!(
        result.output.contains("__luck_require(\"util\")"),
        "a name declared `global` reaches the global `require`:\n{}",
        result.output
    );
}

#[test]
fn flags_w005_for_a_call_through_g_require() {
    let result = bundle_files(
        &[("main.lua", b"local util = _G.require(\"util\")\nlocal other = require(\"other\")\nprint(util, other)\n"), ("util.lua", RETURN_ONE), ("other.lua", RETURN_ONE)],
        "main.lua",
        LuaVersion::Lua54,
    );
    let is_warned = result.warnings.iter().any(|warning| warning.code == "W005");
    assert!(
        is_warned || !result.output.contains("_G.require"),
        "`_G.require` escapes bundling, so it must be rewritten or get W005: {:?}",
        result.warnings
    );
}

#[test]
fn standard_library_require_is_not_an_unresolved_module() {
    let main = b"local text = require(\"string\")\nprint(text.upper(\"x\"))\n";
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
            !result.output.contains("value=preloader(name)\n"),
            "{version:?}: a preload loader must be called with (name, nil):\n{}",
            result.output
        );
    }
}

#[test]
fn luau_value_export_cycle_does_not_raise() {
    let result = bundle_files(
        &[
            ("main.luau", b"local a = require(\"./a\")\nprint(a.fa())\n"),
            (
                "a.luau",
                b"local b = require(\"./b\")\nexport function fa() return b.fb() end\nexport function name() return \"A\" end\n",
            ),
            ("b.luau", b"local a = require(\"./a\")\nexport function fb() return a.name() end\n"),
        ],
        "main.luau",
        LuaVersion::Luau,
    );
    assert!(
        !result.output.contains("require cycle hit"),
        "a cycle of value-export modules must load:\n{}",
        result.output
    );
}

#[test]
fn luau_failed_load_does_not_leave_the_cycle_marker() {
    let result = bundle_files(
        &[("main.luau", LUAU_REQUIRE_UTIL), ("util.luau", b"error(\"boom\")\n")],
        "main.luau",
        LuaVersion::Luau,
    );
    let has_in_progress_marker = result.output.contains("__luck_loaded[id]=__luck_loading");
    assert!(
        !has_in_progress_marker || result.output.contains("pcall("),
        "a raising module must not stay marked as loading:\n{}",
        result.output
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
        !result.output.contains("(...)==nil"),
        "`return nil` is exactly one result and must load:\n{}",
        result.output
    );
}

#[test]
fn luau_export_table_reads_the_export_not_a_later_shadow() {
    let util = b"export local animal = \"dog\"\nlocal animal = \"cat\"\nanimal = \"bird\"\n";
    let result = bundle_files(
        &[("main.luau", LUAU_REQUIRE_UTIL), ("util.luau", util)],
        "main.luau",
        LuaVersion::Luau,
    );
    let tree = scope_tree_of(&result.output, LuaVersion::Luau);
    let shadow_start = result.output.find("\"cat\"").expect("the shadow stays in the bundle");
    let has_shadow_read = tree
        .references
        .iter()
        .filter(|reference| reference.name == "animal" && reference.kind.is_read())
        .filter_map(|reference| reference.resolved)
        .any(|symbol| {
            luck_token::offset_to_index(tree.symbol(symbol).definition_span.start)
                > shadow_start.saturating_sub("local animal = ".len())
        });
    assert!(!has_shadow_read, "the export must read the exported binding:\n{}", result.output);
}

#[test]
fn luau_export_table_survives_a_module_local_named_table() {
    let util = b"local table = 5\nexport local value = table\n";
    let result = bundle_files(
        &[("main.luau", LUAU_REQUIRE_UTIL), ("util.luau", util)],
        "main.luau",
        LuaVersion::Luau,
    );
    let tree = scope_tree_of(&result.output, LuaVersion::Luau);
    let freeze_start = result.output.find("table.freeze(").unwrap_or(usize::MAX);
    let is_captured = tree.references.iter().any(|reference| {
        reference.name == "table"
            && luck_token::offset_to_index(reference.span.start) == freeze_start
            && reference.resolved.is_some()
    });
    assert!(!is_captured, "freeze must reach the library table:\n{}", result.output);
}

#[test]
fn luau_closure_write_to_an_export_does_not_silently_succeed() {
    let util = b"export local count = 0\nexport function increment() count += 1 end\n";
    let result = bundle_files(
        &[("main.luau", LUAU_REQUIRE_UTIL), ("util.luau", util)],
        "main.luau",
        LuaVersion::Luau,
    );
    let tree = scope_tree_of(&result.output, LuaVersion::Luau);
    let has_local_write = tree.references.iter().any(|reference| {
        reference.name == "count" && reference.kind.is_write() && reference.resolved.is_some()
    });
    assert!(
        !has_local_write,
        "a write to an export must not hit a plain local:\n{}",
        result.output
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
    let parsed = luck_parser::parse(&result.output, LuaVersion::Luau);
    assert!(
        parsed.errors.is_empty(),
        "bundle must reparse: {:?}\n{}",
        parsed.errors,
        result.output
    );
}

#[test]
fn luau_export_function_attribute_lowering_keeps_lines_one_to_one() {
    for (util, marker_line) in [
        ("@native export function f() return 1 end\nlocal marker = 1\n", 2usize),
        ("@native\n-- note\n\nexport function f() return 1 end\nlocal marker = 1\n", 5),
    ] {
        let result = bundle_files(
            &[("main.luau", LUAU_REQUIRE_UTIL), ("util.luau", util.as_bytes())],
            "main.luau",
            LuaVersion::Luau,
        );
        let entry = line_map_entry(&result, "util.luau");
        let lines: Vec<&str> = result.output.lines().collect();
        assert_eq!(
            lines[entry.bundle_start_line + marker_line - 2],
            "local marker = 1",
            "source line {marker_line} must stay 1:1 for {util:?}:\n{}",
            result.output
        );
    }
}

#[test]
fn luau_export_table_storage_handles_every_declaration_shape() {
    let util = b"export local a, b: number = 1, 2\nexport local blank;\nlocal function poke() a = 3 blank = 4 end\n@native\n-- export mention\nexport function keep() return a end\nlocal keep = 5\nprint(keep, b)\n";
    let result = bundle_files(
        &[("main.luau", LUAU_REQUIRE_UTIL), ("util.luau", util)],
        "main.luau",
        LuaVersion::Luau,
    );
    let output = result.output.as_str();
    let parsed = luck_parser::parse(output, LuaVersion::Luau);
    assert!(parsed.errors.is_empty(), "bundle must reparse: {:?}\n{output}", parsed.errors);
    for needle in [
        "__luck_exports.a,__luck_exports.b = 1, 2\n",
        "__luck_exports.blank = nil;\n",
        "local function poke() __luck_exports.a = 3 __luck_exports.blank = 4 end\n",
        "@native\n-- export mention\nfunction __luck_exports.keep() return __luck_exports.a end\n",
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
        result.output.split_once("__luck_require(\"util\"").expect("rewritten call");
    let (call_tail, _) = after_callee.split_once(')').expect("closing parenthesis");
    assert_eq!(
        call_tail.matches(['\r', '\n']).count(),
        2,
        "the call spanned two extra lines:\n{}",
        result.output
    );
}

#[test]
fn long_left_associative_chain_does_not_overflow_the_stack() {
    let mut huge = "return 1".to_owned();
    huge.push_str(&"+1".repeat(400_000));
    huge.push('\n');
    let directory = write_project(&[
        ("main.lua", b"return require(\"huge\")\n"),
        ("huge.lua", huge.as_bytes()),
    ]);
    let joined = Builder::new()
        .stack_size(16 * 1024 * 1024)
        .spawn(move || {
            bundle_project(directory.path(), "main.lua", LuaVersion::Lua54)
                .map(|result| result.output.len())
        })
        .expect("spawn")
        .join();
    assert!(joined.is_ok(), "bundling a long expression must return, not abort");
}
