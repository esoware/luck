use core::mem;

use luck_ast::{
    Block, FunctionBody, FunctionDecl, GenericFor, IfStatement, LocalAssignment, NumericFor,
    Statement, Var, Visitor,
};
use luck_token::{CompactString, Span, TokenKind};
use rustc_hash::FxHashMap;

use crate::scope::{
    Reference, ReferenceId, ReferenceKind, Scope, ScopeId, ScopeKind, ScopeTree, Symbol, SymbolId,
    SymbolKind,
};

/// Builds the scope tree for `block`, declaring and resolving names in the
/// order Lua makes them visible.
pub(crate) fn build(block: &Block) -> ScopeTree {
    let mut builder = ScopeTreeBuilder {
        tree: ScopeTree {
            scopes: vec![Scope { parent: None, kind: ScopeKind::Module }],
            symbols: Vec::new(),
            references: Vec::new(),
        },
        current_scope: ScopeId::from_index(0),
        outer_scopes: Vec::new(),
        bindings: FxHashMap::default(),
        declared_names: Vec::new(),
    };
    builder.visit_block(block);
    builder.tree
}

struct ScopeTreeBuilder {
    tree: ScopeTree,
    // The scope stack is never empty, so its top lives in `current_scope`
    // and reading it doesn't branch. `outer_scopes` holds the enclosing
    // scopes, each with the `declared_names` length to rewind to when it's
    // restored.
    current_scope: ScopeId,
    outer_scopes: Vec<(ScopeId, usize)>,
    /// Each name's stack of live declarations, innermost last.
    ///
    /// Resolution is one lookup. A flat array scanned backwards went
    /// quadratic on files with hundreds of module-level locals, and per-scope
    /// maps cost an allocation per block.
    bindings: FxHashMap<CompactString, Vec<SymbolId>>,
    /// Undo log: names declared since each scope entry, popped on exit.
    declared_names: Vec<CompactString>,
}

impl ScopeTreeBuilder {
    fn push_scope(&mut self, kind: ScopeKind) {
        let id = ScopeId::from_index(self.tree.scopes.len());
        self.tree.scopes.push(Scope { parent: Some(self.current_scope), kind });
        let outer_scope = mem::replace(&mut self.current_scope, id);
        self.outer_scopes.push((outer_scope, self.declared_names.len()));
    }

    fn pop_scope(&mut self) {
        let Some((outer_scope, mark)) = self.outer_scopes.pop() else {
            return;
        };
        self.current_scope = outer_scope;
        // Emptied stacks stay in the map so a re-declared name reuses their
        // capacity.
        for name in self.declared_names.drain(mark..) {
            if let Some(stack) = self.bindings.get_mut(&name) {
                stack.pop();
            }
        }
    }

    fn visit_scoped_block(&mut self, kind: ScopeKind, block: &Block) {
        self.push_scope(kind);
        self.visit_block(block);
        self.pop_scope();
    }

    fn resolve(&self, name: &str) -> Option<SymbolId> {
        self.bindings.get(name).and_then(|stack| stack.last()).copied()
    }

    fn declare_local(&mut self, name: &CompactString, span: Span, kind: SymbolKind) -> SymbolId {
        let shadows = self.resolve(name);
        let id = SymbolId::from_index(self.tree.symbols.len());
        self.tree.symbols.push(Symbol {
            id,
            name: name.clone(),
            scope: self.current_scope,
            kind,
            definition_span: span,
            reference_ids: Vec::new(),
            is_upvalue: false,
            is_modified: false,
            is_exported: false,
            shadows,
        });
        self.bindings.entry(name.clone()).or_default().push(id);
        self.declared_names.push(name.clone());
        id
    }

    fn mark_exported(&mut self, id: SymbolId) {
        if let Some(symbol) = self.tree.symbols.get_mut(id.index()) {
            symbol.is_exported = true;
        }
    }

