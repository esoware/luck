use luck_bundler::bundle;
use luck_core::types::LuaTarget;
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
    let input_dir = fixture_dir(target_dir, name);
    let entry = input_dir.join(entry_name);
    let search_paths = if target.is_luau() {
        vec![]
    } else {
        vec!["?.lua".to_string(), "?/init.lua".to_string()]
    };
    let result = bundle(&entry, target, &search_paths, &input_dir)?;
    Ok(result.output)
}

#[test]
fn test_basic_bundle() {
    let output =
        run_bundle("lua54", "basic_bundle", LuaTarget::Lua54, "main.lua").expect("bundle failed");
    insta::assert_snapshot!("basic_bundle", output);
}

#[test]
fn test_nested_deps() {
    let output =
        run_bundle("lua54", "nested_deps", LuaTarget::Lua54, "main.lua").expect("bundle failed");
    insta::assert_snapshot!("nested_deps", output);
}

#[test]
fn test_diamond_deps() {
    let output =
        run_bundle("lua54", "diamond_deps", LuaTarget::Lua54, "main.lua").expect("bundle failed");
    insta::assert_snapshot!("diamond_deps", output);
}

#[test]
fn test_deep_chain() {
    let output =
        run_bundle("lua54", "deep_chain", LuaTarget::Lua54, "main.lua").expect("bundle failed");
    insta::assert_snapshot!("deep_chain", output);
}

#[test]
fn test_init_module() {
    let output =
        run_bundle("lua54", "init_module", LuaTarget::Lua54, "main.lua").expect("bundle failed");
    insta::assert_snapshot!("init_module", output);
}

#[test]
fn test_multiple_requires() {
    let output = run_bundle("lua54", "multiple_requires", LuaTarget::Lua54, "main.lua")
        .expect("bundle failed");
    insta::assert_snapshot!("multiple_requires", output);
}

#[test]
fn test_module_required_by_many() {
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
fn test_nil_return() {
    let output =
        run_bundle("lua54", "nil_return", LuaTarget::Lua54, "main.lua").expect("bundle failed");
    insta::assert_snapshot!("nil_return", output);
}

#[test]
fn test_no_deps() {
    let output =
        run_bundle("lua54", "no_deps", LuaTarget::Lua54, "main.lua").expect("bundle failed");
    insta::assert_snapshot!("no_deps", output);
}

#[test]
fn test_circular_dep_bundles_with_warning() {
    // Cycles bundle now (lazy loader); a W003 warning flags the risk.
    let input_dir = fixture_dir("lua54", "errors/circular_dep");
    let entry = input_dir.join("a.lua");
    let result = luck_bundler::bundle(
        &entry,
        LuaTarget::Lua54,
        &["?.lua".to_string(), "?/init.lua".to_string()],
        &input_dir,
    )
    .expect("cycle must bundle");
    assert!(
        result.warnings.iter().any(|w| w.code == "W003"),
        "Expected W003 warning, got: {:?}",
        result.warnings.iter().map(|w| &w.code).collect::<Vec<_>>()
    );
    assert!(result.output.contains("__luck_require"));
}

#[test]
fn test_unresolved_module() {
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
fn test_non_literal_require() {
    let result = run_bundle(
        "lua54",
        "errors/non_literal_require",
        LuaTarget::Lua54,
        "main.lua",
    );
    assert!(result.is_err());
    let errors = result.unwrap_err();
    assert!(
        errors.iter().any(|e| e.code == "E002"),
        "Expected E002, got: {:?}",
        errors.iter().map(|e| &e.code).collect::<Vec<_>>()
    );
}

#[test]
fn test_bare_require_bundles() {
    // Side-effect imports (`require("x")` as a statement) are legal now.
    let output = run_bundle("lua54", "errors/bare_require", LuaTarget::Lua54, "main.lua")
        .expect("bare require must bundle");
    assert!(output.contains("__luck_require"), "{output}");
}

#[test]
fn test_require_after_code_bundles() {
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
fn test_package_loaded_manip() {
    let result = run_bundle(
        "lua54",
        "errors/package_loaded_manip",
        LuaTarget::Lua54,
        "main.lua",
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
fn test_luau_relative_require() {
    let output = run_bundle("luau", "relative_require", LuaTarget::Luau, "main.luau")
        .expect("bundle failed");
    insta::assert_snapshot!("luau_relative_require", output);
}

#[test]
fn test_luau_alias_require() {
    let output = run_bundle("luau", "alias_require", LuaTarget::Luau, "src/main.luau")
        .expect("bundle failed");
    insta::assert_snapshot!("luau_alias_require", output);
}

#[test]
fn test_luau_luaurc_inheritance() {
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
fn test_luau_init_luau() {
    let output =
        run_bundle("luau", "init_luau", LuaTarget::Luau, "main.luau").expect("bundle failed");
    insta::assert_snapshot!("luau_init_luau", output);
}

#[test]
fn test_luau_type_annotations() {
    let output = run_bundle("luau", "type_annotations", LuaTarget::Luau, "main.luau")
        .expect("bundle failed");
    insta::assert_snapshot!("luau_type_annotations", output);
}

#[test]
fn test_luau_string_interpolation() {
    let output = run_bundle("luau", "string_interpolation", LuaTarget::Luau, "main.luau")
        .expect("bundle failed");
    insta::assert_snapshot!("luau_string_interpolation", output);
}

#[test]
fn test_luau_ambiguous_ext() {
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
fn test_lua52_goto_in_module() {
    let output =
        run_bundle("lua52", "goto_in_module", LuaTarget::Lua52, "main.lua").expect("bundle failed");
    insta::assert_snapshot!("lua52_goto_in_module", output);
}

#[test]
fn test_lua53_bitwise_ops() {
    let output =
        run_bundle("lua53", "bitwise_ops", LuaTarget::Lua53, "main.lua").expect("bundle failed");
    insta::assert_snapshot!("lua53_bitwise_ops", output);
}

#[test]
fn test_lua54_const_close_attrs() {
    let output = run_bundle("lua54", "const_close_attrs", LuaTarget::Lua54, "main.lua")
        .expect("bundle failed");
    insta::assert_snapshot!("lua54_const_close_attrs", output);
}
