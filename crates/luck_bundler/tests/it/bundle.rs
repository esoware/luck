use std::path::{Path, PathBuf};
use std::{fs, io};

use luck_allocator::{Allocator, ThreadAllocators};
use luck_bundler::{BundleOptions, BundleResult, DependencyGraph, DynamicRequire};
use luck_diagnostics::FileDiagnostics;
use luck_resolver::{FileSystem as _, FileSystemOs, to_slash_lossy};
use luck_token::LuaVersion;
use tempfile::TempDir;

use crate::common::{bundle_both, codes, located, off_pool, on_pool, text};

fn fixture_directory(dialect: &str, name: &str) -> PathBuf {
    let manifest_directory = env!("CARGO_MANIFEST_DIR");
    PathBuf::from(manifest_directory)
        .join("../../tests/fixtures")
        .join(dialect)
        .join(name)
        .join("input")
}

fn run_bundle(
    dialect: &str,
    name: &str,
    version: LuaVersion,
    entry_name: &str,
) -> Result<String, Vec<FileDiagnostics>> {
    run_bundle_with(dialect, name, version, entry_name, DynamicRequire::default())
        .map(|result| String::from_utf8(result.output).unwrap())
}

fn run_bundle_with(
    dialect: &str,
    name: &str,
    version: LuaVersion,
    entry_name: &str,
    dynamic_require: DynamicRequire,
) -> Result<BundleResult, Vec<FileDiagnostics>> {
    let input_directory = fixture_directory(dialect, name);
    let entry = input_directory.join(entry_name);
    let search_paths =
        if version.is_luau() { vec![] } else { vec!["?.lua".to_owned(), "?/init.lua".to_owned()] };
    bundle_both(
        &entry,
        &BundleOptions {
            version,
            search_paths: &search_paths,
            project_root: &input_directory,
            dynamic_require,
        },
    )
}

#[test]
fn basic_bundle() {
    let output =
        run_bundle("lua54", "basic_bundle", LuaVersion::Lua54, "main.lua").expect("bundle failed");
    insta::assert_snapshot!("basic_bundle", output);
}

#[test]
fn nested_deps() {
    let output =
        run_bundle("lua54", "nested_deps", LuaVersion::Lua54, "main.lua").expect("bundle failed");
    insta::assert_snapshot!("nested_deps", output);
}

#[test]
fn diamond_deps() {
    let output =
        run_bundle("lua54", "diamond_deps", LuaVersion::Lua54, "main.lua").expect("bundle failed");
    insta::assert_snapshot!("diamond_deps", output);
}

#[test]
fn deep_chain() {
    let output =
        run_bundle("lua54", "deep_chain", LuaVersion::Lua54, "main.lua").expect("bundle failed");
    insta::assert_snapshot!("deep_chain", output);
}

#[test]
fn init_module() {
    let output =
        run_bundle("lua54", "init_module", LuaVersion::Lua54, "main.lua").expect("bundle failed");
    insta::assert_snapshot!("init_module", output);
}

#[test]
fn multiple_requires() {
    let output = run_bundle("lua54", "multiple_requires", LuaVersion::Lua54, "main.lua")
        .expect("bundle failed");
    insta::assert_snapshot!("multiple_requires", output);
}

#[test]
fn module_required_by_many() {
    let output = run_bundle("lua54", "module_required_by_many", LuaVersion::Lua54, "main.lua")
        .expect("bundle failed");
    insta::assert_snapshot!("module_required_by_many", output);
}

#[test]
fn nil_return() {
    let output =
        run_bundle("lua54", "nil_return", LuaVersion::Lua54, "main.lua").expect("bundle failed");
    insta::assert_snapshot!("nil_return", output);
}

#[test]
fn no_deps() {
    let output =
        run_bundle("lua54", "no_deps", LuaVersion::Lua54, "main.lua").expect("bundle failed");
    insta::assert_snapshot!("no_deps", output);
}

#[test]
fn circular_dependency_bundles_with_w003() {
    // The lazy loader lets a cycle bundle; a W003 warning flags the risk.
    let result = run_bundle_with(
        "lua54",
        "errors/circular_dep",
        LuaVersion::Lua54,
        "a.lua",
        DynamicRequire::default(),
    )
    .expect("cycle must bundle");
    assert!(
        codes(&result.warnings).contains(&"W003"),
        "Expected W003 warning, got: {:?}",
        codes(&result.warnings)
    );
    assert!(text(&result.output).contains("__luck_require"));
    // The cycle runs through the entry: its function must register under
    // the require string so the bundle stays self-contained.
    assert!(
        text(&result.output).contains("__luck_modules[\"a\"]=__luck_entry"),
        "entry must register when required:\n{}",
        text(&result.output)
    );
}

