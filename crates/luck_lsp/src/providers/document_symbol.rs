//! Document symbols for the outline: functions, methods, locals, and
//! function-valued assignments, with each function's own declarations nested
//! under it.

#![expect(
    deprecated,
    reason = "`DocumentSymbol` has no constructor, so building one names its deprecated field"
)]

use luck_ast::{Block, Expression, FunctionBody, FunctionName, Statement, Var, Visitor};
use luck_token::Span;
use tower_lsp::lsp_types::{DocumentSymbol, SymbolKind};

use crate::document::DocumentState;

pub(crate) fn document_symbols(document: &DocumentState) -> Vec<DocumentSymbol> {
    outline(document, &document.parsed.block)
}

/// The symbols declared in `block` and in the control-flow blocks nested in
/// it. A function's declarations nest under the function's own symbol.
fn outline(document: &DocumentState, block: &Block) -> Vec<DocumentSymbol> {
    let mut emitter = SymbolEmitter { document, symbols: Vec::new() };
    emitter.visit_block(block);
    emitter.symbols
}

struct SymbolEmitter<'document> {
    document: &'document DocumentState,
    symbols: Vec<DocumentSymbol>,
}

impl<'ast> Visitor<'ast> for SymbolEmitter<'_> {
    fn visit_stmt(&mut self, stmt: &'ast Statement) {
        match stmt {
            Statement::FunctionDecl(declaration) => {
                let (name, kind) = function_name_label(&declaration.name);
                self.push_function(&name, kind, &declaration.body, declaration.span);
            }
            Statement::LocalFunction(local_function) => {
                if let Some(name) = local_function.name.identifier() {
                    self.push_function(
                        name,
                        SymbolKind::FUNCTION,
                        &local_function.body,
                        local_function.span,
                    );
                }
            }
            Statement::LocalAssignment(local) => {
                for (index, name_token) in local.names.iter().enumerate() {
                    let Some(name) = name_token.name.identifier() else {
                        continue;
                    };
                    let initializer =
                        local.exprs.as_ref().and_then(|exprs| exprs.iter().nth(index));
                    if let Some(Expression::FunctionDef(function)) = initializer {
                        self.push_function(name, SymbolKind::FUNCTION, &function.body, local.span);
                        continue;
                    }
                    self.symbols.push(DocumentSymbol {
                        name: name.to_owned(),
                        detail: Some("local".to_owned()),
                        kind: SymbolKind::VARIABLE,
                        tags: None,
                        deprecated: None,
                        range: self.document.range(local.span),
                        selection_range: self.document.range(name_token.name.span),
                        children: None,
                    });
                }
            }
            Statement::Assignment(assignment) => {
                // Function-table assignment: `Module.thing = function() ... end`
                for (index, var) in assignment.targets.iter().enumerate() {
                    let Some(Expression::FunctionDef(function)) =
                        assignment.values.iter().nth(index)
                    else {
                        continue;
                    };
                    let (name, span) = match var {
                        Var::Name(token) => {
                            let Some(name) = token.identifier() else {
                                continue;
                            };
                            (name, token.span)
                        }
                        Var::FieldAccess(access) => {
                            (access.span.text(self.document.text()), access.span)
                        }
                        Var::Index(_) => continue,
                    };
                    self.push_function(name, SymbolKind::FUNCTION, &function.body, span);
                }
            }
            Statement::DoBlock(_)
            | Statement::WhileLoop(_)
            | Statement::RepeatLoop(_)
            | Statement::IfStatement(_)
            | Statement::NumericFor(_)
            | Statement::GenericFor(_) => self.walk_stmt(stmt),
            Statement::FunctionCall(_)
            | Statement::EmptyStatement(_)
            | Statement::Goto(_)
            | Statement::Label(_)
            | Statement::GlobalDeclaration(_)
            | Statement::GlobalFunction(_)
            | Statement::GlobalStar(_)
            | Statement::Break(_)
            | Statement::CompoundAssignment(_)
            | Statement::TypeDeclaration(_)
            | Statement::Error(_) => {}
        }
    }

    // A function inside an expression gets no symbol, so neither do its
    // declarations.
    fn visit_expr(&mut self, _expr: &'ast Expression) {}
}

impl SymbolEmitter<'_> {
    fn push_function(&mut self, name: &str, kind: SymbolKind, body: &FunctionBody, span: Span) {
        let children = outline(self.document, &body.block);
        let range = self.document.range(span);
        self.symbols.push(DocumentSymbol {
            name: name.to_owned(),
            detail: Some(format_params(body)),
            kind,
            tags: None,
            deprecated: None,
            range,
            selection_range: range,
            children: (!children.is_empty()).then_some(children),
        });
    }
}

fn function_name_label(name: &FunctionName) -> (String, SymbolKind) {
    let mut label = String::new();
    for identifier in name.names.iter().filter_map(|token| token.identifier()) {
        if !label.is_empty() {
            label.push('.');
        }
        label.push_str(identifier);
    }
    if let Some(method) = &name.method {
        label.push(':');
        if let Some(identifier) = method.identifier() {
            label.push_str(identifier);
        }
    }
    let kind = if name.method.is_none() && name.names.len() <= 1 {
        SymbolKind::FUNCTION
    } else {
        SymbolKind::METHOD
    };
    (label, kind)
}

fn format_params(body: &FunctionBody) -> String {
    let mut parts: Vec<&str> =
        body.params.iter().filter_map(|param| param.name.identifier()).collect();
    if body.vararg.is_some() {
        parts.push("...");
    }
    format!("({})", parts.join(", "))
}
