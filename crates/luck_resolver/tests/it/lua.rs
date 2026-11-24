use std::fs;
use std::path::{Path, PathBuf};

use luck_resolver::{Resolution, ResolveError, ResolveOptions, Resolver, ResolverGeneric};
use luck_token::LuaVersion;
use tempfile::TempDir;

use crate::common::{MemoryFileSystem, canonical, project_with};

fn lua_project() -> TempDir {
    project_with(&[
        ("src/main.lua", "local u = require('utils')"),
        ("src/utils.lua", "return {}"),
        ("src/foo/bar.lua", "return {}"),
        ("src/components/init.lua", "return {}"),
        ("lib/helper.lua", "return {}"),
    ])
}

fn lua_options(root: &Path, templates: &[&str]) -> ResolveOptions {
    ResolveOptions {
        version: LuaVersion::Lua54,
        search_paths: templates.iter().map(|template| (*template).to_owned()).collect(),
        root: root.to_path_buf(),
    }
}

fn resolve_lua(
    project: &TempDir,
    templates: &[&str],
    module: &str,
) -> Result<Resolution, ResolveError> {
    Resolver::new(lua_options(project.path(), templates))
        .resolve(&canonical(project, "src/main.lua"), module)
}

#[test]
fn resolves_module_from_first_template() {
    let project = lua_project();
    let resolved =
        resolve_lua(&project, &["src/?.lua", "src/?/init.lua"], "utils").expect("resolve utils");
    assert_eq!(resolved.path, canonical(&project, "src/utils.lua"));
    assert!(!resolved.is_self_alias_shadowed);
}

#[test]
fn resolves_dotted_module_to_nested_path() {
    let project = lua_project();
    let resolved = resolve_lua(&project, &["src/?.lua"], "foo.bar").expect("resolve foo.bar");
    assert_eq!(resolved.path, canonical(&project, "src/foo/bar.lua"));
}

#[test]
fn resolves_directory_module_via_init_template() {
    let project = lua_project();
    let resolved = resolve_lua(&project, &["src/?.lua", "src/?/init.lua"], "components")
        .expect("resolve components");
    assert_eq!(resolved.path, canonical(&project, "src/components/init.lua"));
}

#[test]
fn falls_back_to_later_template() {
    let project = lua_project();
    let resolved =
        resolve_lua(&project, &["src/?.lua", "lib/?.lua"], "helper").expect("resolve helper");
    assert_eq!(resolved.path, canonical(&project, "lib/helper.lua"));
}

#[test]
fn reports_every_template_it_searched_when_missing() {
    let project = lua_project();
    let error = resolve_lua(&project, &["src/?.lua", "src/?/init.lua"], "nonexistent")
        .expect_err("a missing module must not resolve");
    assert_eq!(
        error,
        ResolveError::NotFound(vec![
            PathBuf::from("src/nonexistent.lua"),
            PathBuf::from("src/nonexistent/init.lua"),
        ])
    );
}

#[test]
fn empty_search_paths_try_the_default_templates() {
    let project =
        project_with(&[("dep.lua", "return {}"), ("pkg/init.lua", "return {}"), ("main.lua", "")]);
    let from_file = canonical(&project, "main.lua");
    let resolver = Resolver::new(lua_options(project.path(), &[]));
    let by_name = resolver.resolve(&from_file, "dep").expect("resolve dep");
    assert_eq!(by_name.path, canonical(&project, "dep.lua"));
    let by_init = resolver.resolve(&from_file, "pkg").expect("resolve pkg");
    assert_eq!(by_init.path, canonical(&project, "pkg/init.lua"));
}

// Real Lua's default `./?.lua` keeps `.dep` (`/dep` once dots map to `/`) under
// the working directory, and the relative default template must keep it under
// the project root rather than on the filesystem root.
#[test]
fn leading_dot_module_stays_under_the_project_root() {
    let project = project_with(&[("dep.lua", "return {}"), ("main.lua", "")]);
    let module = Resolver::new(lua_options(project.path(), &[]))
        .resolve(&canonical(&project, "main.lua"), ".dep")
        .expect("`.dep` must resolve to `dep.lua` under the project root");
    assert_eq!(module.path, canonical(&project, "dep.lua"));
}

#[test]
fn a_resolver_keeps_what_it_found_until_its_cache_clears() {
    let project = lua_project();
    let from_file = canonical(&project, "src/main.lua");
    let resolver = Resolver::new(lua_options(project.path(), &["src/?.lua", "lib/?.lua"]));
    let first = resolver.resolve(&from_file, "helper").expect("resolve before the change");
    assert_eq!(first.path, canonical(&project, "lib/helper.lua"));
    fs::write(project.path().join("src/helper.lua"), "return {}").expect("write a closer helper");
    assert_eq!(resolver.resolve(&from_file, "helper"), Ok(first));

    resolver.clear_cache();
    let cleared = resolver.resolve(&from_file, "helper").expect("resolve after the change");
    assert_eq!(cleared.path, canonical(&project, "src/helper.lua"));
}

#[test]
fn resolves_against_the_file_system_it_is_given() {
    let files = MemoryFileSystem::new(&[
        ("/project/main.lua", ""),
        ("/project/lib/foo/bar.lua", "return {}"),
        ("/project/lib/pkg/init.lua", "return {}"),
    ]);
    let resolver = ResolverGeneric::new_with_file_system(
        files,
        lua_options(Path::new("/project"), &["lib/?.lua", "lib/?/init.lua"]),
    );
    let from_file = Path::new("/project/main.lua");
    let nested = resolver.resolve(from_file, "foo.bar").expect("resolve foo.bar");
    assert_eq!(nested.path, Path::new("/project/lib/foo/bar.lua"));
    let by_init = resolver.resolve(from_file, "pkg").expect("resolve pkg");
    assert_eq!(by_init.path, Path::new("/project/lib/pkg/init.lua"));
    assert!(matches!(resolver.resolve(from_file, "main"), Err(ResolveError::NotFound(_))));
}
