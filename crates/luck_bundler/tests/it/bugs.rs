use luck_allocator::Allocator;
use luck_bundler::{Bundle, LineMapEntry};
use luck_semantic::{Scoping, SemanticBuilder};

use crate::common::{REQUIRE_UTIL, RETURN_ONE, assert_reparses, bundle_files, text};

fn scoping_of(allocator: &Allocator, output: &str) -> Scoping {
    let parsed = luck_parser::parse(allocator, output.as_bytes());
    assert!(parsed.errors.is_empty(), "bundle must reparse: {:?}\n{output}", parsed.errors);
    let chunk = allocator.alloc(parsed.chunk);
    SemanticBuilder::new().with_counts(parsed.counts).build(chunk).semantic.into_scoping()
}

fn line_map_entry<'map>(result: &'map Bundle, file_name: &str) -> &'map LineMapEntry {
    result
        .line_map
        .iter()
        .find(|entry| entry.path.ends_with(file_name))
        .unwrap_or_else(|| panic!("no line map entry for {file_name}"))
}

#[test]
fn body_opening_with_a_parenthesis_reparses() {
    let result = bundle_files(
        &[
            ("main.luau", REQUIRE_UTIL.as_bytes()),
            (
                "util.luau",
                b"\
(print)(\"x\")
return 1
",
            ),
        ],
        "main.luau",
    )
    .expect("bundle failed");
    assert_reparses(&result.output);
}