    /// Records a use of `name`, resolved against the live declarations. A
    /// use from a nested function also marks its symbol as an upvalue, and
    /// a write marks it modified.
    fn reference_name(&mut self, name: &CompactString, span: Span, kind: ReferenceKind) {
        let resolved = self.resolve(name);
        let id = ReferenceId::from_index(self.tree.references.len());
        if let Some(symbol) =
            resolved.and_then(|symbol_id| self.tree.symbols.get_mut(symbol_id.index()))
        {
            if crosses_function_boundary(&self.tree.scopes, self.current_scope, symbol.scope) {
                symbol.is_upvalue = true;
            }
            symbol.reference_ids.push(id);
            if kind.is_write() {
                symbol.is_modified = true;
            }
        }
        self.tree.references.push(Reference {
            span,
            name: name.clone(),
            kind,
            scope: self.current_scope,
            resolved,
        });
    }

    /// Records a name as a reference of `kind`; an indexed or field target
    /// only reads its prefix and key.
    fn reference_var(&mut self, var: &Var, kind: ReferenceKind) {
        match var {
            Var::Name(token) => {
                if let TokenKind::Identifier(name) = &token.kind {
                    self.reference_name(name, token.span, kind);
                }
            }
            Var::Index(_) | Var::FieldAccess(_) => self.walk_var(var),
        }
    }

    fn visit_function_body_with_method(&mut self, body: &FunctionBody, is_method: bool) {
        self.push_scope(ScopeKind::Function);

        if is_method {
            self.declare_local(&CompactString::const_new("self"), body.span, SymbolKind::Parameter);
        }

        for param in body.params.iter() {
            if let TokenKind::Identifier(name) = &param.name.kind {
                self.declare_local(name, param.name.span, SymbolKind::Parameter);
            }
            if let Some(annotation) = &param.type_annotation {
                self.visit_type(annotation);
            }
        }
        if let Some(vararg) = &body.vararg {
            // Lua 5.5: `...name` binds the vararg table to a local.
            if let Some(name_token) = &vararg.name
                && let TokenKind::Identifier(name) = &name_token.kind
            {
                self.declare_local(name, name_token.span, SymbolKind::Parameter);
            }
            if let Some(annotation) = &vararg.type_annotation {
                self.visit_type(annotation);
            }
        }
        if let Some(annotation) = &body.return_type {
            self.visit_type(annotation);
        }

        self.visit_block(&body.block);
        self.pop_scope();
    }

    fn visit_local_assignment(&mut self, local: &LocalAssignment) {
        // Initializers can't see the names being declared.
        if let Some(exprs) = &local.exprs {
            for expr in exprs.iter() {
                self.visit_expression(expr);
            }
        }
        for attributed in local.names.iter() {
            // Annotations can reference runtime bindings via `typeof(expr)`.
            if let Some(annotation) = &attributed.type_annotation {
                self.visit_type(annotation);
            }
            if let TokenKind::Identifier(name) = &attributed.name.kind {
                let id = self.declare_local(name, attributed.name.span, SymbolKind::Local);
                if local.is_exported {
                    self.mark_exported(id); // Luau
                }
            }
        }
    }

    fn visit_function_decl(&mut self, function_decl: &FunctionDecl) {
        // `function f()` writes `f`, but `function t.m()` and
        // `function t:m()` only read `t`. Recording a write would set off
        // unused and overwritten warnings on the usual module pattern
        // (`local M = {} function M.f() end return M`).
        if let Some(first) = function_decl.name.names.first()
            && let TokenKind::Identifier(name) = &first.kind
        {
            let is_field_write =
                function_decl.name.names.len() > 1 || function_decl.name.method.is_some();
            let kind = if is_field_write { ReferenceKind::Read } else { ReferenceKind::Write };
            self.reference_name(name, first.span, kind);
        }
        let is_method = function_decl.name.method.is_some();
        self.visit_function_body_with_method(&function_decl.body, is_method);
    }

