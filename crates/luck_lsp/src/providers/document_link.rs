//! A `require("path")` string becomes a clickable link resolved through
//! [`luck_resolver`], the resolver the bundler uses, which the project keeps
//! for every request until the file watcher clears it.
//!
//! Links follow Lua template search paths, Luau relative and `@alias` imports,
//! and the `init` parent-parent rule exactly as a build does.

use std::path::PathBuf;

use ls_types::{DocumentLink, Position, Uri};
use luck_ast::{Block, Call, Expression, FunctionArgs, GetSpan as _, SuffixedExpression, Visit};
use luck_resolver::Resolver;

use crate::document::{DocumentState, file_path};
use crate::providers::cursor::{statements_touching, touches};

pub(crate) fn document_links(document: &DocumentState, document_uri: &Uri) -> Vec<DocumentLink> {
    collect_links(document, document_uri, None)
}

pub(crate) fn target_at(
    document: &DocumentState,
    document_uri: &Uri,
    position: Position,
) -> Option<Uri> {
    collect_links(document, document_uri, Some(position)).into_iter().find_map(|link| link.target)
}

fn collect_links(
    document: &DocumentState,
    document_uri: &Uri,
    position: Option<Position>,
) -> Vec<DocumentLink> {
    let Some(document_path) = file_path(document_uri) else {
        return Vec::new();
    };
    // A document with no `luck.json` above it is under the defaults rooted
    // at its own directory, so bare siblings link as they bundle.
    let mut collector = LinkCollector {
        document,
        position,
        offset: position.map(|at| document.offset(at)),
        document_path,
        resolver: document.settings().project.resolver(document.version()),
        links: Vec::new(),
    };
    collector.visit_block(&document.parse().chunk.block);
    collector.links
}

struct LinkCollector<'document> {
    document: &'document DocumentState,
    position: Option<Position>,
    /// Where `position` falls, which every link at it touches, so the walk
    /// reads only what does.
    offset: Option<u32>,
    document_path: PathBuf,
    resolver: &'document Resolver,
    links: Vec<DocumentLink>,
}

impl<'ast> Visit<'ast> for LinkCollector<'_> {
    fn visit_block(&mut self, block: &'ast Block<'ast>) {
        let Some(offset) = self.offset else {
            self.walk_block(block);
            return;
        };
        for stmt in statements_touching(block, offset) {
            self.visit_stmt(stmt);
        }
    }

    fn visit_expr(&mut self, expr: &'ast Expression<'ast>) {
        if self.offset.is_none_or(|offset| touches(expr.span(), offset)) {
            self.walk_expr(expr);
        }
    }

    fn visit_suffixed_expr(&mut self, suffixed_expr: &'ast SuffixedExpression<'ast>) {
        for call in suffixed_expr.calls() {
            self.try_record(call);
        }
        self.walk_suffixed_expr(suffixed_expr);
    }
}

impl LinkCollector<'_> {
    fn try_record(&mut self, call: Call<'_>) {
        if call.method.is_some() {
            return;
        }
        let Some(name) = call.callee.as_name() else {
            return;
        };
        // A local `require`, a local `_ENV`, or a rebound global isn't the
        // module loader, so a build leaves the call alone too.
        if name.text != "require" || !self.document.tree().reaches_host_global(name) {
            return;
        }
        let literal = match call.args {
            FunctionArgs::Parenthesized { args, .. } => {
                let [Expression::StringLiteral(literal)] = &**args else {
                    return;
                };
                &**literal
            }
            FunctionArgs::StringLiteral(literal) => literal,
            FunctionArgs::TableConstructor(_) | FunctionArgs::Error(_) => return,
        };

        let range = self.document.range(literal.span);
        if self.position.is_some_and(|position| position < range.start || position > range.end) {
            return;
        }
        let Some(module) = literal.as_str().filter(|module| !module.is_empty()) else {
            return;
        };
        let Ok(resolution) = self.resolver.resolve(&self.document_path, module) else {
            return;
        };
        let Some(target) = Uri::from_file_path(&resolution.path) else {
            return;
        };
        self.links.push(DocumentLink {
            range,
            target: Some(target),
            tooltip: Some(module.to_owned()),
            data: None,
        });
    }
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
