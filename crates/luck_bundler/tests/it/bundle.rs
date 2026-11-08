#![expect(
    clippy::missing_panics_doc,
    reason = "a test panics to fail, so a Panics section says nothing"
)]

use std::fs;
use std::path::PathBuf;

use luck_bundler::{BundleResult, bundle};
use luck_core::{Diagnostic, DynamicRequire};
use luck_token::LuaVersion;

fn fixture_directory(dialect: &str, name: &str) -> PathBuf {
    let manifest_directory = env!("CARGO_MANIFEST_DIR");
    PathBuf::from(manifest_directory)
        .join("../../tests/fixtures")
        .join(dialect)
        .join(name)
        .join("input")
}

/// # Errors
///
/// Returns the diagnostics that stopped the fixture from bundling.
fn run_bundle(
    dialect: &str,
    name: &str,
    version: LuaVersion,
    entry_name: &str,
) -> Result<String, Vec<Diagnostic>> {
    run_bundle_with(dialect, name, version, entry_name, DynamicRequire::default())
        .map(|result| result.output)
}

/// # Errors
///
/// Returns the diagnostics that stopped the fixture from bundling.
fn run_bundle_with(
    dialect: &str,
    name: &str,
    version: LuaVersion,
    entry_name: &str,
    dynamic_require: DynamicRequire,
) -> Result<BundleResult, Vec<Diagnostic>> {
    let input_directory = fixture_directory(dialect, name);
    let entry = input_directory.join(entry_name);
    let search_paths =
        if version.is_luau() { vec![] } else { vec!["?.lua".to_owned(), "?/init.lua".to_owned()] };
    bundle(&entry, version, &search_paths, &input_directory, dynamic_require)
}

fn codes(diagnostics: &[Diagnostic]) -> Vec<&str> {
    diagnostics.iter().map(|diagnostic| diagnostic.code).collect()
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
    assert!(result.output.contains("__luck_require"));
    // The cycle runs through the entry: its function must register under
    // the require string so the bundle stays self-contained.
    assert!(
        result.output.contains("__luck_modules[\"a\"]=__luck_entry"),
        "entry must register when required:\n{}",
        result.output
    );
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
    insta::assert_snapshot!("dynamic_require", result.output);
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
    assert!(result.output.contains("__luck_dynamic("), "{}", result.output);
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
fn package_loaded_manip_is_allowed_on_lua_targets() {
    // On Lua targets the bundle cache is package.loaded itself, so
    // preseeding an entry behaves as in real Lua and needs no E006.
    let output = run_bundle("lua54", "errors/package_loaded_manip", LuaVersion::Lua54, "main.lua")
        .expect("package.loaded writes bundle fine on Lua targets");
    assert!(output.contains("package.loaded[\"x\"] = {}"), "{output}");
}

#[test]
fn package_loaded_manip_flags_e006_on_luau() {
    let errors = run_bundle("luau", "errors/package_loaded_manip", LuaVersion::Luau, "main.luau")
        .expect_err("a package.loaded write must not bundle on Luau");
    assert!(codes(&errors).contains(&"E006"), "Expected E006, got: {:?}", codes(&errors));
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
        output.starts_with("--!strict\n--!native\n"),
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
        result
            .warnings
            .iter()
            .any(|warning| warning.code == "W006" && warning.message.contains("--!nonstrict")),
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
    let reparsed = luck_parser::parse(&output, LuaVersion::Luau);
    assert!(reparsed.errors.is_empty(), "bundle must reparse: {:?}", reparsed.errors);
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
        "local util = require(\"util\")\nreturn util.value\n",
    )
    .expect("write main");
    fs::write(shared_directory.join("util.lua"), "return { value = 1 }\n").expect("write util");

    // Rooted at the entry's own directory, as `luck bundle src/main.lua` does,
    // so `shared/` lands above the root.
    let output = bundle(
        &source_directory.join("main.lua"),
        LuaVersion::Lua54,
        &["../shared/?.lua".to_owned()],
        &source_directory,
        DynamicRequire::default(),
    )
    .expect("bundle failed")
    .output;

    let native_root = directory.path().to_string_lossy().to_string();
    let slash_root = native_root.replace('\\', "/");
    assert!(
        !output.contains(&native_root) && !output.contains(&slash_root),
        "absolute path leaked into the bundle:\n{output}"
    );
    assert!(output.contains("../shared/util.lua"), "{output}");
}
