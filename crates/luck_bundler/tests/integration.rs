use luck_bundler::bundle;
use luck_core::types::{DynamicRequire, LuaTarget};
use std::path::PathBuf;

fn fixture_dir(target_dir: &str, name: &str) -> PathBuf {
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    PathBuf::from(manifest_dir)
        .join("../../tests/fixtures")
        .join(target_dir)
        .join(name)
        .join("input")
}

fn run_bundle(
    target_dir: &str,
    name: &str,
    target: LuaTarget,
    entry_name: &str,
) -> Result<String, Vec<luck_core::diagnostics::Diagnostic>> {
    run_bundle_with(
        target_dir,
        name,
        target,
        entry_name,
        DynamicRequire::default(),
    )
    .map(|result| result.output)
}

fn run_bundle_with(
    target_dir: &str,
    name: &str,
    target: LuaTarget,
    entry_name: &str,
    dynamic_require: DynamicRequire,
) -> Result<luck_bundler::BundleResult, Vec<luck_core::diagnostics::Diagnostic>> {
    let input_dir = fixture_dir(target_dir, name);
    let entry = input_dir.join(entry_name);
    let search_paths = if target.is_luau() {
        vec![]
    } else {
        vec!["?.lua".to_string(), "?/init.lua".to_string()]
    };
    bundle(&entry, target, &search_paths, &input_dir, dynamic_require)
}

#[test]
fn basic_bundle() {
    let output =
        run_bundle("lua54", "basic_bundle", LuaTarget::Lua54, "main.lua").expect("bundle failed");
    insta::assert_snapshot!("basic_bundle", output);
}

#[test]
fn nested_deps() {
    let output =
        run_bundle("lua54", "nested_deps", LuaTarget::Lua54, "main.lua").expect("bundle failed");
    insta::assert_snapshot!("nested_deps", output);
}

#[test]
fn diamond_deps() {
    let output =
        run_bundle("lua54", "diamond_deps", LuaTarget::Lua54, "main.lua").expect("bundle failed");
    insta::assert_snapshot!("diamond_deps", output);
}

#[test]
fn deep_chain() {
    let output =
        run_bundle("lua54", "deep_chain", LuaTarget::Lua54, "main.lua").expect("bundle failed");
    insta::assert_snapshot!("deep_chain", output);
}

#[test]
fn init_module() {
    let output =
        run_bundle("lua54", "init_module", LuaTarget::Lua54, "main.lua").expect("bundle failed");
    insta::assert_snapshot!("init_module", output);
}

#[test]
fn multiple_requires() {
    let output = run_bundle("lua54", "multiple_requires", LuaTarget::Lua54, "main.lua")
        .expect("bundle failed");
    insta::assert_snapshot!("multiple_requires", output);
}

#[test]
fn module_required_by_many() {
    let output = run_bundle(
        "lua54",
        "module_required_by_many",
        LuaTarget::Lua54,
        "main.lua",
    )
    .expect("bundle failed");
    insta::assert_snapshot!("module_required_by_many", output);
}

#[test]
fn nil_return() {
    let output =
        run_bundle("lua54", "nil_return", LuaTarget::Lua54, "main.lua").expect("bundle failed");
    insta::assert_snapshot!("nil_return", output);
}

#[test]
fn no_deps() {
    let output =
        run_bundle("lua54", "no_deps", LuaTarget::Lua54, "main.lua").expect("bundle failed");
    insta::assert_snapshot!("no_deps", output);
}

