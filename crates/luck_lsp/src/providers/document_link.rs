//! Document links. Every `require("path")` literal string becomes a
//! clickable link that the editor can resolve to a file. Resolution
//! walks `luck_resolver`'s default search paths.

use std::path::{Path, PathBuf};

use luck_ast::expr::{Expression, FunctionArgs, FunctionCall, Var};
use luck_ast::visitor::Visitor;
use luck_token::TokenKind;
use tower_lsp::lsp_types::{DocumentLink, Url};

use crate::backend::DocumentState;

#[must_use]
pub fn document_links(doc: &DocumentState, document_uri: &Url) -> Vec<DocumentLink> {
    let base_dir = document_uri
        .to_file_path()
        .ok()
        .and_then(|path| path.parent().map(PathBuf::from));
    let mut collector = LinkCollector {
        doc,
        base_dir,
        out: Vec::new(),
    };
    collector.visit_block(&doc.parsed.block);
    collector.out
}

struct LinkCollector<'a> {
    doc: &'a DocumentState,
    base_dir: Option<PathBuf>,
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
        // We only handle `require("...")` calls.
        if call.method.is_some() {
            return;
        }
        let Expression::Var(var) = &call.callee else {
            return;
        };
        let Var::Name(token) = var.as_ref() else {
            return;
        };
        let TokenKind::Identifier(name) = &token.kind else {
            return;
        };
        if name.as_str() != "require" {
            return;
        }

        let module = match &call.args {
            FunctionArgs::Parenthesized { args, .. } => args.first(),
            FunctionArgs::StringLiteral(literal) => {
                self.record_string(literal);
                return;
            }
            FunctionArgs::TableConstructor(_) => None,
        };
        let Some(Expression::StringLiteral(literal)) = module else {
            return;
        };
        self.record_string(literal);
    }

    fn record_string(&mut self, literal: &luck_ast::expr::Literal) {
        let raw = &self.doc.text[literal.span.start as usize..literal.span.end as usize];
        let trimmed = trim_string_literal(raw);
        if trimmed.is_empty() {
            return;
        }
        let Some(base_dir) = &self.base_dir else {
            return;
        };
        let candidate = resolve_module(base_dir, trimmed);
        let target = candidate.and_then(|p| Url::from_file_path(p).ok());
        if let Some(target) = target {
            let range =
                self.doc
                    .line_index
                    .range(&self.doc.text, literal.span.start, literal.span.end);
            self.out.push(DocumentLink {
                range,
                target: Some(target),
                tooltip: Some(trimmed.to_string()),
                data: None,
            });
        }
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

fn resolve_module(base: &Path, module: &str) -> Option<PathBuf> {
    // The dotted-path style: `foo.bar.baz` -> `foo/bar/baz`.
    let relative = module.replace('.', std::path::MAIN_SEPARATOR_STR);
    let mut candidates = vec![
        base.join(format!("{relative}.lua")),
        base.join(format!("{relative}.luau")),
        base.join(&relative).join("init.lua"),
        base.join(&relative).join("init.luau"),
    ];
    // Also try literal as-is (e.g. ./foo/bar).
    candidates.push(base.join(module));
    candidates.into_iter().find(|p| p.exists())
}
