//! A `require("path")` string becomes a clickable link resolved through
//! [`luck_resolver`], the resolver the bundler uses, which the project keeps
//! for every request until the file watcher clears it.
//!
//! The strings are the paths of the analysis's `require` sites, which leave
//! out a local `require`, a local `_ENV`, and a rebound global, since none is
//! the module loader and a build leaves its call alone too. Links follow Lua
//! template search paths, Luau relative and `@alias` imports, and the `init`
//! parent-parent rule exactly as a build does.

use ls_types::{DocumentLink, Position, Uri};
use luck_ast::StringLiteral;

use crate::document::{DocumentState, file_path};

pub(crate) fn document_links(document: &DocumentState, document_uri: &Uri) -> Vec<DocumentLink> {
    links(document, document_uri, |_| true)
}

/// The module the `require` string at `position` links to, either end
/// included.
pub(crate) fn target_at(
    document: &DocumentState,
    document_uri: &Uri,
    position: Position,
) -> Option<Uri> {
    let offset = document.offset(position);
    let is_at = |path: &StringLiteral<'_>| path.span.start <= offset && offset <= path.span.end;
    links(document, document_uri, is_at).into_iter().find_map(|link| link.target)
}

/// The links of the `require` paths `is_wanted` keeps.
fn links(
    document: &DocumentState,
    document_uri: &Uri,
    is_wanted: impl Fn(&StringLiteral<'_>) -> bool,
) -> Vec<DocumentLink> {
    let Some(document_path) = file_path(document_uri) else {
        return Vec::new();
    };
    // A document with no `luck.json` above it is under the defaults rooted
    // at its own directory, so bare siblings link as they bundle.
    let resolver = document.settings().project.resolver(document.version());
    let mut positions = document.positions();
    let paths = document.semantic().semantic().requires().iter().filter_map(|site| site.path);
    paths
        .filter(|path| is_wanted(path))
        .filter_map(|path| {
            let module = path.as_str().filter(|module| !module.is_empty())?;
            let resolution = resolver.resolve(&document_path, module).ok()?;
            Some(DocumentLink {
                range: positions.range(path.span),
                target: Some(Uri::from_file_path(&resolution.path)?),
                tooltip: Some(module.to_owned()),
                data: None,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::Path;

    use luck_config::Project;
    use luck_token::LuaTarget;

    use super::*;
    use crate::fixtures::document_in;

    /// `text` under the defaults rooted at `directory`.
    ///
    /// That's what a document with no `luck.json` above it gets. Going
    /// through `SettingsCache` instead would let discovery walk above the
    /// temp directory and pick up whatever `luck.json` sits there.
    fn document_beside(text: &str, directory: &Path) -> DocumentState {
        document_in(text, LuaTarget::Lua54, Project::defaults(directory.to_path_buf()))
    }

    #[test]
    fn bare_sibling_resolves_against_the_requiring_file() {
        let directory = tempfile::tempdir().expect("tempdir");
        fs::write(directory.path().join("foo.lua"), "return {}\n").expect("write foo.lua");
        let main_path = directory.path().join("main.lua");
        let text = "local foo = require(\"foo\")\n";
        fs::write(&main_path, text).expect("write main.lua");
        let uri = Uri::from_file_path(&main_path).expect("file path URI");

        let links = document_links(&document_beside(text, directory.path()), &uri);
        assert_eq!(links.len(), 1, "{links:?}");
        let target = links[0].target.as_ref().expect("link target");
        assert!(target.path().as_str().ends_with("foo.lua"), "{}", target.as_str());
    }

    #[test]
    fn links_follow_the_project_s_search_paths_from_its_root() {
        let directory = tempfile::tempdir().expect("tempdir");
        fs::create_dir_all(directory.path().join("lib")).expect("mkdir lib");
        fs::create_dir_all(directory.path().join("src")).expect("mkdir src");
        fs::write(directory.path().join("lib/dep.lua"), "return {}\n").expect("write dep.lua");
        fs::write(directory.path().join("luck.json"), r#"{ "search_paths": ["lib/?.lua"] }"#)
            .expect("write luck.json");
        let main_path = directory.path().join("src/main.lua");
        let text = "local dep = require(\"dep\")\n";
        fs::write(&main_path, text).expect("write main.lua");
        let uri = Uri::from_file_path(&main_path).expect("file path URI");

        let project = Project::load(&directory.path().join("luck.json")).expect("config loads");
        let links = document_links(&document_in(text, LuaTarget::Lua54, project), &uri);
        assert_eq!(links.len(), 1, "{links:?}");
        let target = links[0].target.as_ref().expect("link target");
        assert!(target.path().as_str().ends_with("lib/dep.lua"), "{}", target.as_str());
    }

    #[test]
    fn target_at_resolves_the_require_string_under_the_position() {
        let directory = tempfile::tempdir().expect("tempdir");
        fs::write(directory.path().join("foo.lua"), "return {}\n").expect("write foo.lua");
        let main_path = directory.path().join("main.lua");
        let text = "\
local foo = require(\"foo\")
local x = 1
";
        fs::write(&main_path, text).expect("write main.lua");
        let uri = Uri::from_file_path(&main_path).expect("file path URI");
        let document = document_beside(text, directory.path());

        // Both ends of the string count.
        for utf16_column in [20, 25] {
            let target = target_at(&document, &uri, Position::new(0, utf16_column))
                .expect("the require string resolves");
            assert!(target.path().as_str().ends_with("foo.lua"), "{}", target.as_str());
        }
        assert_eq!(target_at(&document, &uri, Position::new(0, 19)), None);
        assert_eq!(target_at(&document, &uri, Position::new(1, 6)), None);
    }

    fn links_beside_foo(text: &str) -> Vec<DocumentLink> {
        let directory = tempfile::tempdir().expect("tempdir");
        fs::write(directory.path().join("foo.lua"), "return {}\n").expect("write foo.lua");
        let main_path = directory.path().join("main.lua");
        fs::write(&main_path, text).expect("write main.lua");
        let uri = Uri::from_file_path(&main_path).expect("file path URI");
        document_links(&document_beside(text, directory.path()), &uri)
    }

    #[test]
    fn a_statement_level_require_links() {
        let links = links_beside_foo("require(\"foo\")\n");
        assert_eq!(links.len(), 1, "a side-effect `require` resolves like any other: {links:?}");
    }

    #[test]
    fn a_require_through_the_global_table_links() {
        let links = links_beside_foo("local m = _G.require(\"foo\")\n");
        assert_eq!(links.len(), 1, "a build bundles `_G.require(\"foo\")`: {links:?}");
    }

    #[test]
    fn a_shadowed_require_does_not_link() {
        // The bundler leaves a call through a local `require` alone (W005),
        // so a build doesn't resolve it to the module.
        let links = links_beside_foo(
            "\
local require = print
local m = require(\"foo\")
",
        );
        assert!(links.is_empty(), "a local `require` isn't the module loader: {links:?}");
    }

    #[test]
    fn a_long_string_require_links() {
        // The bundler decodes long-bracket require strings.
        let links = links_beside_foo("local m = require[[foo]]\n");
        assert_eq!(links.len(), 1, "`require[[foo]]` resolves in a build: {links:?}");
    }

    #[test]
    fn an_escaped_require_string_links() {
        // The bundler decodes escapes, so this requires `foo`.
        let links = links_beside_foo("local m = require(\"\\x66oo\")\n");
        assert_eq!(links.len(), 1, "`\"\\x66oo\"` is the string `foo`: {links:?}");
    }
}
