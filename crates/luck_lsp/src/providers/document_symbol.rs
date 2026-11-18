#![expect(
    deprecated,
    reason = "`DocumentSymbol` has no constructor, so building one names its deprecated field"
)]

use ls_types::{DocumentSymbol, SymbolKind};
use luck_ast::{
    Assignment, Block, Expression, FunctionBody, FunctionName, Name, Statement, Suffix,
    TypeDeclarationValue, Var, Visit,
};
use luck_token::Span;

use crate::document::DocumentState;
use crate::providers::cursor::identifier;

pub(crate) fn document_symbols(document: &DocumentState) -> Vec<DocumentSymbol> {
    outline(document, &document.parse().block)
}

fn outline(document: &DocumentState, block: &Block<'_>) -> Vec<DocumentSymbol> {
    let mut emitter = SymbolEmitter { document, symbols: Vec::new() };
    emitter.visit_block(block);
    emitter.symbols
}

struct SymbolEmitter<'document> {
    document: &'document DocumentState,
    symbols: Vec<DocumentSymbol>,
}

impl<'ast> Visit<'ast> for SymbolEmitter<'_> {
    fn visit_stmt(&mut self, stmt: &'ast Statement<'ast>) {
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
                if let TypeDeclarationValue::TypeFunction(body) = &declaration.value {
                    self.push_named_function(&declaration.name, body, declaration.span);
                }
            }
            Statement::LocalAssignment(local) => {
                for (index, attributed) in local.names.iter().enumerate() {
                    let Some(name) = identifier(&attributed.name) else {
                        continue;
                    };
                    if let Some(Expression::FunctionDef(function)) = local.exprs.get(index) {
                        self.push_function(
                            name,
                            SymbolKind::FUNCTION,
                            &function.body,
                            local.span,
                            attributed.name.span,
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
                        selection_range: self.document.range(attributed.name.span),
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
    fn visit_expr(&mut self, _expr: &'ast Expression<'ast>) {}
}

impl SymbolEmitter<'_> {
    fn push_assigned_functions(&mut self, assignment: &Assignment<'_>) {
        for (index, var) in assignment.targets.iter().enumerate() {
            let Some(Expression::FunctionDef(function)) = assignment.values.get(index) else {
                continue;
            };
            let (label, selection_span) = match var {
                Var::Name(name) => (identifier(name), name.span),
                Var::Suffixed(chain) => {
                    let Suffix::Field(name) = chain.suffixes.last() else {
                        continue;
                    };
                    (identifier(name).map(|_| chain.span.text(self.document.text())), chain.span)
                }
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

    fn push_named_function(&mut self, name: &Name<'_>, body: &FunctionBody<'_>, span: Span) {
        if let Some(label) = identifier(name) {
            self.push_function(label, SymbolKind::FUNCTION, body, span, name.span);
        }
    }

    /// `span` is the declaration, and `selection_span` the name within it,
    /// which LSP wants the editor to reveal.
    fn push_function(
        &mut self,
        name: &str,
        kind: SymbolKind,
        body: &FunctionBody<'_>,
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
fn function_name_label(name: &FunctionName<'_>) -> (String, SymbolKind) {
    let mut label = String::new();
    for segment in name.names.iter().filter_map(identifier) {
        if !label.is_empty() {
            label.push('.');
        }
        label.push_str(segment);
    }
    if let Some(method) = name.method.as_ref().and_then(identifier) {
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

fn format_params(body: &FunctionBody<'_>) -> String {
    let mut parts: Vec<&str> =
        body.params.iter().filter_map(|param| identifier(&param.name)).collect();
    if body.vararg.is_some() {
        parts.push("...");
    }
    format!("({})", parts.join(", "))
}

#[cfg(test)]
mod tests {
    use luck_core::LuaTarget;

    use super::*;
    use crate::fixtures::document;

    fn symbols_of(text: &str, target: LuaTarget) -> Vec<DocumentSymbol> {
        document_symbols(&document(text, target))
    }

    #[test]
    fn assigned_function_symbol_range_covers_its_body() {
        let symbols = symbols_of(
            "\
M.foo = function()
  local x = 1
end
",
            LuaTarget::Lua54,
        );
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
        let symbols = symbols_of(
            "\
global function setup()
end
",
            LuaTarget::Lua55,
        );
        assert!(
            symbols.iter().any(|symbol| symbol.name == "setup"),
            "a `global function` is a function in the outline: {symbols:?}"
        );
    }
}