#[test]
fn circular_dep_bundles_with_warning() {
    // The lazy loader lets a cycle bundle; a W003 warning flags the risk.
    let input_dir = fixture_dir("lua54", "errors/circular_dep");
    let entry = input_dir.join("a.lua");
    let result = luck_bundler::bundle(
        &entry,
        LuaTarget::Lua54,
        &["?.lua".to_string(), "?/init.lua".to_string()],
        &input_dir,
        DynamicRequire::default(),
    )
    .expect("cycle must bundle");
    assert!(
        result.warnings.iter().any(|w| w.code == "W003"),
        "Expected W003 warning, got: {:?}",
        result.warnings.iter().map(|w| &w.code).collect::<Vec<_>>()
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
fn unresolved_module() {
    let result = run_bundle(
        "lua54",
        "errors/unresolved_module",
        LuaTarget::Lua54,
        "main.lua",
    );
    assert!(result.is_err());
    let errors = result.unwrap_err();
    assert!(
        errors.iter().any(|e| e.code == "E004"),
        "Expected E004, got: {:?}",
        errors.iter().map(|e| &e.code).collect::<Vec<_>>()
    );
}

#[test]
fn non_literal_require_errors_in_error_mode() {
    let result = run_bundle_with(
        "lua54",
        "errors/non_literal_require",
        LuaTarget::Lua54,
        "main.lua",
        DynamicRequire::Error,
    );
    assert!(result.is_err());
    let errors = result.unwrap_err();
    assert!(
        errors.iter().any(|e| e.code == "E002"),
        "Expected E002, got: {:?}",
        errors.iter().map(|e| &e.code).collect::<Vec<_>>()
    );
}

/// The default: the bundle is produced, the call is retargeted at the
/// loader's dynamic entry point, and W007 says so.
#[test]
fn dynamic_require_bundles_through_the_loader() {
    let result = run_bundle_with(
        "lua54",
        "dynamic_require",
        LuaTarget::Lua54,
        "main.lua",
        DynamicRequire::default(),
    )
    .expect("a dynamic require must not abort the bundle");

    assert!(
        result.warnings.iter().any(|w| w.code == "W007"),
        "Expected W007, got: {:?}",
        result.warnings.iter().map(|w| &w.code).collect::<Vec<_>>()
    );
    insta::assert_snapshot!("dynamic_require", result.output);
}

#[test]
fn dynamic_require_in_allow_mode_is_silent() {
    let result = run_bundle_with(
        "lua54",
        "dynamic_require",
        LuaTarget::Lua54,
        "main.lua",
        DynamicRequire::Allow,
    )
    .expect("bundle failed");
    assert!(
        result.warnings.is_empty(),
        "expected no warnings, got: {:?}",
        result.warnings.iter().map(|w| &w.code).collect::<Vec<_>>()
    );
    assert!(
        result.output.contains("__luck_dynamic("),
        "{}",
        result.output
    );
}

#[test]
fn bare_require_bundles() {
    // Side-effect imports (`require("x")` as a statement) are legal.
    let output = run_bundle("lua54", "errors/bare_require", LuaTarget::Lua54, "main.lua")
        .expect("bare require must bundle");
    assert!(output.contains("__luck_require"), "{output}");
}

#[test]
fn require_after_code_bundles() {
    // Requires are position-independent with the lazy loader.
    let output = run_bundle(
        "lua54",
        "errors/require_after_code",
        LuaTarget::Lua54,
        "main.lua",
    )
    .expect("require after code must bundle");
    assert!(output.contains("__luck_require"), "{output}");
}

#[test]
fn package_loaded_manip_is_allowed_on_lua_targets() {
    // The bundle cache IS package.loaded on Lua targets, so preseeding
    // an entry behaves exactly as in real Lua, so no E006.
    let output = run_bundle(
        "lua54",
        "errors/package_loaded_manip",
        LuaTarget::Lua54,
        "main.lua",
    )
    .expect("package.loaded writes bundle fine on Lua targets");
    assert!(output.contains("package.loaded[\"x\"] = {}"), "{output}");
}

#[test]
fn package_loaded_manip_flags_e006_on_luau() {
    let result = run_bundle(
        "luau",
        "errors/package_loaded_manip",
        LuaTarget::Luau,
        "main.luau",
    );
    assert!(result.is_err());
    let errors = result.unwrap_err();
    assert!(
        errors.iter().any(|e| e.code == "E006"),
        "Expected E006, got: {:?}",
        errors.iter().map(|e| &e.code).collect::<Vec<_>>()
    );
}

#[test]
fn luau_relative_require() {
    let output = run_bundle("luau", "relative_require", LuaTarget::Luau, "main.luau")
        .expect("bundle failed");
    insta::assert_snapshot!("luau_relative_require", output);
}

#[test]
fn luau_alias_require() {
    let output = run_bundle("luau", "alias_require", LuaTarget::Luau, "src/main.luau")
        .expect("bundle failed");
    insta::assert_snapshot!("luau_alias_require", output);
}

#[test]
fn luau_luaurc_inheritance() {
    let output = run_bundle(
        "luau",
        "luaurc_inheritance",
        LuaTarget::Luau,
        "src/deep/nested/mod.luau",
    )
    .expect("bundle failed");
    insta::assert_snapshot!("luau_luaurc_inheritance", output);
}

#[test]
fn luau_init_luau() {
    let output =
        run_bundle("luau", "init_luau", LuaTarget::Luau, "main.luau").expect("bundle failed");
    insta::assert_snapshot!("luau_init_luau", output);
}

#[test]
fn luau_type_annotations() {
    let output = run_bundle("luau", "type_annotations", LuaTarget::Luau, "main.luau")
        .expect("bundle failed");
    insta::assert_snapshot!("luau_type_annotations", output);
}

#[test]
fn luau_string_interpolation() {
    let output = run_bundle("luau", "string_interpolation", LuaTarget::Luau, "main.luau")
        .expect("bundle failed");
    insta::assert_snapshot!("luau_string_interpolation", output);
}

#[test]
fn luau_ambiguous_ext() {
    let result = run_bundle(
        "luau",
        "errors/luau_ambiguous_ext",
        LuaTarget::Luau,
        "main.luau",
    );
    assert!(result.is_err());
    let errors = result.unwrap_err();
    assert!(
        errors.iter().any(|e| e.code == "E007"),
        "Expected E007, got: {:?}",
        errors.iter().map(|e| &e.code).collect::<Vec<_>>()
    );
}

#[test]
fn lua52_goto_in_module() {
    let output =
        run_bundle("lua52", "goto_in_module", LuaTarget::Lua52, "main.lua").expect("bundle failed");
    insta::assert_snapshot!("lua52_goto_in_module", output);
}

#[test]
fn lua53_bitwise_ops() {
    let output =
        run_bundle("lua53", "bitwise_ops", LuaTarget::Lua53, "main.lua").expect("bundle failed");
    insta::assert_snapshot!("lua53_bitwise_ops", output);
}

#[test]
fn lua54_const_close_attrs() {
    let output = run_bundle("lua54", "const_close_attrs", LuaTarget::Lua54, "main.lua")
        .expect("bundle failed");
    insta::assert_snapshot!("lua54_const_close_attrs", output);
}

#[test]
fn luau_hot_comments_hoisted() {
    // Hot comments only apply before any code, so the entry module's
    // leading run must reach the very top of the bundle.
    let output =
        run_bundle("luau", "hot_comments", LuaTarget::Luau, "main.luau").expect("bundle failed");
    assert!(
        output.starts_with("--!strict\n--!native\n"),
        "entry hot comments must lead the bundle:\n{output}"
    );
}

#[test]
fn luau_dependency_hot_comments_warn_w006() {
    // A non-entry module's hot comments land mid-bundle where Luau
    // ignores them; W006 makes that visible.
    let input_dir = fixture_dir("luau", "hot_comments");
    let entry = input_dir.join("main.luau");
    let result = luck_bundler::bundle(
        &entry,
        LuaTarget::Luau,
        &[],
        &input_dir,
        DynamicRequire::default(),
    )
    .expect("bundle failed");
    assert!(
        result
            .warnings
            .iter()
            .any(|w| w.code == "W006" && w.message.contains("--!nonstrict")),
        "Expected W006 for util.luau, got: {:?}",
        result.warnings.iter().map(|w| &w.code).collect::<Vec<_>>()
    );
}

#[test]
fn luau_export_type_stripped_in_thunks() {
    // `export type` is invalid below the top level; bundled module
    // bodies live inside loader functions, so the keyword is dropped
    // (the alias stays usable within its module).
    let output =
        run_bundle("luau", "export_type", LuaTarget::Luau, "main.luau").expect("bundle failed");
    assert!(
        !output.contains("export type"),
        "export must be stripped inside thunks:\n{output}"
    );
    assert!(
        output.contains("type Adder"),
        "the alias itself must survive:\n{output}"
    );
    let reparsed = luck_parser::parse(&output, luck_token::LuaVersion::Luau);
    assert!(
        reparsed.errors.is_empty(),
        "bundle must reparse: {:?}",
        reparsed.errors
    );
}

/// Nothing the bundle carries may name a directory on the build host: the
/// provenance comments and the 5.2+ loader data are all project-relative,
/// including for a module vendored above the project root.
#[test]
fn emitted_paths_stay_project_relative() {
    let dir = tempfile::tempdir().expect("tempdir");
    let src = dir.path().join("src");
    let shared = dir.path().join("shared");
    std::fs::create_dir_all(&src).expect("mkdir src");
    std::fs::create_dir_all(&shared).expect("mkdir shared");
    std::fs::write(
        src.join("main.lua"),
        "local util = require(\"util\")\nreturn util.value\n",
    )
    .expect("write main");
    std::fs::write(shared.join("util.lua"), "return { value = 1 }\n").expect("write util");

    // Rooted at the entry's own directory, as `luck bundle src/main.lua` does,
    // so `shared/` lands above the root.
    let output = bundle(
        &src.join("main.lua"),
        LuaTarget::Lua54,
        &["../shared/?.lua".to_string()],
        &src,
        DynamicRequire::default(),
    )
    .expect("bundle failed")
    .output;

    let native_root = dir.path().to_string_lossy().to_string();
    let slash_root = native_root.replace('\\', "/");
    assert!(
        !output.contains(&native_root) && !output.contains(&slash_root),
        "absolute path leaked into the bundle:\n{output}"
    );
    assert!(output.contains("../shared/util.lua"), "{output}");
}
