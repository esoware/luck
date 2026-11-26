//! A `require("path")` string becomes a clickable link resolved through
//! [`luck_resolver`], the resolver the bundler uses, which the session keeps
//! for every request until the file watcher clears it.
//!
//! The strings are the requires of the document's record, which leave out a
//! local `require` and a rebound global, since neither loads a file and a
//! build leaves each alone too. Links follow relative and `@alias` requires
//! and the `init` parent-parent rule exactly as a build does.

use ls_types::{DocumentLink, Position, Uri};

use crate::document::DocumentState;
use crate::module_record::Require;

pub(crate) fn document_links(document: &DocumentState) -> Vec<DocumentLink> {
    links(document, |_| true)
}

/// The module the `require` string at `position` links to, either end
/// included.
pub(crate) fn target_at(document: &DocumentState, position: Position) -> Option<Uri> {
    let is_at =
        |require: &Require| require.range.start <= position && position <= require.range.end;
    links(document, is_at).into_iter().find_map(|link| link.target)
}

/// The links of the requires `is_wanted` keeps.
fn links(document: &DocumentState, is_wanted: impl Fn(&Require) -> bool) -> Vec<DocumentLink> {
    document
        .record()
        .requires()
        .iter()
        .filter(|require| is_wanted(require))
        .filter_map(|require| {
            let resolution = document.resolve(require)?.ok()?;
            Some(DocumentLink {
                range: require.range,
                target: Some(document.settings().project.uri(&resolution.path)?),
                tooltip: Some(require.module.to_string()),
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
    use luck_resolver::FileSystemOs;

    use super::*;
    use crate::fixtures::document_at;

    /// `text` as `main.luau` under the defaults rooted at `directory`.
    ///
    /// That's what a document with no `luck.json` above it gets. Going
    /// through `SettingsCache` instead would let discovery walk above the
    /// temp directory and pick up whatever `luck.json` sits there.
    fn main_beside(text: &str, directory: &Path) -> DocumentState {
        let main_path = directory.join("main.luau");
        fs::write(&main_path, text).expect("write main.luau");
        let project = Project::defaults(directory.to_path_buf(), FileSystemOs);
        document_at(text, project, &main_path)
    }

    #[test]
    fn a_relative_sibling_resolves_against_the_requiring_file() {
        let directory = tempfile::tempdir().expect("tempdir");
        fs::write(directory.path().join("foo.luau"), "return {}\n").expect("write foo.luau");
        let links =
            document_links(&main_beside("local foo = require(\"./foo\")\n", directory.path()));
        assert_eq!(links.len(), 1, "{links:?}");
        let target = links[0].target.as_ref().expect("link target");
        assert!(target.path().as_str().ends_with("foo.luau"), "{}", target.as_str());
    }

    #[test]
    fn links_follow_the_aliases_of_a_luaurc() {
        let directory = tempfile::tempdir().expect("tempdir");
        fs::create_dir_all(directory.path().join("lib")).expect("mkdir lib");
        fs::create_dir_all(directory.path().join("src")).expect("mkdir src");
        fs::write(directory.path().join("lib/dep.luau"), "return {}\n").expect("write dep.luau");
        fs::write(directory.path().join(".luaurc"), r#"{ "aliases": { "lib": "./lib" } }"#)
            .expect("write .luaurc");
        let main_path = directory.path().join("src/main.luau");
        let text = "local dep = require(\"@lib/dep\")\n";
        fs::write(&main_path, text).expect("write main.luau");

        let project = Project::defaults(directory.path().to_path_buf(), FileSystemOs);
        let links = document_links(&document_at(text, project, &main_path));
        assert_eq!(links.len(), 1, "{links:?}");
        let target = links[0].target.as_ref().expect("link target");
        assert!(target.path().as_str().ends_with("lib/dep.luau"), "{}", target.as_str());
    }

    #[test]
    fn target_at_resolves_the_require_string_under_the_position() {
        let directory = tempfile::tempdir().expect("tempdir");
        fs::write(directory.path().join("foo.luau"), "return {}\n").expect("write foo.luau");
        let text = "\
local foo = require(\"./foo\")
local x = 1
";
        let document = main_beside(text, directory.path());

        // Both ends of the string count.
        for utf16_column in [20, 27] {
            let target = target_at(&document, Position::new(0, utf16_column))
                .expect("the require string resolves");
            assert!(target.path().as_str().ends_with("foo.luau"), "{}", target.as_str());
        }
        assert_eq!(target_at(&document, Position::new(0, 19)), None);
        assert_eq!(target_at(&document, Position::new(1, 6)), None);
    }

    fn links_beside_foo(text: &str) -> Vec<DocumentLink> {
        let directory = tempfile::tempdir().expect("tempdir");
        fs::write(directory.path().join("foo.luau"), "return {}\n").expect("write foo.luau");
        document_links(&main_beside(text, directory.path()))
    }

    #[test]
    fn a_statement_level_require_links() {
        let links = links_beside_foo("require(\"./foo\")\n");
        assert_eq!(links.len(), 1, "a side-effect `require` resolves like any other: {links:?}");
    }

    #[test]
    fn a_require_through_the_global_table_links() {
        let links = links_beside_foo("local m = _G.require(\"./foo\")\n");
        assert_eq!(links.len(), 1, "a build bundles `_G.require(\"./foo\")`: {links:?}");
    }

    #[test]
    fn a_shadowed_require_does_not_link() {
        // The bundler leaves a call through a local `require` alone (W005),
        // so a build doesn't resolve it to the module.
        let links = links_beside_foo(
            "\
local require = print
local m = require(\"./foo\")
",
        );
        assert!(links.is_empty(), "a local `require` isn't the module loader: {links:?}");
    }

    #[test]
    fn a_long_string_require_links() {
        // The bundler decodes long-bracket require strings.
        let links = links_beside_foo("local m = require[[./foo]]\n");
        assert_eq!(links.len(), 1, "`require[[./foo]]` resolves in a build: {links:?}");
    }

    #[test]
    fn an_escaped_require_string_links() {
        // The bundler decodes escapes, so this requires `./foo`.
        let links = links_beside_foo("local m = require(\"./\\x66oo\")\n");
        assert_eq!(links.len(), 1, "`\"./\\x66oo\"` is the string `./foo`: {links:?}");
    }

    #[test]
    fn a_require_without_a_prefix_does_not_link() {
        let links = links_beside_foo("local m = require(\"foo\")\n");
        assert!(links.is_empty(), "a require needs `./`, `../`, or `@`: {links:?}");
    }
}