    fn visit_if_statement(&mut self, if_stmt: &IfStatement) {
        self.visit_expression(&if_stmt.condition);
        self.visit_scoped_block(ScopeKind::Block, &if_stmt.block);
        for clause in &if_stmt.elseif_clauses {
            self.visit_expression(&clause.condition);
            self.visit_scoped_block(ScopeKind::Block, &clause.block);
        }
        if let Some(else_clause) = &if_stmt.else_clause {
            self.visit_scoped_block(ScopeKind::Block, &else_clause.block);
        }
    }

    fn visit_numeric_for(&mut self, numeric_for: &NumericFor) {
        self.visit_expression(&numeric_for.start);
        self.visit_expression(&numeric_for.limit);
        if let Some(step) = &numeric_for.step {
            self.visit_expression(step);
        }
        self.push_scope(ScopeKind::Loop);
        if let TokenKind::Identifier(name) = &numeric_for.name.kind {
            self.declare_local(name, numeric_for.name.span, SymbolKind::NumericForVariable);
        }
        if let Some(annotation) = &numeric_for.type_annotation {
            self.visit_type(annotation);
        }
        self.visit_block(&numeric_for.block);
        self.pop_scope();
    }

    fn visit_generic_for(&mut self, generic_for: &GenericFor) {
        // The iterator expressions can't see the loop variables.
        for expr in generic_for.exprs.iter() {
            self.visit_expression(expr);
        }
        self.push_scope(ScopeKind::Loop);
        for binding in generic_for.names.iter() {
            if let TokenKind::Identifier(name) = &binding.name.kind {
                self.declare_local(name, binding.name.span, SymbolKind::IteratorVariable);
            }
            if let Some(annotation) = &binding.type_annotation {
                self.visit_type(annotation);
            }
        }
        self.visit_block(&generic_for.block);
        self.pop_scope();
    }
}

impl<'ast> Visitor<'ast> for ScopeTreeBuilder {
    fn visit_function_body(&mut self, body: &'ast FunctionBody) {
        self.visit_function_body_with_method(body, false);
    }

    fn visit_statement(&mut self, stmt: &'ast Statement) {
        match stmt {
            Statement::LocalAssignment(local) => self.visit_local_assignment(local),
            Statement::Assignment(assignment) => {
                for expr in assignment.values.iter() {
                    self.visit_expression(expr);
                }
                for var in assignment.targets.iter() {
                    self.reference_var(var, ReferenceKind::Write);
                }
            }
            Statement::LocalFunction(function) => {
                // Declared before the body so the function can call itself.
                if let TokenKind::Identifier(name) = &function.name.kind {
                    let id = self.declare_local(name, function.name.span, SymbolKind::FunctionName);
                    if function.is_exported {
                        self.mark_exported(id); // Luau
                    }
                }
                self.visit_function_body(&function.body);
            }
            Statement::FunctionDecl(function_decl) => self.visit_function_decl(function_decl),
            Statement::DoBlock(do_block) => {
                self.visit_scoped_block(ScopeKind::Block, &do_block.block);
            }
            Statement::WhileLoop(while_loop) => {
                self.visit_expression(&while_loop.condition);
                self.visit_scoped_block(ScopeKind::Loop, &while_loop.block);
            }
            Statement::RepeatLoop(repeat_loop) => {
                self.push_scope(ScopeKind::Loop);
                self.visit_block(&repeat_loop.block);
                // The `until` condition can see the body's locals.
                self.visit_expression(&repeat_loop.condition);
                self.pop_scope();
            }
            Statement::IfStatement(if_stmt) => self.visit_if_statement(if_stmt),
            Statement::NumericFor(numeric_for) => self.visit_numeric_for(numeric_for),
            Statement::GenericFor(generic_for) => self.visit_generic_for(generic_for),
            // Luau
            Statement::CompoundAssignment(compound) => {
                self.visit_expression(&compound.expr);
                self.reference_var(&compound.var, ReferenceKind::ReadWrite);
            }
            // Lua 5.5 globals declare no locals, but their initializers
            // still reference the enclosing scopes.
            Statement::GlobalDeclaration(global_declaration) => {
                if let Some(exprs) = &global_declaration.exprs {
                    for expr in exprs.iter() {
                        self.visit_expression(expr);
                    }
                }
            }
            // None of these declares a local, so the default walk records
            // their references: call arguments, a `global function` body,
            // a type alias's `typeof(expr)`, and a `type function` body,
            // which is ordinary code.
            Statement::FunctionCall(_)
            | Statement::GlobalFunction(_)
            | Statement::TypeDeclaration(_)
            | Statement::GlobalStar(_)
            | Statement::Goto(_)
            | Statement::Label(_)
            | Statement::EmptyStatement(_)
            | Statement::Break(_)
            | Statement::Error(_) => self.walk_statement(stmt),
        }
    }

