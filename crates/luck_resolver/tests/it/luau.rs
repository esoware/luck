use luck_core::Diagnostic;
use luck_resolver::{ResolvedModule, Resolver};
use luck_token::LuaVersion;
use tempfile::TempDir;

use crate::common::{luau_project, normalized, request, write_files};

/// Resolves `module` as required from `requiring_file`, a path relative to
/// `project`.
///
/// # Errors
///
/// Returns the resolver's diagnostic when `module` doesn't resolve.
fn resolve_luau(
    project: &TempDir,
    requiring_file: &str,
    module: &str,
) -> Result<ResolvedModule, Diagnostic> {
    let from_file = normalized(project, requiring_file);
    Resolver::new(LuaVersion::Luau, &[], project.path()).resolve(&request(module, &from_file))
}

#[test]
fn resolves_relative_luau_file() {
    let project = luau_project();
    let resolved = resolve_luau(&project, "src/main.luau", "./utils").expect("resolve ./utils");
    assert_eq!(resolved.path, normalized(&project, "src/utils.luau"));
}

#[test]
fn resolves_lua_file_when_only_lua_exists() {
    let project = luau_project();
    let resolved = resolve_luau(&project, "src/main.luau", "./helper").expect("resolve ./helper");
    assert_eq!(resolved.path, normalized(&project, "src/helper.lua"));
}

#[test]
fn reports_e007_when_luau_and_lua_files_both_exist() {
    let project = luau_project();
    write_files(project.path(), &[("src/both.luau", "return {}"), ("src/both.lua", "return {}")]);
    let error = resolve_luau(&project, "src/main.luau", "./both").expect_err("ambiguous");
    assert_eq!(error.code, "E007");
    let expected_help = format!(
        "ambiguity: both `{}` and `{}` exist, so remove one of the conflicting files",
        normalized(&project, "src/both.luau"),
        normalized(&project, "src/both.lua")
    );
    assert_eq!(error.help, Some(expected_help));
}

#[test]
fn resolves_directory_module_via_init_luau() {
    let project = luau_project();
    write_files(project.path(), &[("src/components/init.luau", "return {}")]);
    let resolved =
        resolve_luau(&project, "src/main.luau", "./components").expect("resolve ./components");
    assert_eq!(resolved.path, normalized(&project, "src/components/init.luau"));
}

#[test]
fn resolves_directory_module_via_init_lua() {
    let project = luau_project();
    write_files(project.path(), &[("src/legacy/init.lua", "return {}")]);
    let resolved = resolve_luau(&project, "src/main.luau", "./legacy").expect("resolve ./legacy");
    assert_eq!(resolved.path, normalized(&project, "src/legacy/init.lua"));
}

#[test]
fn reports_e007_when_file_and_directory_module_both_exist() {
    let project = luau_project();
    write_files(
        project.path(),
        &[("src/widget.luau", "return {}"), ("src/widget/init.luau", "return {}")],
    );
    let error = resolve_luau(&project, "src/main.luau", "./widget").expect_err("ambiguous");
    assert_eq!(error.code, "E007");
    let expected_help = format!(
        "ambiguity: both file `{}` and directory init file `{}` exist, so remove one of the \
         conflicting files",
        normalized(&project, "src/widget.luau"),
        normalized(&project, "src/widget/init.luau")
    );
    assert_eq!(error.help, Some(expected_help));
}

#[test]
fn reports_e007_when_init_luau_and_init_lua_both_exist() {
    let project = luau_project();
    write_files(
        project.path(),
        &[("src/ambig/init.luau", "return {}"), ("src/ambig/init.lua", "return {}")],
    );
    let error = resolve_luau(&project, "src/main.luau", "./ambig").expect_err("ambiguous");
    assert_eq!(error.code, "E007");
    let expected_help = format!(
        "ambiguity: both `{}` and `{}` exist, so remove one of the conflicting files",
        normalized(&project, "src/ambig/init.luau"),
        normalized(&project, "src/ambig/init.lua")
    );
    assert_eq!(error.help, Some(expected_help));
}

/// Missing candidates aren't canonicalized, so they keep the require's
/// `./`.
#[test]
fn reports_e004_with_searched_paths_when_relative_module_missing() {
    let project = luau_project();
    let error = resolve_luau(&project, "src/main.luau", "./nonexistent").expect_err("missing");
    assert_eq!(error.code, "E004");
    assert_eq!(error.message, "module not found: `./nonexistent`");
    let base = format!("{}/./nonexistent", normalized(&project, "src"));
    let expected_help = format!(
        "searched paths:\n  - `{base}.luau`\n  - `{base}.lua`\n  - `{base}/init.luau`\n  - \
         `{base}/init.lua`"
    );
    assert_eq!(error.help, Some(expected_help));
}

