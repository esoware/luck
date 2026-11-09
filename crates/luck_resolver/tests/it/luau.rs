use std::fs;

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

// `parseAlias` tests the key as written against its lowercased entries, so
// only a later name with an uppercase letter replaces an equal one.
#[test]
fn later_alias_replaces_an_equal_name_unless_already_lowercase() {
    for (aliases, module, expected) in [
        (r#"{"aliases":{"Foo":"a","foo":"b"}}"#, "@foo/m", "a/m.luau"),
        (r#"{"aliases":{"foo":"a","Foo":"b"}}"#, "@foo/m", "b/m.luau"),
        (r#"{"aliases":{"bar":"a","Bar":"b"}}"#, "@bar/m", "b/m.luau"),
        (r#"{"aliases":{"Bar":"a","bar":"b"}}"#, "@bar/m", "a/m.luau"),
        (r#"{"aliases":{"x":"a","x":"b"}}"#, "@x/m", "a/m.luau"),
    ] {
        let project = luau_project();
        write_files(
            project.path(),
            &[("a/m.luau", "return 1"), ("b/m.luau", "return 2"), (".luaurc", aliases)],
        );
        let resolved = resolve_luau(&project, "src/main.luau", module).expect("resolve alias");
        assert_eq!(resolved.path, normalized(&project, expected), "{aliases}");
    }
}

#[test]
fn one_resolver_sees_the_files_it_first_probed() {
    let project = luau_project();
    let from_file = normalized(&project, "src/main.luau");
    let mut resolver = Resolver::new(LuaVersion::Luau, &[], project.path());
    let first =
        resolver.resolve(&request("./utils", &from_file)).expect("resolve before the change");
    fs::remove_file(project.path().join("src/utils.luau")).expect("remove utils");
    let second =
        resolver.resolve(&request("./utils", &from_file)).expect("resolve from the warm cache");
    assert_eq!(first.path, second.path);
}

// Luau chains an alias whose value starts with `@`, per the require-by-string
// aliases RFC and the `chained_aliases` fixtures.
#[test]
fn chained_alias_resolves_through_the_aliased_directory() {
    let project = luau_project();
    write_files(
        project.path(),
        &[
            ("lib/sub/m.luau", "return {}"),
            (".luaurc", r#"{"aliases":{"outer":"@inner/sub","inner":"./lib"}}"#),
        ],
    );
    let resolved = resolve_luau(&project, "src/main.luau", "@outer/m")
        .expect("`@outer` must chain through `@inner` to `lib/sub`");
    assert_eq!(resolved.path, normalized(&project, "lib/sub/m.luau"));
}

// Luau looks a chained alias up from the `.luaurc` declaring it, so a closer
// file's alias of the same name isn't in play.
#[test]
fn chained_alias_searches_from_the_luaurc_declaring_it() {
    let project = luau_project();
    write_files(
        project.path(),
        &[
            (".luaurc", r#"{"aliases":{"outer":"@inner/sub","inner":"./lib"}}"#),
            ("src/.luaurc", r#"{"aliases":{"inner":"./elsewhere"}}"#),
            ("lib/sub/m.luau", "return {}"),
        ],
    );
    let resolved = resolve_luau(&project, "src/main.luau", "@outer/m")
        .expect("`@inner` must resolve in the `.luaurc` declaring `@outer`");
    assert_eq!(resolved.path, normalized(&project, "lib/sub/m.luau"));
}

// Luau restarts its cycle check at each `.luaurc` the search moves to.
#[test]
fn chained_alias_through_a_parent_luaurc_is_no_cycle() {
    let project = luau_project();
    write_files(
        project.path(),
        &[
            (".luaurc", r#"{"aliases":{"a":"./lib","b":"@a/sub"}}"#),
            ("src/.luaurc", r#"{"aliases":{"a":"@b"}}"#),
            ("lib/sub/m.luau", "return {}"),
        ],
    );
    let resolved = resolve_luau(&project, "src/main.luau", "@a/m")
        .expect("`src`'s `@a` goes through the root's `@b` to the root's own `@a`");
    assert_eq!(resolved.path, normalized(&project, "lib/sub/m.luau"));
}

#[test]
fn alias_cycle_reports_an_error_instead_of_a_literal_directory() {
    let project = luau_project();
    write_files(
        project.path(),
        &[(".luaurc", r#"{"aliases":{"a":"@b","b":"@a"}}"#), ("@b/m.luau", "return {}")],
    );
    let result = resolve_luau(&project, "src/main.luau", "@a/m");
    assert!(
        result.is_err(),
        "an alias cycle must be an error, not the literal directory `@b`: {result:?}"
    );
}

// `@self` is the current module, so from `main.luau` it names `main/...`, not a
// sibling. Luau: `could not resolve child component "utils"`.
#[test]
fn self_alias_from_a_non_init_file_does_not_name_a_sibling() {
    let project = luau_project();
    let error = resolve_luau(&project, "src/main.luau", "@self/utils")
        .expect_err("`@self/utils` from `main.luau` must not resolve to the sibling `utils.luau`");
    assert_eq!(error.code, "E004");
}

// The module path of `pkg/init.luau` is `pkg`, so alias discovery starts at
// `pkg`'s parent. Luau's own fixture names the value
// `this_should_not_be_read_by_init_luau`.
#[test]
fn init_file_ignores_the_luaurc_beside_it() {
    let project = luau_project();
    write_files(
        project.path(),
        &[
            ("pkg/init.luau", "-- init"),
            ("pkg/lib/m.luau", "return {}"),
            ("pkg/.luaurc", r#"{"aliases":{"x":"./lib"}}"#),
        ],
    );
    let error = resolve_luau(&project, "pkg/init.luau", "@x/m")
        .expect_err("an init file must not read the `.luaurc` in its own directory");
    assert_eq!(error.code, "E004");
}

#[test]
fn relative_require_cannot_name_an_init_file_directly() {
    let project = luau_project();
    write_files(project.path(), &[("src/init.luau", "return {}")]);
    let error = resolve_luau(&project, "src/main.luau", "./init")
        .expect_err("`init` is not a child module of its directory");
    assert_eq!(error.code, "E004");
}

#[test]
fn relative_require_cannot_name_the_config_luau_file() {
    let project = luau_project();
    write_files(project.path(), &[("src/.config.luau", "return {}")]);
    let error = resolve_luau(&project, "src/main.luau", "./.config")
        .expect_err("`.config.luau` must not be requirable");
    assert_eq!(error.code, "E004");
}

// Luau's navigator reports Ambiguous as soon as both `mod.luau` and a `mod`
// directory exist, whether or not the directory has an init file.
#[test]
fn file_and_same_named_directory_without_init_is_ambiguous() {
    let project = luau_project();
    write_files(project.path(), &[("src/mod.luau", "return {}"), ("src/mod/other.luau", "")]);
    let error = resolve_luau(&project, "src/main.luau", "./mod")
        .expect_err("`mod.luau` beside a `mod` directory must be ambiguous");
    assert_eq!(error.code, "E007");
}

#[test]
fn intermediate_component_with_same_named_file_is_ambiguous() {
    let project = luau_project();
    write_files(project.path(), &[("src/mod.luau", "return {}"), ("src/mod/child.luau", "")]);
    let error = resolve_luau(&project, "src/main.luau", "./mod/child")
        .expect_err("a path through a component that is also a file must be ambiguous");
    assert_eq!(error.code, "E007");
}

// Luau: `could not reset to requiring context (ambiguous)`.
#[test]
fn requiring_file_with_same_named_directory_is_ambiguous() {
    let project = luau_project();
    write_files(project.path(), &[("src/main/x.luau", "return {}")]);
    let error = resolve_luau(&project, "src/main.luau", "./utils")
        .expect_err("a requiring file beside a directory of its own name must be ambiguous");
    assert_eq!(error.code, "E007");
}

#[test]
fn relative_require_accepts_backslash_separators() {
    let project = luau_project();
    let resolved = resolve_luau(&project, "src/main.luau", ".\\utils")
        .expect("Luau maps `\\` to `/` before reading the require string");
    assert_eq!(resolved.path, normalized(&project, "src/utils.luau"));
}

#[test]
fn trailing_slash_still_names_the_module() {
    let project = luau_project();
    let resolved = resolve_luau(&project, "src/main.luau", "./utils/")
        .expect("an empty path component is ignored, so `./utils/` is `./utils`");
    assert_eq!(resolved.path, normalized(&project, "src/utils.luau"));
}

// `str::to_lowercase` folds U+212A KELVIN SIGN to `k`; Luau lowers ASCII only.
#[test]
fn alias_lookup_lowercases_ascii_only() {
    let project = luau_project();
    write_files(
        project.path(),
        &[(".luaurc", r#"{"aliases":{"k":"./lib"}}"#), ("lib/m.luau", "return {}")],
    );
    let error = resolve_luau(&project, "src/main.luau", "@\u{212A}/m")
        .expect_err("U+212A is not the alias `k`");
    assert_eq!(error.code, "E004");
}

// Luau reports the parse error, so the parent's `x` must not stand in for it.
#[test]
fn malformed_closer_luaurc_does_not_fall_through_to_a_parent_alias() {
    let project = luau_project();
    write_files(
        project.path(),
        &[
            (".luaurc", r#"{"aliases":{"x":"./lib"}}"#),
            ("lib/m.luau", "return {}"),
            ("src/.luaurc", "{"),
        ],
    );
    let result = resolve_luau(&project, "src/main.luau", "@x/m");
    assert!(
        result.is_err(),
        "a malformed `.luaurc` must fail the lookup, not defer to its parent: {result:?}"
    );
}

#[test]
fn luaurc_with_non_utf8_bytes_keeps_its_aliases() {
    let project = luau_project();
    write_files(project.path(), &[("lib/m.luau", "return {}")]);
    fs::write(project.path().join(".luaurc"), b"{// caf\xe9\n\"aliases\":{\"x\":\"./lib\"}}")
        .expect("write .luaurc");
    let resolved = resolve_luau(&project, "src/main.luau", "@x/m")
        .expect("Luau reads `.luaurc` as bytes, so a Latin-1 comment must not drop the aliases");
    assert_eq!(resolved.path, normalized(&project, "lib/m.luau"));
}

#[test]
fn luaurc_with_repeated_aliases_key_merges_them() {
    let project = luau_project();
    write_files(
        project.path(),
        &[
            ("lib/m.luau", "return {}"),
            (".luaurc", r#"{"aliases":{"x":"./lib"},"aliases":{"y":"./other"}}"#),
        ],
    );
    let resolved = resolve_luau(&project, "src/main.luau", "@x/m")
        .expect("a repeated `aliases` key merges in Luau instead of rejecting the file");
    assert_eq!(resolved.path, normalized(&project, "lib/m.luau"));
}

#[test]
fn deeply_nested_luaurc_does_not_overflow_the_stack() {
    let project = luau_project();
    let contents = format!(r#"{{"globals":{}"#, "[".repeat(200_000));
    write_files(project.path(), &[(".luaurc", contents.as_str())]);
    let error = resolve_luau(&project, "src/main.luau", "@x/m")
        .expect_err("a `.luaurc` that doesn't parse declares no alias");
    assert_eq!(error.code, "E004");
}

// Alias names are limited to `[A-Za-z0-9.\-_]`; Luau fails with `Invalid alias`.
#[test]
fn invalid_alias_name_is_rejected() {
    let project = luau_project();
    write_files(
        project.path(),
        &[(".luaurc", r#"{"aliases":{"a b":"./lib"}}"#), ("lib/m.luau", "return {}")],
    );
    let error = resolve_luau(&project, "src/main.luau", "@a b/m")
        .expect_err("`a b` is not a legal alias name");
    assert_eq!(error.code, "E004");
}

// The empty component is ignored, so `@x//m` is `lib/m`.
#[test]
fn double_slash_after_alias_stays_inside_the_alias() {
    let project = luau_project();
    write_files(
        project.path(),
        &[(".luaurc", r#"{"aliases":{"x":"./lib"}}"#), ("lib/m.luau", "return {}")],
    );
    let resolved = resolve_luau(&project, "src/main.luau", "@x//m")
        .expect("`@x//m` must resolve inside the alias directory");
    assert_eq!(resolved.path, normalized(&project, "lib/m.luau"));
}

// Luau navigates per component, so a missing `nope` fails even before `..`.
#[test]
fn parent_of_a_missing_directory_does_not_resolve() {
    let project = luau_project();
    let error = resolve_luau(&project, "src/main.luau", "./nope/../utils")
        .expect_err("`./nope/../utils` must fail: `nope` doesn't exist");
    assert_eq!(error.code, "E004");
}