    // A method name is a field selector, not a variable, so no walk reaches
    // it here.
    fn visit_var(&mut self, var: &'ast Var) {
        self.reference_var(var, ReferenceKind::Read);
    }
}

fn crosses_function_boundary(scopes: &[Scope], from: ScopeId, to: ScopeId) -> bool {
    let mut current = Some(from);
    while let Some(scope_id) = current {
        if scope_id == to {
            return false;
        }
        let Some(scope) = scopes.get(scope_id.index()) else {
            return false;
        };
        if scope.kind == ScopeKind::Function {
            return true;
        }
        current = scope.parent;
    }
    false
}

#[cfg(test)]
mod tests {
    use luck_token::LuaVersion;

    use super::*;

    fn scope_tree_for(source: &str, version: LuaVersion) -> ScopeTree {
        let parsed = luck_parser::parse(source, version);
        assert!(parsed.errors.is_empty(), "parse errors: {:?}", parsed.errors);
        build(&parsed.block)
    }

    fn scope_tree(source: &str) -> ScopeTree {
        scope_tree_for(source, LuaVersion::Lua54)
    }

    fn symbol<'tree>(tree: &'tree ScopeTree, name: &str) -> &'tree Symbol {
        tree.symbols
            .iter()
            .find(|symbol| symbol.name == name)
            .unwrap_or_else(|| panic!("{name} must be declared"))
    }

    fn references<'tree>(tree: &'tree ScopeTree, name: &str) -> Vec<&'tree Reference> {
        tree.references.iter().filter(|reference| reference.name == name).collect()
    }

    #[test]
    fn local_declaration_creates_symbol() {
        let tree = scope_tree("local x = 1");
        assert_eq!(tree.symbols.len(), 1);
        assert_eq!(tree.symbols[0].name, "x");
    }

    #[test]
    fn reference_resolves_to_local() {
        let tree = scope_tree("local x = 1\nprint(x)");
        let x_references = references(&tree, "x");
        assert_eq!(x_references.len(), 1);
        assert_eq!(x_references[0].resolved, Some(symbol(&tree, "x").id));
    }

    #[test]
    fn global_reference_is_unresolved() {
        let tree = scope_tree("print(hello)");
        let names: Vec<&str> =
            tree.unresolved_references().map(|reference| reference.name.as_str()).collect();
        assert!(names.contains(&"print"));
        assert!(names.contains(&"hello"));
    }

    #[test]
    fn function_body_creates_scope() {
        let tree = scope_tree("local function f(a, b) return a + b end");
        assert_eq!(tree.scopes.len(), 2);
        assert_eq!(tree.scopes[1].kind, ScopeKind::Function);
        assert_eq!(tree.symbols.len(), 3);
        assert_eq!(symbol(&tree, "a").scope, ScopeId::from_index(1));
    }

    #[test]
    fn local_read_from_nested_function_is_upvalue() {
        let tree = scope_tree("local x = 1\nlocal function f() return x end");
        let x_symbol = &tree.symbols[0];
        assert_eq!(x_symbol.name, "x");
        assert!(x_symbol.is_upvalue);
    }

    #[test]
    fn local_read_two_functions_deep_is_upvalue() {
        let tree = scope_tree(
            "local x = 1\nlocal function outer()\n  local function inner() return x end\nend",
        );
        assert!(
            symbol(&tree, "x").is_upvalue,
            "x should be detected as upvalue when captured by nested inner function"
        );
    }

    #[test]
    fn inner_local_shadows_outer_local() {
        let tree = scope_tree("local x = 1\ndo\n  local x = 2\nend");
        assert_eq!(tree.symbols.len(), 2);
        let inner_x = &tree.symbols[1];
        assert_eq!(inner_x.name, "x");
        assert_eq!(inner_x.shadows, Some(tree.symbols[0].id));
    }

    #[test]
    fn numeric_for_declares_loop_variable() {
        let tree = scope_tree("for i = 1, 10 do print(i) end");
        assert_eq!(symbol(&tree, "i").kind, SymbolKind::NumericForVariable);
    }

    #[test]
    fn generic_for_declares_iterator_variables() {
        let tree = scope_tree("for k, v in pairs(t) do print(k, v) end");
        let iterator_variable_count = tree
            .symbols
            .iter()
            .filter(|symbol| symbol.kind == SymbolKind::IteratorVariable)
            .count();
        assert_eq!(iterator_variable_count, 2);
    }

    #[test]
    fn numeric_for_variable_not_visible_after_loop() {
        let tree = scope_tree("for i = 1, 10 do end\nprint(i)");
        assert!(
            references(&tree, "i").iter().any(|reference| reference.resolved.is_none()),
            "for-loop variable i should not be visible outside the loop"
        );
    }

    #[test]
    fn generic_for_variables_not_visible_after_loop() {
        let tree = scope_tree("for k, v in pairs(t) do end\nprint(k)");
        assert!(
            references(&tree, "k").iter().any(|reference| reference.resolved.is_none()),
            "generic for variables should not be visible outside the loop"
        );
    }

    #[test]
    fn named_vararg_declares_symbol() {
        // Lua 5.5: `...name` binds the vararg table to a name.
        let tree = scope_tree_for("local function f(...args) return args end", LuaVersion::Lua55);
        let args_symbol = symbol(&tree, "args");
        assert_eq!(args_symbol.kind, SymbolKind::Parameter);
        let args_references = references(&tree, "args");
        assert_eq!(args_references.len(), 1);
        assert!(
            args_references[0].resolved == Some(args_symbol.id),
            "reference must resolve to the vararg binding"
        );
    }

    #[test]
    fn method_declares_self() {
        let tree = scope_tree("local t = {}\nfunction t:method() return self end");
        assert!(
            tree.symbols.iter().any(|symbol| symbol.name == "self"),
            "self should be declared as a symbol in method scope"
        );
    }

    #[test]
    fn dot_function_declares_no_self() {
        let tree = scope_tree("local t = {}\nfunction t.method() end");
        assert!(
            !tree.symbols.iter().any(|symbol| symbol.name == "self"),
            "self should not be declared in non-method function"
        );
    }

    #[test]
    fn repeat_until_condition_sees_block_locals() {
        let tree = scope_tree("repeat\n  local done = true\nuntil done");
        let done_references = references(&tree, "done");
        assert_eq!(done_references.len(), 1);
        assert!(
            done_references[0].resolved.is_some(),
            "repeat-until condition should resolve locals from the loop body"
        );
    }

    #[test]
    fn global_function_body_captures_enclosing_local() {
        let tree =
            scope_tree_for("local x = 1\nglobal function f() return x end", LuaVersion::Lua55);
        assert!(symbol(&tree, "x").is_upvalue);
    }

    #[test]
    fn type_alias_typeof_reads_enclosing_local() {
        let tree = scope_tree_for("local x = 1\ntype T = typeof(x)", LuaVersion::Luau);
        let x_references = references(&tree, "x");
        assert_eq!(x_references.len(), 1);
        assert_eq!(x_references[0].resolved, Some(symbol(&tree, "x").id));
    }
}