#[test]
fn reports_e004_when_no_luaurc_declares_the_alias() {
    let project = luau_project();
    let error =
        resolve_luau(&project, "src/main.luau", "@Missing/utils").expect_err("unknown alias");
    assert_eq!(error.code, "E004");
    assert_eq!(error.message, "module not found: `@Missing/utils`");
    assert_eq!(
        error.help.as_deref(),
        Some("searched paths:\n  - alias `@Missing` not found in any `.luaurc`")
    );
}

#[test]
fn reports_e004_for_unprefixed_require() {
    let project = luau_project();
    let error = resolve_luau(&project, "src/main.luau", "utils").expect_err("unprefixed");
    assert_eq!(error.code, "E004");
}

#[test]
fn resolves_parent_relative_luau_file() {
    let project = luau_project();
    write_files(project.path(), &[("src/sub/child.luau", "-- child")]);
    let resolved =
        resolve_luau(&project, "src/sub/child.luau", "../utils").expect("resolve ../utils");
    assert_eq!(resolved.path, normalized(&project, "src/utils.luau"));
}

#[test]
fn init_file_resolves_relative_from_parent_parent() {
    let project = luau_project();
    write_files(project.path(), &[("src/components/init.luau", "-- init")]);
    let resolved =
        resolve_luau(&project, "src/components/init.luau", "./utils").expect("resolve ./utils");
    assert_eq!(resolved.path, normalized(&project, "src/utils.luau"));
}

#[test]
fn resolves_alias_from_luaurc() {
    let project = luau_project();
    write_files(
        project.path(),
        &[
            ("shared/common.luau", "return {}"),
            ("src/.luaurc", r#"{"aliases":{"shared":"../shared"}}"#),
        ],
    );
    let resolved =
        resolve_luau(&project, "src/main.luau", "@shared/common").expect("resolve @shared");
    assert_eq!(resolved.path, normalized(&project, "shared/common.luau"));
}

#[test]
fn matches_alias_case_insensitively() {
    let project = luau_project();
    write_files(
        project.path(),
        &[
            ("shared/thing.luau", "return {}"),
            ("src/.luaurc", r#"{"aliases":{"Shared":"../shared"}}"#),
        ],
    );
    let resolved =
        resolve_luau(&project, "src/main.luau", "@shared/thing").expect("resolve @shared");
    assert_eq!(resolved.path, normalized(&project, "shared/thing.luau"));
}

#[test]
fn nested_file_sees_aliases_from_every_ancestor_luaurc() {
    let project = luau_project();
    write_files(
        project.path(),
        &[
            ("src/deep/nested/mod.luau", "-- entry"),
            (".luaurc", r#"{"aliases":{"root_alias":"./root_lib"}}"#),
            ("src/.luaurc", r#"{"aliases":{"src_alias":"./src_lib"}}"#),
            ("root_lib/foo.luau", "return {}"),
            ("src/src_lib/bar.luau", "return {}"),
        ],
    );
    let from_file = "src/deep/nested/mod.luau";

    let root_alias_resolved =
        resolve_luau(&project, from_file, "@root_alias/foo").expect("resolve @root_alias");
    assert_eq!(root_alias_resolved.path, normalized(&project, "root_lib/foo.luau"));

    let src_alias_resolved =
        resolve_luau(&project, from_file, "@src_alias/bar").expect("resolve @src_alias");
    assert_eq!(src_alias_resolved.path, normalized(&project, "src/src_lib/bar.luau"));
}

#[test]
fn self_alias_resolves_from_an_init_files_own_directory() {
    let project = luau_project();
    write_files(
        project.path(),
        &[("src/components/init.luau", "-- init"), ("src/components/button.luau", "return {}")],
    );
    let resolved = resolve_luau(&project, "src/components/init.luau", "@self/button")
        .expect("resolve @self/button");
    assert_eq!(resolved.path, normalized(&project, "src/components/button.luau"));
}

#[test]
fn reports_e004_for_bare_self_alias() {
    let project = luau_project();
    let error = resolve_luau(&project, "src/main.luau", "@self").expect_err("bare @self");
    assert_eq!(error.code, "E004");
}

#[test]
fn last_declared_alias_wins_among_names_equal_after_lowercasing() {
    for (aliases, module) in [
        (r#"{"aliases":{"Foo":"a","foo":"b"}}"#, "@foo/m"),
        (r#"{"aliases":{"foo":"a","Foo":"b"}}"#, "@foo/m"),
        (r#"{"aliases":{"bar":"a","Bar":"b"}}"#, "@bar/m"),
        (r#"{"aliases":{"Bar":"a","bar":"b"}}"#, "@bar/m"),
    ] {
        let project = luau_project();
        write_files(
            project.path(),
            &[("a/m.luau", "return 1"), ("b/m.luau", "return 2"), (".luaurc", aliases)],
        );
        let resolved = resolve_luau(&project, "src/main.luau", module).expect("resolve alias");
        assert_eq!(resolved.path, normalized(&project, "b/m.luau"), "{aliases}");
    }
}
