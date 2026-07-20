//! Document links. Every `require("path")` literal string becomes a
//! clickable link resolved through [`luck_resolver`] - the same resolver the
//! bundler uses, so links honor Lua template search paths, Luau relative and
//! `@alias` imports, and the `init` parent-parent rule identically.

use std::path::{Path, PathBuf};

use luck_ast::expr::{Expression, FunctionArgs, FunctionCall, Var};
use luck_ast::visitor::Visitor;
use luck_resolver::{ResolveRequest, Resolver, normalize_path_str};
use luck_token::TokenKind;
use tower_lsp::lsp_types::{DocumentLink, Url};

use crate::backend::DocumentState;
use crate::config::ProjectSettings;

#[must_use]
pub fn document_links(
    doc: &DocumentState,
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
        .clone()
        .or_else(|| document_path.parent().map(Path::to_path_buf))
        .unwrap_or_else(|| PathBuf::from("."));
    let mut collector = LinkCollector {
        doc,
        from_file: normalize_path_str(&document_path),
        project_root,
        search_paths: &settings.search_paths,
        resolver: Resolver::new(),
        out: Vec::new(),
    };
    collector.visit_block(&doc.parsed.block);
    collector.out
}

struct LinkCollector<'a> {
    doc: &'a DocumentState,
    from_file: String,
    project_root: PathBuf,
    search_paths: &'a [String],
    resolver: Resolver,
    out: Vec<DocumentLink>,
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
        let TokenKind::Identifier(name) = &token.kind else {
            return;
        };
        if name.as_str() != "require" {
            return;
        }

        let literal = match &call.args {
            FunctionArgs::Parenthesized { args, .. } => match args.first() {
                Some(Expression::StringLiteral(literal)) => literal,
                _ => return,
            },
            FunctionArgs::StringLiteral(literal) => literal,
            FunctionArgs::TableConstructor(_) => return,
        };
        self.record_string(literal);
    }

    fn record_string(&mut self, literal: &luck_ast::expr::Literal) {
        let raw = &self.doc.text[literal.span.start as usize..literal.span.end as usize];
        let module = trim_string_literal(raw);
        if module.is_empty() {
            return;
        }
        let resolved = self.resolver.resolve(&ResolveRequest {
            module,
            from_file: &self.from_file,
            target: self.doc.target,
            search_paths: self.search_paths,
            project_root: &self.project_root,
            span: literal.span,
        });
        let Ok(resolved) = resolved else {
            return;
        };
        let Ok(target) = Url::from_file_path(&resolved.path) else {
            return;
        };
        let range = self
            .doc
            .line_index
            .range(&self.doc.text, literal.span.start, literal.span.end);
        self.out.push(DocumentLink {
            range,
            target: Some(target),
            tooltip: Some(module.to_string()),
            data: None,
        });
    }
}

fn trim_string_literal(raw: &str) -> &str {
    let bytes = raw.as_bytes();
    if bytes.len() >= 2 {
        match bytes[0] {
            b'"' | b'\'' | b'`' => return &raw[1..raw.len() - 1],
            _ => {}
        }
    }
    raw
}
