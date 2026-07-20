//! Document symbols (outline) provider. Walks the AST emitting one
//! `DocumentSymbol` per declaration: top-level functions, methods,
//! locals, nested function definitions.

#![allow(deprecated)]

use luck_ast::expr::{Expression, Var};
use luck_ast::shared::{Block, FunctionBody};
use luck_ast::stmt::{FuncName, Statement};
use luck_token::{Span, TokenKind};
use tower_lsp::lsp_types::{DocumentSymbol, DocumentSymbolResponse, SymbolKind};

use crate::backend::DocumentState;
use crate::line_index::LineIndex;

#[must_use]
pub fn document_symbols(doc: &DocumentState) -> DocumentSymbolResponse {
    let mut emitter = SymbolEmitter {
        line_index: &doc.line_index,
        source: &doc.text,
        out: Vec::new(),
    };
    emitter.emit_block(&doc.parsed.block);
    DocumentSymbolResponse::Nested(emitter.out)
}

struct SymbolEmitter<'a> {
    line_index: &'a LineIndex,
    source: &'a str,
    out: Vec<DocumentSymbol>,
}

impl SymbolEmitter<'_> {
    fn emit_block(&mut self, block: &Block) {
        for stmt in &block.stmts {
            self.emit_stmt(stmt);
        }
    }

    fn emit_stmt(&mut self, stmt: &Statement) {
        match stmt {
            Statement::FunctionDecl(decl) => {
                let (name, kind) = func_name_label(&decl.name);
                if let Some(symbol) = self.build_function_symbol(&name, kind, &decl.body, decl.span)
                {
                    self.out.push(symbol);
                }
            }
            Statement::LocalFunction(local_fn) => {
                if let TokenKind::Identifier(name) = &local_fn.name.kind {
                    if let Some(symbol) = self.build_function_symbol(
                        name.as_str(),
                        SymbolKind::FUNCTION,
                        &local_fn.body,
                        local_fn.span,
                    ) {
                        self.out.push(symbol);
                    }
                }
            }
            Statement::LocalAssignment(local) => {
                for (idx, name_token) in local.names.iter().enumerate() {
                    if let TokenKind::Identifier(name) = &name_token.name.kind {
                        let initializer =
                            local.exprs.as_ref().and_then(|exprs| exprs.iter().nth(idx));
                        if let Some(Expression::FunctionDef(def)) = initializer {
                            if let Some(symbol) = self.build_function_symbol(
                                name.as_str(),
                                SymbolKind::FUNCTION,
                                &def.body,
                                local.span,
                            ) {
                                self.out.push(symbol);
                                continue;
                            }
                        }
                        self.out.push(DocumentSymbol {
                            name: name.to_string(),
                            detail: Some("local".to_string()),
                            kind: SymbolKind::VARIABLE,
                            tags: None,
                            deprecated: None,
                            range: self.range(local.span),
                            selection_range: self.range(name_token.name.span),
                            children: None,
                        });
                    }
                }
            }
            Statement::Assignment(assign) => {
                // Function-table assignment: `Module.thing = function() ... end`
                for (idx, var) in assign.targets.iter().enumerate() {
                    let value = assign.values.iter().nth(idx);
                    let Some(value) = value else { continue };
                    let Expression::FunctionDef(def) = value else {
                        continue;
                    };
                    let (name, span) = match var {
                        Var::Name(token) => match &token.kind {
                            TokenKind::Identifier(name) => (name.to_string(), token.span),
                            _ => continue,
                        },
                        Var::FieldAccess(fa) => {
                            let name = self.slice(fa.span).to_string();
                            (name, fa.span)
                        }
                        Var::Index(_) => continue,
                    };
                    if let Some(symbol) =
                        self.build_function_symbol(&name, SymbolKind::FUNCTION, &def.body, span)
                    {
                        self.out.push(symbol);
                    }
                }
            }
            Statement::DoBlock(do_block) => self.emit_block(&do_block.block),
            Statement::WhileLoop(loop_) => self.emit_block(&loop_.block),
            Statement::RepeatLoop(loop_) => self.emit_block(&loop_.block),
            Statement::IfStatement(if_stmt) => {
                self.emit_block(&if_stmt.block);
                for clause in &if_stmt.elseif_clauses {
                    self.emit_block(&clause.block);
                }
                if let Some(else_clause) = &if_stmt.else_clause {
                    self.emit_block(&else_clause.block);
                }
            }
            Statement::NumericFor(nfor) => self.emit_block(&nfor.block),
            Statement::GenericFor(gfor) => self.emit_block(&gfor.block),
            _ => {}
        }
    }

    fn build_function_symbol(
        &mut self,
        name: &str,
        kind: SymbolKind,
        body: &FunctionBody,
        outer: Span,
    ) -> Option<DocumentSymbol> {
        let mut child_emitter = SymbolEmitter {
            line_index: self.line_index,
            source: self.source,
            out: Vec::new(),
        };
        child_emitter.emit_block(&body.block);
        let children = if child_emitter.out.is_empty() {
            None
        } else {
            Some(child_emitter.out)
        };
        Some(DocumentSymbol {
            name: name.to_string(),
            detail: Some(format_params(body)),
            kind,
            tags: None,
            deprecated: None,
            range: self.range(outer),
            selection_range: self.range(outer),
            children,
        })
    }

    fn range(&self, span: Span) -> tower_lsp::lsp_types::Range {
        self.line_index.range(self.source, span.start, span.end)
    }

    fn slice(&self, span: Span) -> &str {
        &self.source[span.start as usize..span.end as usize]
    }
}

fn func_name_label(name: &FuncName) -> (String, SymbolKind) {
    let mut label = String::new();
    let mut first = true;
    for token in &name.names {
        if let TokenKind::Identifier(ident) = &token.kind {
            if !first {
                label.push('.');
            }
            label.push_str(ident.as_str());
            first = false;
        }
    }
    let kind = if let Some(method) = &name.method {
        label.push(':');
        if let TokenKind::Identifier(ident) = &method.kind {
            label.push_str(ident.as_str());
        }
        SymbolKind::METHOD
    } else if name.names.len() <= 1 {
        SymbolKind::FUNCTION
    } else {
        SymbolKind::METHOD
    };
    (label, kind)
}

fn format_params(body: &FunctionBody) -> String {
    let mut parts: Vec<String> = body
        .params
        .iter()
        .filter_map(|p| match &p.name.kind {
            TokenKind::Identifier(name) => Some(name.to_string()),
            _ => None,
        })
        .collect();
    if body.vararg.is_some() {
        parts.push("...".to_string());
    }
    format!("({})", parts.join(", "))
}