#[test]
fn missing_entry_flags_e010() {
    let directory = tempfile::tempdir().expect("tempdir");
    let errors = bundle_both(
        &directory.path().join("main.lua"),
        &BundleOptions {
            version: LuaVersion::Lua54,
            search_paths: &[],
            project_root: directory.path(),
            dynamic_require: DynamicRequire::default(),
        },
    )
    .expect_err("a missing entry must not bundle");
    assert_eq!(codes(&errors), ["E010"], "{errors:?}");
}

#[test]
fn unresolved_module_flags_e004() {
    let errors = run_bundle("lua54", "errors/unresolved_module", LuaVersion::Lua54, "main.lua")
        .expect_err("an unresolved module must not bundle");
    assert!(codes(&errors).contains(&"E004"), "Expected E004, got: {:?}", codes(&errors));
}

#[test]
fn non_literal_require_flags_e002_in_error_mode() {
    let errors = run_bundle_with(
        "lua54",
        "errors/non_literal_require",
        LuaVersion::Lua54,
        "main.lua",
        DynamicRequire::Error,
    )
    .expect_err("error mode must refuse a dynamic require");
    assert!(codes(&errors).contains(&"E002"), "Expected E002, got: {:?}", codes(&errors));
}

/// The default: the bundle is produced, the call is retargeted at the
/// loader's dynamic entry point, and W007 says so.
#[test]
fn dynamic_require_bundles_through_the_loader() {
    let result = run_bundle_with(
        "lua54",
        "dynamic_require",
        LuaVersion::Lua54,
        "main.lua",
        DynamicRequire::default(),
    )
    .expect("a dynamic require must not abort the bundle");

    assert!(
        codes(&result.warnings).contains(&"W007"),
        "Expected W007, got: {:?}",
        codes(&result.warnings)
    );
    insta::assert_snapshot!("dynamic_require", text(&result.output));
}

#[test]
fn dynamic_require_in_allow_mode_is_silent() {
    let result = run_bundle_with(
        "lua54",
        "dynamic_require",
        LuaVersion::Lua54,
        "main.lua",
        DynamicRequire::Allow,
    )
    .expect("bundle failed");
    assert!(result.warnings.is_empty(), "expected no warnings, got: {:?}", codes(&result.warnings));
    assert!(text(&result.output).contains("__luck_dynamic("), "{}", text(&result.output));
}

#[test]
fn bare_require_bundles() {
    // Side-effect imports (`require("x")` as a statement) are legal.
    let output = run_bundle("lua54", "errors/bare_require", LuaVersion::Lua54, "main.lua")
        .expect("bare require must bundle");
    assert!(output.contains("__luck_require"), "{output}");
}

#[test]
fn require_after_code_bundles() {
    // Requires are position-independent with the lazy loader.
    let output = run_bundle("lua54", "errors/require_after_code", LuaVersion::Lua54, "main.lua")
        .expect("require after code must bundle");
    assert!(output.contains("__luck_require"), "{output}");
}

#[test]
fn package_loaded_write_bundles_on_lua_targets() {
    // On Lua targets the bundle cache is package.loaded itself, so
    // preseeding an entry behaves as in real Lua.
    let output = run_bundle("lua54", "errors/package_loaded_manip", LuaVersion::Lua54, "main.lua")
        .expect("package.loaded writes bundle fine on Lua targets");
    assert!(output.contains("package.loaded[\"x\"] = {}"), "{output}");
}

#[test]
fn package_loaded_write_bundles_on_luau() {
    // Luau's `require` never reads `package.loaded`, so the write means in
    // the bundle what it means without one.
    let output = run_bundle("luau", "errors/package_loaded_manip", LuaVersion::Luau, "main.luau")
        .expect("package.loaded writes bundle fine on Luau");
    assert!(output.contains("package.loaded[\"x\"] = {}"), "{output}");
}

#[test]
fn luau_relative_require() {
    let output = run_bundle("luau", "relative_require", LuaVersion::Luau, "main.luau")
        .expect("bundle failed");
    insta::assert_snapshot!("luau_relative_require", output);
}

