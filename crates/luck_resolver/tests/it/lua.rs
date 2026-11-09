use luck_core::Diagnostic;
use luck_resolver::{ResolvedModule, Resolver};
use luck_token::LuaVersion;
use tempfile::TempDir;

use crate::common::{normalized, project_with, request};

fn lua_project() -> TempDir {
    project_with(&[
        ("src/main.lua", "local u = require('utils')"),
        ("src/utils.lua", "return {}"),
        ("src/foo/bar.lua", "return {}"),
        ("src/components/init.lua", "return {}"),
        ("lib/helper.lua", "return {}"),
    ])
}

/// Resolves `module`, required from `src/main.lua`, against `templates`
/// rooted at `project`.
///
/// # Errors
///
/// Returns the resolver's diagnostic when `module` doesn't resolve.
fn resolve_lua(
    project: &TempDir,
    templates: &[&str],
    module: &str,
) -> Result<ResolvedModule, Diagnostic> {
    let search_paths: Vec<String> =
        templates.iter().map(|template| (*template).to_owned()).collect();
    let from_file = normalized(project, "src/main.lua");
    Resolver::new(LuaVersion::Lua54, &search_paths, project.path())
        .resolve(&request(module, &from_file))
}

#[test]
fn resolves_module_from_first_template() {
    let project = lua_project();
    let resolved =
        resolve_lua(&project, &["src/?.lua", "src/?/init.lua"], "utils").expect("resolve utils");
    assert_eq!(resolved.path, normalized(&project, "src/utils.lua"));
}

#[test]
fn resolves_dotted_module_to_nested_path() {
    let project = lua_project();
    let resolved = resolve_lua(&project, &["src/?.lua"], "foo.bar").expect("resolve foo.bar");
    assert_eq!(resolved.path, normalized(&project, "src/foo/bar.lua"));
}

#[test]
fn resolves_directory_module_via_init_template() {
    let project = lua_project();
    let resolved = resolve_lua(&project, &["src/?.lua", "src/?/init.lua"], "components")
        .expect("resolve components");
    assert_eq!(resolved.path, normalized(&project, "src/components/init.lua"));
}

#[test]
fn falls_back_to_later_template() {
    let project = lua_project();
    let resolved =
        resolve_lua(&project, &["src/?.lua", "lib/?.lua"], "helper").expect("resolve helper");
    assert_eq!(resolved.path, normalized(&project, "lib/helper.lua"));
}

#[test]
fn reports_e004_with_searched_paths_when_missing() {
    let project = lua_project();
    let error = resolve_lua(&project, &["src/?.lua", "src/?/init.lua"], "nonexistent")
        .expect_err("a missing module must not resolve");
    assert_eq!(error.code, "E004");
    assert_eq!(error.message, "module not found: `nonexistent`");
    assert_eq!(
        error.help.as_deref(),
        Some("searched paths:\n  - `src/nonexistent.lua`\n  - `src/nonexistent/init.lua`")
    );
}

#[test]
fn empty_search_paths_try_the_default_templates() {
    let project =
        project_with(&[("dep.lua", "return {}"), ("pkg/init.lua", "return {}"), ("main.lua", "")]);
    let from_file = normalized(&project, "main.lua");
    let mut resolver = Resolver::new(LuaVersion::Lua54, &[], project.path());
    let by_name = resolver.resolve(&request("dep", &from_file)).expect("resolve dep");
    assert_eq!(by_name.path, normalized(&project, "dep.lua"));
    let by_init = resolver.resolve(&request("pkg", &from_file)).expect("resolve pkg");
    assert_eq!(by_init.path, normalized(&project, "pkg/init.lua"));
}

// Real Lua's default `./?.lua` keeps `.dep` (`/dep` once dots map to `/`) under
// the working directory, and the relative default template must keep it under
// the project root rather than on the filesystem root.
#[test]
fn leading_dot_module_stays_under_the_project_root() {
    let project = project_with(&[("dep.lua", "return {}"), ("main.lua", "")]);
    let from_file = normalized(&project, "main.lua");
    let module = Resolver::new(LuaVersion::Lua54, &[], project.path())
        .resolve(&request(".dep", &from_file))
        .expect("`.dep` must resolve to `dep.lua` under the project root");
    assert_eq!(module.path, normalized(&project, "dep.lua"));
}