#[test]
fn ignores_require_after_the_file_replaces_the_global() {
    let main = b"\
function require(name) return \"custom:\" .. name end
print(require(\"./util\"))
";
    let result = bundle_files(&[("main.luau", main), ("util.luau", RETURN_ONE)], "main.luau")
        .expect("bundle failed");
    assert!(
        !text(&result.output).contains("__luck_require(1)"),
        "a call to the file's own global `require` must not be rewritten:\n{}",
        text(&result.output)
    );
}

#[test]
fn rewrites_a_call_through_g_require() {
    let result = bundle_files(
        &[
            (
                "main.luau",
                b"\
local util = _G.require(\"./util\")
local other = (require)(\"./other\")
local name = \"./util\"
print(util, other, _G.require(name))
",
            ),
            ("util.luau", RETURN_ONE),
            ("other.luau", RETURN_ONE),
        ],
        "main.luau",
    )
    .expect("bundle failed");
    let output = text(&result.output);
    for rewritten in ["local util = __luck_require(", "local other = __luck_require("] {
        assert!(output.contains(rewritten), "`{rewritten}` must be bundled:\n{output}");
    }
    assert!(output.contains("_G.require(name)"), "a dynamic require stays as written:\n{output}");
}

#[test]
fn a_call_through_the_global_table_across_lines_keeps_lines_one_to_one() {
    let main = b"\
local name = \"./util\"
local dynamic = _G
    .require(name)
local static = _G
    .require(\"./util\")
local marker = 1
print(dynamic, static, marker)
";
    let result = bundle_files(&[("main.luau", main), ("util.luau", RETURN_ONE)], "main.luau")
        .expect("bundle failed");
    let entry = line_map_entry(&result, "main.luau");
    let lines: Vec<&str> = text(&result.output).lines().collect();
    assert_eq!(
        lines[entry.bundle_start_line + 4],
        "local marker = 1",
        "source line 6 must stay 1:1:\n{}",
        text(&result.output)
    );
}

#[test]
fn value_export_cycle_does_not_raise() {
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
    )
    .expect("bundle failed");
    assert!(
        !text(&result.output).contains("require cycle hit"),
        "a cycle of value-export modules must load:\n{}",
        text(&result.output)
    );
}

#[test]
fn failed_load_does_not_leave_the_cycle_marker() {
    let result = bundle_files(
        &[("main.luau", REQUIRE_UTIL.as_bytes()), ("util.luau", b"error(\"boom\")\n")],
        "main.luau",
    )
    .expect("bundle failed");
    let has_in_progress_marker = text(&result.output).contains("__luck_loaded[id]=__luck_loading");
    assert!(
        !has_in_progress_marker || text(&result.output).contains("pcall("),
        "a raising module must not stay marked as loading:\n{}",
        text(&result.output)
    );
}

#[test]
fn module_returning_nil_is_accepted() {
    let result = bundle_files(
        &[("main.luau", REQUIRE_UTIL.as_bytes()), ("util.luau", b"return nil\n")],
        "main.luau",
    )
    .expect("bundle failed");
    assert!(
        !text(&result.output).contains("(...)==nil"),
        "`return nil` is exactly one result and must load:\n{}",
        text(&result.output)
    );
}

#[test]
fn export_table_reads_the_export_not_a_later_shadow() {
    let util = b"\
export local animal = \"dog\"
local animal = \"cat\"
animal = \"bird\"
";
    let result =
        bundle_files(&[("main.luau", REQUIRE_UTIL.as_bytes()), ("util.luau", util)], "main.luau")
            .expect("bundle failed");
    let allocator = Allocator::new();
    let scoping = scoping_of(&allocator, text(&result.output));
    let shadow_start =
        text(&result.output).find("\"cat\"").expect("the shadow stays in the bundle");
    let has_shadow_read = scoping
        .references()
        .iter()
        .filter(|reference| scoping.name(reference.name) == "animal" && reference.kind.is_read())
        .filter_map(|reference| reference.resolved)
        .any(|symbol| {
            scoping.symbol(symbol).definition_span.start as usize
                > shadow_start - "local animal = ".len()
        });
    assert!(
        !has_shadow_read,
        "the export must read the exported binding:\n{}",
        text(&result.output)
    );
}

#[test]
fn export_table_survives_a_module_local_named_table() {
    let util = b"\
local table = 5
export local value = table
";
    let result =
        bundle_files(&[("main.luau", REQUIRE_UTIL.as_bytes()), ("util.luau", util)], "main.luau")
            .expect("bundle failed");
    let allocator = Allocator::new();
    let scoping = scoping_of(&allocator, text(&result.output));
    let freeze_start =
        text(&result.output).find("table.freeze").expect("the bundle reads `table.freeze`");
    let freeze_read = scoping
        .references()
        .iter()
        .find(|reference| reference.span.start as usize == freeze_start)
        .expect("a name is read where `table.freeze` starts");
    assert_eq!(scoping.name(freeze_read.name), "table");
    assert!(
        freeze_read.resolved.is_none(),
        "freeze must reach the library table:\n{}",
        text(&result.output)
    );
}

#[test]
fn closure_write_to_an_export_does_not_silently_succeed() {
    let util = b"\
export local count = 0
export function increment() count += 1 end
";
    let result =
        bundle_files(&[("main.luau", REQUIRE_UTIL.as_bytes()), ("util.luau", util)], "main.luau")
            .expect("bundle failed");
    let allocator = Allocator::new();
    let scoping = scoping_of(&allocator, text(&result.output));
    let has_local_write = scoping.references().iter().any(|reference| {
        scoping.name(reference.name) == "count"
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
fn bracketed_attribute_on_an_export_function_survives_lowering() {
    let result = bundle_files(
        &[
            ("main.luau", REQUIRE_UTIL.as_bytes()),
            ("util.luau", b"@[native] export function f() return 1 end\n"),
        ],
        "main.luau",
    )
    .expect("bundle failed");
    assert_reparses(&result.output);
}

#[test]
fn export_function_attribute_lowering_keeps_lines_one_to_one() {
    for (util, marker_line) in [
        (
            "\
@native export function f() return 1 end
local marker = 1
",
            2,
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
            &[("main.luau", REQUIRE_UTIL.as_bytes()), ("util.luau", util.as_bytes())],
            "main.luau",
        )
        .expect("bundle failed");
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
fn export_table_storage_handles_every_declaration_shape() {
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
    let result =
        bundle_files(&[("main.luau", REQUIRE_UTIL.as_bytes()), ("util.luau", util)], "main.luau")
            .expect("bundle failed");
    let output = text(&result.output);
    assert_reparses(&result.output);
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

/// Only LF ends a line, as Luau reads it, so a lone CR doesn't.
#[test]
fn line_map_reads_a_lone_cr_as_no_line_break_in_a_wrapped_module() {
    let result = bundle_files(
        &[
            ("main.luau", REQUIRE_UTIL.as_bytes()),
            ("util.luau", b"local x = 1\rlocal y = 2\rreturn y\r"),
        ],
        "main.luau",
    )
    .expect("bundle failed");
    let entry = line_map_entry(&result, "util.luau");
    assert_eq!(
        entry.bundle_end_line - entry.bundle_start_line + 1,
        1,
        "a module with only lone CRs is one line: {entry:?}"
    );
}

#[test]
fn line_map_reads_a_lone_cr_as_no_line_break_in_a_lone_module() {
    let result = bundle_files(&[("main.luau", b"print(1)\rprint(2)\r")], "main.luau")
        .expect("bundle failed");
    let entry = line_map_entry(&result, "main.luau");
    assert_eq!(entry.bundle_end_line, 1, "a file with only lone CRs is one line: {entry:?}");
}

/// A module that holds the default prefix moves every generated name to
/// another, in the tree as in the text.
#[test]
fn a_module_holding_the_default_prefix_moves_the_loader_in_both_forms() {
    let result = bundle_files(
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
    )
    .expect("bundle failed");
    let output = text(&result.output);
    for declaration in
        ["local function __luck1_require(id)", "local function __luck1_exporting(id,body)"]
    {
        assert!(output.contains(declaration), "{output}");
    }
}