#[test]
fn luau_alias_require() {
    let output = run_bundle("luau", "alias_require", LuaVersion::Luau, "src/main.luau")
        .expect("bundle failed");
    insta::assert_snapshot!("luau_alias_require", output);
}

#[test]
fn luau_luaurc_inheritance() {
    let output =
        run_bundle("luau", "luaurc_inheritance", LuaVersion::Luau, "src/deep/nested/mod.luau")
            .expect("bundle failed");
    insta::assert_snapshot!("luau_luaurc_inheritance", output);
}

#[test]
fn luau_init_luau() {
    let output =
        run_bundle("luau", "init_luau", LuaVersion::Luau, "main.luau").expect("bundle failed");
    insta::assert_snapshot!("luau_init_luau", output);
}

#[test]
fn luau_type_annotations() {
    let output = run_bundle("luau", "type_annotations", LuaVersion::Luau, "main.luau")
        .expect("bundle failed");
    insta::assert_snapshot!("luau_type_annotations", output);
}

#[test]
fn luau_string_interpolation() {
    let output = run_bundle("luau", "string_interpolation", LuaVersion::Luau, "main.luau")
        .expect("bundle failed");
    insta::assert_snapshot!("luau_string_interpolation", output);
}

#[test]
fn luau_ambiguous_extension_flags_e007() {
    let errors = run_bundle("luau", "errors/luau_ambiguous_ext", LuaVersion::Luau, "main.luau")
        .expect_err("an ambiguous module must not bundle");
    assert!(codes(&errors).contains(&"E007"), "Expected E007, got: {:?}", codes(&errors));
}

#[test]
fn lua52_goto_in_module() {
    let output = run_bundle("lua52", "goto_in_module", LuaVersion::Lua52, "main.lua")
        .expect("bundle failed");
    insta::assert_snapshot!("lua52_goto_in_module", output);
}

#[test]
fn lua53_bitwise_ops() {
    let output =
        run_bundle("lua53", "bitwise_ops", LuaVersion::Lua53, "main.lua").expect("bundle failed");
    insta::assert_snapshot!("lua53_bitwise_ops", output);
}

#[test]
fn lua54_const_close_attrs() {
    let output = run_bundle("lua54", "const_close_attrs", LuaVersion::Lua54, "main.lua")
        .expect("bundle failed");
    insta::assert_snapshot!("lua54_const_close_attrs", output);
}

#[test]
fn luau_entry_hot_comments_are_hoisted() {
    // Hot comments only apply before any code, so the entry module's
    // leading run must reach the very top of the bundle.
    let output =
        run_bundle("luau", "hot_comments", LuaVersion::Luau, "main.luau").expect("bundle failed");
    assert!(
        output.starts_with(
            "\
--!strict
--!native
"
        ),
        "entry hot comments must lead the bundle:\n{output}"
    );
}

#[test]
fn luau_dependency_hot_comments_warn_w006() {
    // A non-entry module's hot comments land mid-bundle where Luau
    // ignores them; W006 makes that visible.
    let result = run_bundle_with(
        "luau",
        "hot_comments",
        LuaVersion::Luau,
        "main.luau",
        DynamicRequire::default(),
    )
    .expect("bundle failed");
    assert!(
        located(&result.warnings).iter().any(|(path, warning)| {
            path.ends_with("util.luau")
                && warning.code == "W006"
                && warning.message.contains("--!nonstrict")
        }),
        "Expected W006 for util.luau, got: {:?}",
        codes(&result.warnings)
    );
}

#[test]
fn luau_export_type_is_stripped_inside_module_wrappers() {
    // `export type` is invalid below the top level; bundled module
    // bodies live inside loader functions, so the keyword is dropped
    // (the alias stays usable within its module).
    let output =
        run_bundle("luau", "export_type", LuaVersion::Luau, "main.luau").expect("bundle failed");
    assert!(
        !output.contains("export type"),
        "export must be stripped inside module wrappers:\n{output}"
    );
    assert!(output.contains("type Adder"), "the alias itself must survive:\n{output}");
    assert_reparses(&output, LuaVersion::Luau);
}

