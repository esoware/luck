#![expect(
    deprecated,
    reason = "`DocumentSymbol` has no constructor, so building one names its deprecated field"
)]

use luck_ast::{
    Assignment, Block, Expression, FunctionBody, FunctionName, Statement, TypeDeclarationValue,
    Var, Visitor,
};
use luck_token::{Span, Token};
use tower_lsp_server::ls_types::{DocumentSymbol, SymbolKind};

use crate::document::DocumentState;

pub(crate) fn document_symbols(document: &DocumentState) -> Vec<DocumentSymbol> {
    outline(document, &document.parsed.block)
}

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
                if !name.is_empty() {
                    self.push_function(
                        &name,
                        kind,
                        &declaration.body,
                        declaration.span,
                        declaration.name.span,
                    );
                }
            }
            Statement::LocalFunction(local_function) => self.push_named_function(
                &local_function.name,
                &local_function.body,
                local_function.span,
            ),
            // Lua 5.5
            Statement::GlobalFunction(global_function) => self.push_named_function(
                &global_function.name,
                &global_function.body,
                global_function.span,
            ),
            // Luau
            Statement::TypeDeclaration(declaration) => {
                if let TypeDeclarationValue::TypeFunction(body) = &declaration.type_value {
                    self.push_named_function(&declaration.name, body, declaration.span);
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
                        self.push_function(
                            name,
                            SymbolKind::FUNCTION,
                            &function.body,
                            local.span,
                            name_token.name.span,
                        );
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
            Statement::Assignment(assignment) => self.push_assigned_functions(assignment),
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
            | Statement::GlobalStar(_)
            | Statement::Break(_)
            | Statement::CompoundAssignment(_)
            | Statement::Error(_) => {}
        }
    }

    // A function inside an expression gets no symbol, so neither do its
    // declarations.
    fn visit_expr(&mut self, _expr: &'ast Expression) {}
}

impl SymbolEmitter<'_> {
    fn push_assigned_functions(&mut self, assignment: &Assignment) {
        for (index, var) in assignment.targets.iter().enumerate() {
            let Some(Expression::FunctionDef(function)) = assignment.values.iter().nth(index)
            else {
                continue;
            };
            let (label, selection_span) = match var {
                Var::Name(token) => (token.identifier(), token.span),
                Var::FieldAccess(access) => (
                    access.name.identifier().map(|_| access.span.text(self.document.text())),
                    access.span,
                ),
                Var::Index(_) => continue,
            };
            if let Some(name) = label {
                self.push_function(
                    name,
                    SymbolKind::FUNCTION,
                    &function.body,
                    assignment.span,
                    selection_span,
                );
            }
        }
    }

    fn push_named_function(&mut self, name: &Token, body: &FunctionBody, span: Span) {
        if let Some(label) = name.identifier() {
            self.push_function(label, SymbolKind::FUNCTION, body, span, name.span);
        }
    }

    /// `span` is the declaration, and `selection_span` the name within it,
    /// which LSP wants the editor to reveal.
    fn push_function(
        &mut self,
        name: &str,
        kind: SymbolKind,
        body: &FunctionBody,
        span: Span,
        selection_span: Span,
    ) {
        let children = outline(self.document, &body.block);
        self.symbols.push(DocumentSymbol {
            name: name.to_owned(),
            detail: Some(format_params(body)),
            kind,
            tags: None,
            deprecated: None,
            range: self.document.range(span),
            selection_range: self.document.range(selection_span),
            children: (!children.is_empty()).then_some(children),
        });
    }
}

/// The label for `name`, empty when recovery left no real name in it.
fn function_name_label(name: &FunctionName) -> (String, SymbolKind) {
    let mut label = String::new();
    for identifier in name.names.iter().filter_map(Token::identifier) {
        if !label.is_empty() {
            label.push('.');
        }
        label.push_str(identifier);
    }
    if let Some(method) = name.method.as_ref().and_then(Token::identifier) {
        label.push(':');
        label.push_str(method);
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

#[cfg(test)]
mod tests {
    use luck_core::LuaTarget;
    use rustc_hash::FxHashSet;

    use super::*;

    fn symbols_of(text: &str, target: LuaTarget) -> Vec<DocumentSymbol> {
        let document = DocumentState::new(text.to_owned(), 1, target, FxHashSet::default());
        document_symbols(&document)
    }

    #[test]
    fn assigned_function_symbol_range_covers_its_body() {
        let symbols = symbols_of("M.foo = function()\n  local x = 1\nend\n", LuaTarget::Lua54);
        let foo = symbols.first().expect("the assigned function is outlined");
        assert_eq!(
            foo.range.end.line, 2,
            "the symbol's range must enclose the function, not just `M.foo`: {foo:?}"
        );
    }

    #[test]
    fn a_recovered_function_name_yields_no_empty_symbol() {
        // LSP: a symbol's name "must not be an empty string".
        let symbols = symbols_of("local function (x) end\n", LuaTarget::Lua54);
        assert!(
            symbols.iter().all(|symbol| !symbol.name.trim().is_empty()),
            "a nameless function being typed must not produce an empty symbol: {symbols:?}"
        );
    }

    #[test]
    fn lua55_global_function_is_outlined() {
        let symbols = symbols_of("global function setup()\nend\n", LuaTarget::Lua55);
        assert!(
            symbols.iter().any(|symbol| symbol.name == "setup"),
            "a `global function` is a function in the outline: {symbols:?}"
        );
    }
}
