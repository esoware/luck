//! A `require("path")` string becomes a clickable link resolved through
//! [`luck_resolver`], the resolver the bundler uses.
//!
//! Links follow Lua template search paths, Luau relative and `@alias` imports,
//! and the `init` parent-parent rule exactly as a build does.

use std::path::Path;

use luck_ast::{Expression, FunctionArgs, FunctionCall, Var, Visitor};
use luck_resolver::{ResolveRequest, Resolver, normalize_path};
use luck_token::decode_string;
use tower_lsp_server::ls_types::{DocumentLink, Position, Uri};

use crate::document::{DocumentState, file_path};
use crate::settings::ProjectSettings;

pub(crate) fn document_links(
    document: &DocumentState,
    document_uri: &Uri,
    settings: &ProjectSettings,
) -> Vec<DocumentLink> {
    collect_links(document, document_uri, settings, None)
}

pub(crate) fn target_at(
    document: &DocumentState,
    document_uri: &Uri,
    settings: &ProjectSettings,
    position: Position,
) -> Option<Uri> {
    collect_links(document, document_uri, settings, Some(position))
        .into_iter()
        .find_map(|link| link.target)
}

fn collect_links(
    document: &DocumentState,
    document_uri: &Uri,
    settings: &ProjectSettings,
    position: Option<Position>,
) -> Vec<DocumentLink> {
    let Some(document_path) = file_path(document_uri) else {
        return Vec::new();
    };
    // Lua templates resolve against the project root; with no config, fall
    // back to the requiring file's own directory so bare siblings still link.
    let project_root = settings
        .root
        .as_deref()
        .or_else(|| document_path.parent())
        .unwrap_or_else(|| Path::new("."));
    let mut collector = LinkCollector {
        document,
        position,
        from_file: normalize_path(&document_path),
        resolver: Resolver::new(document.lua_version, &settings.search_paths, project_root),
        links: Vec::new(),
    };
    collector.visit_block(&document.parsed.block);
    collector.links
}

struct LinkCollector<'document> {
    document: &'document DocumentState,
    position: Option<Position>,
    from_file: String,
    resolver: Resolver<'document>,
    links: Vec<DocumentLink>,
}

impl<'ast> Visitor<'ast> for LinkCollector<'_> {
    fn visit_function_call(&mut self, call: &'ast FunctionCall) {
        self.try_record(call);
        self.walk_function_call(call);
    }
}

impl LinkCollector<'_> {
    fn try_record(&mut self, call: &FunctionCall) {
        if call.method.is_some() {
            return;
        }
        let Expression::Var(Var::Name(token)) = &call.callee else {
            return;
        };
        // A local `require`, a local `_ENV`, or a rebound global isn't the
        // module loader, so a build leaves the call alone too.
        if token.identifier() != Some("require")
            || !self.document.semantic.reaches_host_global("require", token.span)
        {
            return;
        }
        let literal = match &call.args {
            FunctionArgs::Parenthesized { args, .. } => {
                let [Expression::StringLiteral(literal)] = args.items.as_slice() else {
                    return;
                };
                literal
            }
            FunctionArgs::StringLiteral(literal) => literal,
            FunctionArgs::TableConstructor(_) => return,
        };

        let range = self.document.range(literal.span);
        if self.position.is_some_and(|position| position < range.start || position > range.end) {
            return;
        }
        let Some(module) = decode_string(&literal.text, self.document.lua_version)
            .and_then(|decoded| String::from_utf8(decoded.into_owned()).ok())
            .filter(|module| !module.is_empty())
        else {
            return;
        };
        let Ok(resolved) = self.resolver.resolve(&ResolveRequest {
            module: &module,
            from_file: &self.from_file,
            span: literal.span,
        }) else {
            return;
        };
        let Some(target) = Uri::from_file_path(&resolved.path) else {
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
    use rustc_hash::FxHashSet;

    use super::*;

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

        let settings = ProjectSettings::default();
        assert!(settings.root.is_none(), "the fallback needs no project root");
        let document =
            DocumentState::new(text.to_owned(), 1, LuaTarget::Lua54, FxHashSet::default());

        let links = document_links(&document, &uri, &settings);
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
        let document =
            DocumentState::new(text.to_owned(), 1, LuaTarget::Lua54, FxHashSet::default());

        let links = document_links(&document, &uri, &settings);
        assert_eq!(links.len(), 1, "{links:?}");
        let target = links[0].target.as_ref().expect("link target");
        assert!(target.path().as_str().ends_with("dep.lua"), "{}", target.as_str());
    }

    #[test]
    fn target_at_resolves_the_require_string_under_the_position() {
        let directory = tempfile::tempdir().expect("tempdir");
        fs::write(directory.path().join("foo.lua"), "return {}\n").expect("write foo.lua");
        let main_path = directory.path().join("main.lua");
        let text = "local foo = require(\"foo\")\nlocal x = 1\n";
        fs::write(&main_path, text).expect("write main.lua");
        let uri = Uri::from_file_path(&main_path).expect("file path URI");
        let settings = ProjectSettings::default();
        let document =
            DocumentState::new(text.to_owned(), 1, LuaTarget::Lua54, FxHashSet::default());

        // Both ends of the string count.
        for utf16_column in [20, 25] {
            let target = target_at(&document, &uri, &settings, Position::new(0, utf16_column))
                .expect("the require string resolves");
            assert!(target.path().as_str().ends_with("foo.lua"), "{}", target.as_str());
        }
        assert_eq!(target_at(&document, &uri, &settings, Position::new(0, 19)), None);
        assert_eq!(target_at(&document, &uri, &settings, Position::new(1, 6)), None);
    }

    fn links_beside_foo(text: &str) -> Vec<DocumentLink> {
        let directory = tempfile::tempdir().expect("tempdir");
        fs::write(directory.path().join("foo.lua"), "return {}\n").expect("write foo.lua");
        let main_path = directory.path().join("main.lua");
        fs::write(&main_path, text).expect("write main.lua");
        let uri = Uri::from_file_path(&main_path).expect("file path URI");
        let settings = ProjectSettings::default();
        let document =
            DocumentState::new(text.to_owned(), 1, LuaTarget::Lua54, FxHashSet::default());
        document_links(&document, &uri, &settings)
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
        let links = links_beside_foo("local require = print\nlocal m = require(\"foo\")\n");
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