/// The bundle must name no directory on the build host. Provenance comments
/// and 5.2+ loader data stay project-relative, even for a module vendored
/// above the project root.
#[test]
fn emitted_paths_stay_project_relative() {
    let directory = tempfile::tempdir().expect("tempdir");
    let source_directory = directory.path().join("src");
    let shared_directory = directory.path().join("shared");
    fs::create_dir_all(&source_directory).expect("mkdir src");
    fs::create_dir_all(&shared_directory).expect("mkdir shared");
    fs::write(
        source_directory.join("main.lua"),
        "\
local util = require(\"util\")
return util.value
",
    )
    .expect("write main");
    fs::write(shared_directory.join("util.lua"), "return { value = 1 }\n").expect("write util");

    // Rooted at the entry's own directory, as `luck bundle src/main.lua` does,
    // so `shared/` lands above the root.
    let bundled = bundle_both(
        &source_directory.join("main.lua"),
        &BundleOptions {
            version: LuaVersion::Lua54,
            search_paths: &["../shared/?.lua".to_owned()],
            project_root: &source_directory,
            dynamic_require: DynamicRequire::default(),
        },
    )
    .expect("bundle failed");
    let output = text(&bundled.output);

    let native_root = directory.path().to_string_lossy().to_string();
    let slash_root = native_root.replace('\\', "/");
    assert!(
        !output.contains(&native_root) && !output.contains(&slash_root),
        "absolute path leaked into the bundle:\n{output}"
    );
    assert!(output.contains("../shared/util.lua"), "{output}");
}

fn temp_project(files: &[(&str, &[u8])]) -> io::Result<TempDir> {
    let directory = tempfile::tempdir()?;
    for (name, contents) in files {
        fs::write(directory.path().join(name), contents)?;
    }
    Ok(directory)
}

fn bundle_luau_project(directory: &Path) -> Result<BundleResult, Vec<FileDiagnostics>> {
    bundle_both(
        &directory.join("main.luau"),
        &BundleOptions {
            version: LuaVersion::Luau,
            search_paths: &[],
            project_root: directory,
            dynamic_require: DynamicRequire::default(),
        },
    )
}

fn bundle_lua_project(directory: &Path) -> Result<BundleResult, Vec<FileDiagnostics>> {
    bundle_both(
        &directory.join("main.lua"),
        &BundleOptions {
            version: LuaVersion::Lua54,
            search_paths: &["?.lua".to_owned()],
            project_root: directory,
            dynamic_require: DynamicRequire::default(),
        },
    )
}

const REQUIRE_UTIL: &str = "\
local util = require(\"./util\")
print(util)
";
const LUA_REQUIRE_UTIL: &str = "\
local util = require(\"util\")
print(util)
";
const RETURN_ONE: &[u8] = b"return 1\n";

fn assert_reparses(output: &str, version: LuaVersion) {
    let allocator = Allocator::new();
    let parsed = luck_parser::parse(&allocator, output.as_bytes(), version);
    assert!(parsed.errors.is_empty(), "bundle must reparse: {:?}\n{output}", parsed.errors);
}

#[test]
fn entry_shebang_moves_to_the_first_line_of_a_lua_bundle() {
    let main = format!(
        "\
#!/usr/bin/env lua
{LUA_REQUIRE_UTIL}"
    );
    let directory =
        temp_project(&[("main.lua", main.as_bytes()), ("util.lua", RETURN_ONE)]).expect("project");
    let output =
        String::from_utf8(bundle_lua_project(directory.path()).expect("bundle failed").output)
            .unwrap();
    assert!(
        output.starts_with(
            "\
#!/usr/bin/env lua
local __luck_modules={}
"
        ),
        "{output}"
    );
    assert_eq!(output.matches("#!").count(), 1, "{output}");
    assert_reparses(&output, LuaVersion::Lua54);
}

#[test]
fn entry_shebang_moves_to_the_first_line_of_a_luau_bundle() {
    let main = format!(
        "\
#!/usr/bin/env luau
{REQUIRE_UTIL}"
    );
    let directory = temp_project(&[("main.luau", main.as_bytes()), ("util.luau", RETURN_ONE)])
        .expect("project");
    let output =
        String::from_utf8(bundle_luau_project(directory.path()).expect("bundle failed").output)
            .unwrap();
    assert!(
        output.starts_with(
            "\
#!/usr/bin/env luau
local __luck_modules={}
"
        ),
        "{output}"
    );
    assert_eq!(output.matches("#!").count(), 1, "{output}");
    assert_reparses(&output, LuaVersion::Luau);
}

