//! Document links.
//!
//! A `require("path")` string becomes a clickable link resolved through
//! [`luck_resolver`], the resolver the bundler uses, so links follow Lua
//! template search paths, Luau relative and `@alias` imports, and the `init`
//! parent-parent rule exactly as a build does.

use std::path::Path;

use luck_ast::{Expression, FunctionArgs, FunctionCall, Var, Visitor};
use luck_core::normalize_path;
use luck_resolver::{ResolveRequest, Resolver};
use luck_token::short_string_body;
use tower_lsp::lsp_types::{DocumentLink, Url};

use crate::document::DocumentState;
use crate::settings::ProjectSettings;

pub(crate) fn document_links(
    document: &DocumentState,
    document_uri: &Url,
    settings: &ProjectSettings,
) -> Vec<DocumentLink> {
    let Ok(document_path) = document_uri.to_file_path() else {
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
        from_file: normalize_path(&document_path),
        resolver: Resolver::new(
            document.target.lua_version(),
            &settings.search_paths,
            project_root,
        ),
        links: Vec::new(),
    };
    collector.visit_block(&document.parsed.block);
    collector.links
}

struct LinkCollector<'document> {
    document: &'document DocumentState,
    from_file: String,
    resolver: Resolver<'document>,
    links: Vec<DocumentLink>,
}

impl<'ast> Visitor<'ast> for LinkCollector<'_> {
    fn visit_expression(&mut self, expr: &'ast Expression) {
        if let Expression::FunctionCall(call) = expr {
            self.try_record(call);
        }
        self.walk_expression(expr);
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
        if token.identifier() != Some("require") {
            return;
        }
        let literal = match &call.args {
            FunctionArgs::Parenthesized { args, .. } => {
                let Some(Expression::StringLiteral(literal)) = args.first() else {
                    return;
                };
                literal
            }
            FunctionArgs::StringLiteral(literal) => literal,
            FunctionArgs::TableConstructor(_) => return,
        };

        let Some(module) = short_string_body(&literal.text).filter(|module| !module.is_empty())
        else {
            return;
        };
        let Ok(resolved) = self.resolver.resolve(&ResolveRequest {
            module,
            from_file: &self.from_file,
            span: literal.span,
        }) else {
            return;
        };
        let Ok(target) = Url::from_file_path(&resolved.path) else {
            return;
        };
        self.links.push(DocumentLink {
            range: self.document.range(literal.span),
            target: Some(target),
            tooltip: Some(module.to_owned()),
            data: None,
        });
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use luck_core::LuaTarget;

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
        let uri = Url::from_file_path(&main_path).expect("file path URI");

        let settings = ProjectSettings::default();
        assert!(settings.root.is_none(), "the fallback needs no project root");
        let document = DocumentState::new(text.to_owned(), 1, LuaTarget::Lua54);

        let links = document_links(&document, &uri, &settings);
        assert_eq!(links.len(), 1, "{links:?}");
        let target = links[0].target.as_ref().expect("link target");
        assert!(target.path().ends_with("foo.lua"), "{target}");
    }
}
