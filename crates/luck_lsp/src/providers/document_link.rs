//! A `require("path")` string becomes a clickable link resolved through
//! [`luck_resolver`], the resolver the bundler uses.
//!
//! Links follow Lua template search paths, Luau relative and `@alias` imports,
//! and the `init` parent-parent rule exactly as a build does.

use std::path::{Path, PathBuf};

use luck_ast::{Block, Call, Expression, FunctionArgs, Visitor};
use luck_resolver::{ResolveOptions, Resolver};
use luck_token::decode_string;
use tower_lsp_server::ls_types::{DocumentLink, Position, Uri};

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
    let settings = &document.settings().project;
    // Lua templates resolve against the project root; with no config, fall
    // back to the requiring file's own directory so bare siblings still link.
    let project_root = settings
        .root
        .as_deref()
        .or_else(|| document_path.parent())
        .unwrap_or_else(|| Path::new("."));
    let resolver = Resolver::new(ResolveOptions {
        version: document.lua_version(),
        search_paths: settings.search_paths.clone(),
        root: project_root.to_path_buf(),
    });
    let mut collector = LinkCollector {
        document,
        position,
        offset: position.map(|at| document.offset(at)),
        document_path,
        resolver,
        links: Vec::new(),
    };
    collector.visit_block(&document.parse().block);
    collector.links
}

struct LinkCollector<'document> {
    document: &'document DocumentState,
    position: Option<Position>,
    /// Where `position` falls, which every link at it touches, so the walk
    /// reads only what does.
    offset: Option<u32>,
    document_path: PathBuf,
    resolver: Resolver,
    links: Vec<DocumentLink>,
}

impl<'ast> Visitor<'ast> for LinkCollector<'_> {
    fn visit_block(&mut self, block: &'ast Block<'ast>) {
        let Some(offset) = self.offset else {
            self.walk_block(block);
            return;
        };
        for stmt in statements_touching(block, offset) {
            self.visit_stmt(stmt);
        }
        if let Some(last) = &block.last_stmt
            && touches(last.span(), offset)
        {
            self.visit_last_stmt(last);
        }
    }

    fn visit_expr(&mut self, expr: &'ast Expression<'ast>) {
        if self.offset.is_none_or(|offset| touches(expr.span(), offset)) {
            self.walk_expr(expr);
        }
    }

    fn visit_call(&mut self, call: Call<'ast>) {
        self.try_record(call);
        self.walk_call(call);
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
        let Some(module) = decode_string(literal.text, self.document.lua_version())
            .and_then(|decoded| String::from_utf8(decoded.into_owned()).ok())
            .filter(|module| !module.is_empty())
        else {
            return;
        };
        let Ok(resolution) = self.resolver.resolve(&self.document_path, &module) else {
            return;
        };
        let Some(target) = Uri::from_file_path(&resolution.path) else {
            return;
        };
        self.links.push(DocumentLink {
            range,
            target: Some(target),
            tooltip: Some(module),
            data: None,
        });
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use luck_core::LuaTarget;

    use super::*;
    use crate::fixtures::{document, document_under};
    use crate::settings::ProjectSettings;

    /// The unrooted fallback, driven from settings the test owns. Going
    /// through `SettingsCache` instead would let discovery walk above the
    /// temp directory and pick up whatever `luck.json` sits there.
    #[test]
    fn bare_sibling_resolves_against_the_requiring_file() {
        let directory = tempfile::tempdir().expect("tempdir");
        fs::write(directory.path().join("foo.lua"), "return {}\n").expect("write foo.lua");
        let main_path = directory.path().join("main.lua");
        let text = "local foo = require(\"foo\")\n";
        fs::write(&main_path, text).expect("write main.lua");
        let uri = Uri::from_file_path(&main_path).expect("file path URI");

        let document = document(text, LuaTarget::Lua54);
        assert!(document.settings().project.root.is_none(), "the fallback needs no project root");

        let links = document_links(&document, &uri);
        assert_eq!(links.len(), 1, "{links:?}");
        let target = links[0].target.as_ref().expect("link target");
        assert!(target.path().as_str().ends_with("foo.lua"), "{}", target.as_str());
    }

    #[test]
    fn empty_search_paths_link_like_a_build() {
        let directory = tempfile::tempdir().expect("tempdir");
        fs::write(directory.path().join("dep.lua"), "return {}\n").expect("write dep.lua");
        let main_path = directory.path().join("main.lua");
        let text = "local dep = require(\"dep\")\n";
        fs::write(&main_path, text).expect("write main.lua");
        let uri = Uri::from_file_path(&main_path).expect("file path URI");

        let settings = ProjectSettings {
            search_paths: Vec::new(),
            root: Some(directory.path().to_path_buf()),
            ..ProjectSettings::default()
        };
        let document = document_under(text, LuaTarget::Lua54, settings);

        let links = document_links(&document, &uri);
        assert_eq!(links.len(), 1, "{links:?}");
        let target = links[0].target.as_ref().expect("link target");
        assert!(target.path().as_str().ends_with("dep.lua"), "{}", target.as_str());
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
        let document = document(text, LuaTarget::Lua54);

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
        document_links(&document(text, LuaTarget::Lua54), &uri)
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