#[test]
fn first_line_comment_without_a_bang_moves_up_like_a_shebang() {
    let main = format!(
        "\
# not a shebang
{LUA_REQUIRE_UTIL}"
    );
    let directory =
        temp_project(&[("main.lua", main.as_bytes()), ("util.lua", RETURN_ONE)]).expect("project");
    let output =
        String::from_utf8(bundle_lua_project(directory.path()).expect("bundle failed").output)
            .unwrap();
    assert!(
        output.starts_with(
            "\
# not a shebang
local __luck_modules={}
"
        ),
        "{output}"
    );
    assert_reparses(&output, LuaVersion::Lua54);
}

#[test]
fn dependency_shebang_is_dropped() {
    let directory = temp_project(&[
        ("main.lua", LUA_REQUIRE_UTIL.as_bytes()),
        (
            "util.lua",
            b"\
#!/usr/bin/env lua
return 1
",
        ),
    ])
    .expect("project");
    let output =
        String::from_utf8(bundle_lua_project(directory.path()).expect("bundle failed").output)
            .unwrap();
    assert!(output.starts_with("local __luck_modules={}\n"), "{output}");
    assert!(!output.contains("#!"), "{output}");
    assert_reparses(&output, LuaVersion::Lua54);
}

#[test]
fn bom_before_an_entry_shebang_is_dropped() {
    let main = format!(
        "\
\u{FEFF}#!/usr/bin/env lua
{LUA_REQUIRE_UTIL}"
    );
    let directory =
        temp_project(&[("main.lua", main.as_bytes()), ("util.lua", RETURN_ONE)]).expect("project");
    let output =
        String::from_utf8(bundle_lua_project(directory.path()).expect("bundle failed").output)
            .unwrap();
    assert!(
        output.starts_with(
            "\
#!/usr/bin/env lua
local __luck_modules={}
"
        ),
        "{output}"
    );
    assert!(!output.contains('\u{FEFF}'), "{output}");
    assert_reparses(&output, LuaVersion::Lua54);
}

#[test]
fn bom_is_dropped_from_every_module_body() {
    let main = format!("\u{FEFF}{LUA_REQUIRE_UTIL}");
    let directory = temp_project(&[
        ("main.lua", main.as_bytes()),
        ("util.lua", "\u{FEFF}return 1\n".as_bytes()),
    ])
    .expect("project");
    let output =
        String::from_utf8(bundle_lua_project(directory.path()).expect("bundle failed").output)
            .unwrap();
    assert!(!output.contains('\u{FEFF}'), "{output}");
    assert_reparses(&output, LuaVersion::Lua54);
}

#[test]
fn lone_module_with_a_shebang_stays_verbatim() {
    let main = b"\
#!/usr/bin/env lua
print(1)
";
    let directory = temp_project(&[("main.lua", main)]).expect("project");
    let output =
        String::from_utf8(bundle_lua_project(directory.path()).expect("bundle failed").output)
            .unwrap();
    assert_eq!(output.as_bytes(), main);
}

#[test]
fn luau_hot_comment_after_a_block_comment_or_shebang_is_hoisted() {
    for (header, opening) in [
        (
            "\
--[[
block comment
]]
",
            "\
--!strict
local __luck_modules={}
",
        ),
        (
            "--[[ one line ]]\n",
            "\
--!strict
local __luck_modules={}
",
        ),
        (
            "#!/usr/bin/env luau\n",
            "\
#!/usr/bin/env luau
--!strict
local __luck_modules={}
",
        ),
    ] {
        let main = format!(
            "\
{header}--!strict
{REQUIRE_UTIL}"
        );
        let directory = temp_project(&[("main.luau", main.as_bytes()), ("util.luau", RETURN_ONE)])
            .expect("project");
        let output =
            String::from_utf8(bundle_luau_project(directory.path()).expect("bundle failed").output)
                .unwrap();
        assert!(output.starts_with(opening), "header {header:?}:\n{output}");
        assert_reparses(&output, LuaVersion::Luau);
    }
}

#[test]
fn luau_hot_comment_line_inside_a_block_comment_is_not_hoisted() {
    let main = format!(
        "\
--[[
--!strict
]]
{REQUIRE_UTIL}"
    );
    let directory = temp_project(&[("main.luau", main.as_bytes()), ("util.luau", RETURN_ONE)])
        .expect("project");
    let output =
        String::from_utf8(bundle_luau_project(directory.path()).expect("bundle failed").output)
            .unwrap();
    assert!(output.starts_with("local __luck_modules={}\n"), "{output}");
}

#[test]
fn luau_hot_comment_after_code_on_a_comment_line_is_not_hoisted() {
    let main = format!(
        "\
--[[x]] local y = 1
--!strict
{REQUIRE_UTIL}"
    );
    let directory = temp_project(&[("main.luau", main.as_bytes()), ("util.luau", RETURN_ONE)])
        .expect("project");
    let output =
        String::from_utf8(bundle_luau_project(directory.path()).expect("bundle failed").output)
            .unwrap();
    assert!(output.starts_with("local __luck_modules={}\n"), "{output}");
}

#[test]
fn luau_hoisted_hot_comment_drops_the_carriage_return() {
    let main = b"--!strict\r\nlocal util = require(\"./util\")\r\nprint(util)\r\n";
    let directory =
        temp_project(&[("main.luau", main), ("util.luau", RETURN_ONE)]).expect("project");
    let output =
        String::from_utf8(bundle_luau_project(directory.path()).expect("bundle failed").output)
            .unwrap();
    assert!(output.starts_with("--!strict\nlocal __luck_modules={}\n"), "{output}");
}

#[test]
fn luau_dependency_hot_comment_scan_matches_the_entry_scan() {
    for (util, expected_warnings) in [
        (
            "\
--[[
--!strict
]]
return 1
",
            0,
        ),
        (
            "\
--[[x]] local y = 1
--!strict
return 1
",
            0,
        ),
        (
            "\
--[[
block
]]
--!strict
return 1
",
            1,
        ),
    ] {
        let directory =
            temp_project(&[("main.luau", REQUIRE_UTIL.as_bytes()), ("util.luau", util.as_bytes())])
                .expect("project");
        let result = bundle_luau_project(directory.path()).expect("bundle failed");
        let w006_count = codes(&result.warnings).iter().filter(|&&code| code == "W006").count();
        assert_eq!(w006_count, expected_warnings, "util {util:?}: {:?}", result.warnings);
    }
}

/// A byte that isn't UTF-8 outside a string is one parse error, as Lua
/// reads it, which the module reports once however many modules require it.
#[test]
fn broken_dependency_reports_its_error_once_for_its_requirers() {
    let directory = temp_project(&[
        (
            "main.lua",
            b"\
require(\"a\")
require(\"b\")
",
        ),
        ("a.lua", b"require(\"bad\")\n"),
        ("b.lua", b"require(\"bad\")\n"),
        ("bad.lua", &[0xFF]),
    ])
    .expect("project");
    let errors = bundle_lua_project(directory.path()).expect_err("a parse error must not bundle");
    assert_eq!(codes(&errors), ["E008"], "{errors:?}");
}

#[test]
fn broken_dependency_required_twice_by_one_file_reports_its_error_once() {
    let directory = temp_project(&[
        (
            "main.lua",
            b"\
require(\"bad\")
require(\"bad\")
",
        ),
        ("bad.lua", &[0xFF]),
    ])
    .expect("project");
    let errors = bundle_lua_project(directory.path()).expect_err("a parse error must not bundle");
    assert_eq!(codes(&errors), ["E008"], "{errors:?}");
}

/// The modules' bytes reach the bundle as written, a string or a comment
/// that isn't UTF-8 included.
#[test]
fn bytes_that_are_not_utf8_reach_the_bundle_as_written() {
    let directory = temp_project(&[
        ("main.lua", b"-- caf\xE9\nreturn require(\"util\")\n"),
        ("util.lua", b"return '\xFF\xE9'\n"),
    ])
    .expect("project");
    let output = bundle_lua_project(directory.path()).expect("bundle").output;
    for written in [&b"-- caf\xE9\n"[..], b"return '\xFF\xE9'\n"] {
        assert!(output.windows(written.len()).any(|window| window == written), "{written:?}");
    }
}

#[test]
fn require_that_resolves_nowhere_flags_e004_in_each_module_making_it() {
    let directory = temp_project(&[
        (
            "main.lua",
            b"require(\"a\")
require(\"b\")
",
        ),
        ("a.lua", b"require(\"missing\")\n"),
        ("b.lua", b"require(\"missing\")\n"),
    ])
    .expect("project");
    let errors =
        bundle_lua_project(directory.path()).expect_err("an unresolved module must not bundle");
    assert_eq!(codes(&errors), ["E004", "E004"], "{errors:?}");
    let files: Vec<&str> =
        errors.iter().filter_map(|file| file.path.file_name()?.to_str()).collect();
    assert_eq!(files, ["a.lua", "b.lua"], "{errors:?}");
}

/// The canonical form of `directory`, as diagnostics write it.
fn slashed_root(directory: &TempDir) -> String {
    let canonical = FileSystemOs.canonicalize(directory.path()).expect("canonicalize");
    to_slash_lossy(&canonical).into_owned()
}

#[test]
fn unresolved_lua_require_lists_the_paths_it_searched() {
    let directory = temp_project(&[("main.lua", b"require(\"missing.mod\")\n")]).expect("project");
    let errors =
        bundle_lua_project(directory.path()).expect_err("an unresolved module must not bundle");
    let located = located(&errors);
    let [(path, error)] = located.as_slice() else {
        panic!("expected one error: {errors:?}");
    };
    assert_eq!(error.code, "E004");
    assert_eq!(*path, format!("{}/main.lua", slashed_root(&directory)));
    assert_eq!(error.message, "module not found: `missing.mod`");
    assert_eq!(
        error.help.as_deref(),
        Some(
            "\
searched paths:
  - `missing/mod.lua`"
        )
    );
}

#[test]
fn unresolved_luau_require_lists_the_four_files_it_could_be() {
    let directory = temp_project(&[("main.luau", b"require(\"./missing\")\n")]).expect("project");
    let errors =
        bundle_luau_project(directory.path()).expect_err("an unresolved module must not bundle");
    let located = located(&errors);
    let [(_, error)] = located.as_slice() else {
        panic!("expected one error: {errors:?}");
    };
    let base = format!("{}/./missing", slashed_root(&directory));
    assert_eq!(error.code, "E004");
    assert_eq!(
        error.help.as_deref(),
        Some(&*format!(
            "\
searched paths:
  - `{base}.luau`
  - `{base}.lua`
  - `{base}/init.luau`
  - `{base}/init.lua`"
        ))
    );
}

#[test]
fn ambiguous_luau_require_names_both_files() {
    let directory = temp_project(&[
        ("main.luau", b"require(\"./util\")\n"),
        ("util.luau", RETURN_ONE),
        ("util.lua", RETURN_ONE),
    ])
    .expect("project");
    let errors =
        bundle_luau_project(directory.path()).expect_err("an ambiguous module must not bundle");
    let located = located(&errors);
    let [(_, error)] = located.as_slice() else {
        panic!("expected one error: {errors:?}");
    };
    let root = slashed_root(&directory);
    assert_eq!(error.code, "E007");
    assert_eq!(error.message, "ambiguous module resolution");
    assert_eq!(
        error.help.as_deref(),
        Some(&*format!(
            "ambiguity: both `{root}/util.luau` and `{root}/util.lua` exist, so remove one of \
             the conflicting files"
        ))
    );
}

#[test]
fn luaurc_alias_named_self_warns_w004_where_self_is_required() {
    let directory = temp_project(&[
        ("main.luau", b"return require(\"./pkg\")\n"),
        (".luaurc", br#"{"aliases":{"self":"./elsewhere"}}"#),
    ])
    .expect("project");
    let package = directory.path().join("pkg");
    fs::create_dir_all(&package).expect("mkdir pkg");
    fs::write(package.join("init.luau"), "return require(\"@self/x\")\n").expect("write init");
    fs::write(package.join("x.luau"), RETURN_ONE).expect("write x");
    let warnings = bundle_luau_project(directory.path()).expect("bundle failed").warnings;
    let located = located(&warnings);
    let [(path, warning)] = located.as_slice() else {
        panic!("expected one warning: {warnings:?}");
    };
    assert_eq!(warning.code, "W004");
    assert_eq!(*path, format!("{}/pkg/init.luau", slashed_root(&directory)));
}

#[test]
fn oversized_dependency_reports_e012_once_for_its_requirers() {
    let oversized = vec![b'-'; 10 * 1024 * 1024 + 1];
    let directory = temp_project(&[
        (
            "main.lua",
            b"\
require(\"a\")
require(\"b\")
",
        ),
        ("a.lua", b"require(\"huge\")\n"),
        ("b.lua", b"require(\"huge\")\n"),
        ("huge.lua", &oversized),
    ])
    .expect("project");
    let errors =
        bundle_lua_project(directory.path()).expect_err("an oversized file must not bundle");
    assert_eq!(codes(&errors), ["E012"], "{errors:?}");
}

/// Eight directories of twenty modules, so threads race to resolve into the
/// same directories.
///
/// Each module requires every module of its own directory, the first of the
/// next, and, when `has_missing_requires`, one that doesn't exist, while the
/// entry requires few.
fn wide_project(version: LuaVersion, has_missing_requires: bool) -> io::Result<TempDir> {
    let directory = tempfile::tempdir()?;
    let extension = if version.is_luau() { "luau" } else { "lua" };
    // Luau's `./` is the requiring module's directory, which is the root only
    // for `main`.
    let require = |up: &str, package: &str, module: &str| {
        if version.is_luau() {
            format!("require(\"{up}/{package}/{module}\")\n")
        } else {
            format!("require(\"{package}.{module}\")\n")
        }
    };
    let packages = ["a", "b", "c", "d", "e", "f", "g", "h"];
    let modules: Vec<String> = (0..20).map(|module| format!("m{module}")).collect();
    let main: String = packages.iter().map(|package| require(".", package, "m0")).collect();
    fs::write(directory.path().join(format!("main.{extension}")), main)?;
    for (package, next_package) in packages.iter().zip(packages.iter().cycle().skip(1)) {
        let package_directory = directory.path().join(package);
        fs::create_dir_all(&package_directory)?;
        for module in &modules {
            let mut source: String =
                modules.iter().map(|sibling| require("..", package, sibling)).collect();
            source.push_str(&require("..", next_package, "m0"));
            if has_missing_requires {
                source.push_str(&require("..", package, "missing"));
            }
            source.push_str("return {}\n");
            fs::write(package_directory.join(format!("{module}.{extension}")), source)?;
        }
    }
    Ok(directory)
}

#[test]
fn threads_sharing_one_resolver_find_what_one_thread_does() {
    for version in [LuaVersion::Lua54, LuaVersion::Luau] {
        for has_missing_requires in [false, true] {
            let directory = wide_project(version, has_missing_requires).expect("project");
            let entry_name = if version.is_luau() { "main.luau" } else { "main.lua" };
            let entry = directory.path().join(entry_name);
            let options = BundleOptions {
                version,
                search_paths: &[],
                project_root: directory.path(),
                dynamic_require: DynamicRequire::default(),
            };
            let bundle = || luck_bundler::bundle(&ThreadAllocators::new(), &entry, &options);
            let one_thread = on_pool(1, bundle);
            match &one_thread {
                Ok(bundled) => assert_eq!(bundled.source_files.len(), 161, "{version:?}"),
                Err(errors) => assert_eq!(codes(errors), ["E004"; 160], "{version:?}"),
            }
            assert_eq!(one_thread.is_err(), has_missing_requires, "{version:?}");
            for _ in 0..4 {
                let threaded = on_pool(16, bundle);
                assert_eq!(format!("{threaded:?}"), format!("{one_thread:?}"), "{version:?}");
            }
        }
    }
}

#[test]
fn dependency_targets_index_the_modules_they_resolve_to() {
    let directory = temp_project(&[
        ("main.lua", b"require(\"a\")\n"),
        (
            "a.lua",
            b"\
require(\"b\")
require(\"b\")
",
        ),
        ("b.lua", RETURN_ONE),
    ])
    .expect("project");
    let arenas = ThreadAllocators::new();
    let search_paths = ["?.lua".to_owned()];
    let options = BundleOptions {
        version: LuaVersion::Lua54,
        search_paths: &search_paths,
        project_root: directory.path(),
        dynamic_require: DynamicRequire::default(),
    };
    let graph =
        off_pool(|| DependencyGraph::build(&arenas, &directory.path().join("main.lua"), &options))
            .expect("graph failed");

    let relative_paths: Vec<&str> =
        graph.modules.iter().map(|module| module.relative_path.as_str()).collect();
    assert_eq!(relative_paths, ["main.lua", "a.lua", "b.lua"]);
    let targets = |i: usize| -> Vec<&str> {
        graph.modules[i]
            .dependencies
            .iter()
            .map(|dependency| graph.modules[dependency.target.0].relative_path.as_str())
            .collect()
    };
    assert_eq!(targets(0), ["a.lua"]);
    assert_eq!(targets(1), ["b.lua", "b.lua"]);
    assert_eq!(targets(2), Vec::<&str>::new());
}
