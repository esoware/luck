use core::mem;

use luck_ast::{
    Block, FunctionBody, FunctionDecl, GenericFor, IfStatement, LocalAssignment, NumericFor,
    Statement, TypeDeclarationValue, Var, Visitor,
};
use luck_token::{CompactString, LuaVersion, Span, TokenKind};
use rustc_hash::{FxHashMap, FxHashSet};

use crate::scope::{
    Reference, ReferenceId, ReferenceKind, Scope, ScopeId, ScopeKind, ScopeTree, Symbol, SymbolId,
    SymbolKind, crosses_function_boundary,
};

/// The `bindings` key of a live `global *`, which no identifier can equal.
const GLOBAL_STAR_KEY: &str = "*";

#[must_use]
pub fn build(block: &Block, version: LuaVersion) -> ScopeTree {
    let mut builder = ScopeTreeBuilder {
        tree: ScopeTree {
            version,
            scopes: vec![Scope { parent: None, kind: ScopeKind::Module }],
            symbols: Vec::new(),
            references: Vec::new(),
            references_by_span: FxHashMap::default(),
            declared_globals: FxHashSet::default(),
            assigned_globals: FxHashSet::default(),
            is_env_assigned: false,
        },
        current_scope: ScopeId::from_index(0),
        outer_scopes: Vec::new(),
        bindings: FxHashMap::default(),
        declared_names: Vec::new(),
        has_free_name_bindings: false,
        is_in_type_function: false,
        type_names: FxHashSet::default(),
    };
    builder.visit_block(block);
    builder.tree.is_env_assigned =
        version.has_env_upvalue() && builder.tree.assigned_globals.contains("_ENV");
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
    ///
    /// `None` is a Lua 5.5 `global` declaration, which voids any enclosing
    /// local of the name for the rest of its block.
    bindings: FxHashMap<CompactString, Vec<Option<SymbolId>>>,
    /// Undo log: names declared since each scope entry, popped on exit.
    declared_names: Vec<CompactString>,
    /// Set by the first local `_ENV` or `global` statement, so ordinary free
    /// names skip the lookups those need.
    has_free_name_bindings: bool,
    is_in_type_function: bool,
    /// Luau: every type declared by a block entered so far.
    type_names: FxHashSet<CompactString>,
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
        self.bindings.get(name).and_then(|stack| stack.last()).copied().flatten()
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
            is_close: false,
            shadows,
        });
        self.bindings.entry(name.clone()).or_default().push(Some(id));
        self.declared_names.push(name.clone());
        self.has_free_name_bindings |= self.tree.version.has_env_upvalue() && name == "_ENV";
        id
    }

    /// Lua 5.5: records a `global` declaration of `name`, or of everything
    /// with [`GLOBAL_STAR_KEY`].
    fn declare_global(&mut self, name: &CompactString) {
        self.bindings.entry(name.clone()).or_default().push(None);
        self.declared_names.push(name.clone());
        self.has_free_name_bindings = true;
    }

    fn mark_exported(&mut self, id: SymbolId) {
        if let Some(symbol) = self.tree.symbols.get_mut(id.index()) {
            symbol.is_exported = true;
        }
    }

    /// Records a use of `name`, resolved against the live declarations. A
    /// use from a nested function also marks the symbol it reads, or the
    /// local `_ENV` it indexes, as an upvalue, and a write marks a symbol
    /// modified.
    fn reference_name(&mut self, name: &CompactString, span: Span, kind: ReferenceKind) {
        let resolved = self.resolve(name);
        // Luau: `types` and the types in scope belong to the type runtime, so
        // inside a type function they're no references of this chunk.
        if resolved.is_none()
            && self.is_in_type_function
            && (name == "types" || self.type_names.contains(name))
        {
            return;
        }
        let (env, is_declared_global) = match resolved {
            None if self.has_free_name_bindings => {
                let is_declared = |k: &str| {
                    matches!(self.bindings.get(k).and_then(|stack| stack.last()), Some(None))
                };
                (self.resolve("_ENV"), is_declared(name) || is_declared(GLOBAL_STAR_KEY))
            }
            _ => (None, false),
        };
        if kind.is_write() && resolved.is_none() && env.is_none() {
            self.tree.assigned_globals.insert(name.clone());
        }
        let id = ReferenceId::from_index(self.tree.references.len());
        self.tree.references_by_span.entry(span).or_insert(id);
        if let Some(symbol) =
            resolved.or(env).and_then(|symbol_id| self.tree.symbols.get_mut(symbol_id.index()))
        {
            if crosses_function_boundary(&self.tree.scopes, self.current_scope, symbol.scope) {
                symbol.is_upvalue = true;
            }
            if resolved.is_some() {
                symbol.reference_ids.push(id);
                if kind.is_write() {
                    symbol.is_modified = true;
                }
            }
        }
        self.tree.references.push(Reference {
            span,
            name: name.clone(),
            kind,
            scope: self.current_scope,
            resolved,
            env,
            is_declared_global,
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

    /// Luau: the annotations of a function's signature. The parameters, and
    /// a `local function`'s own name, bind after the whole signature parses,
    /// so a `typeof` here reads the enclosing scope.
    fn visit_signature_annotations(&mut self, body: &FunctionBody) {
        for param in body.params.iter() {
            if let Some(annotation) = &param.type_annotation {
                self.visit_type(annotation);
            }
        }
        if let Some(annotation) =
            body.vararg.as_ref().and_then(|vararg| vararg.type_annotation.as_ref())
        {
            self.visit_type(annotation);
        }
        if let Some(annotation) = &body.return_type {
            self.visit_type(annotation);
        }
    }

    /// Binds `body`'s parameters in a new function scope and visits its
    /// block.
    fn visit_function_scope(&mut self, body: &FunctionBody, is_method: bool) {
        self.push_scope(ScopeKind::Function);
        if is_method {
            let span = Span::new(body.span.start, body.span.start);
            self.declare_local(&CompactString::const_new("self"), span, SymbolKind::ImplicitSelf);
        }
        for param in body.params.iter() {
            if let TokenKind::Identifier(name) = &param.name.kind {
                self.declare_local(name, param.name.span, SymbolKind::Parameter);
            }
        }
        // Lua 5.5: `...name` binds the vararg table to a local.
        if let Some(name_token) = body.vararg.as_ref().and_then(|vararg| vararg.name.as_ref())
            && let TokenKind::Identifier(name) = &name_token.kind
        {
            self.declare_local(name, name_token.span, SymbolKind::Parameter);
        }
        self.visit_block(&body.block);
        self.pop_scope();
    }

    fn visit_local_assignment(&mut self, local: &LocalAssignment) {
        // Annotations and initializers can't see the names being declared.
        for attributed in local.names.iter() {
            if let Some(annotation) = &attributed.type_annotation {
                self.visit_type(annotation);
            }
        }
        if let Some(exprs) = &local.exprs {
            for expr in exprs.iter() {
                self.visit_expr(expr);
            }
        }
        for attributed in local.names.iter() {
            if let TokenKind::Identifier(name) = &attributed.name.kind {
                let id = self.declare_local(name, attributed.name.span, SymbolKind::Local);
                if local.is_exported {
                    self.mark_exported(id); // Luau
                }
                if attributed
                    .attribute
                    .as_ref()
                    .is_some_and(|attribute| attribute.name.identifier() == Some("close"))
                    && let Some(symbol) = self.tree.symbols.get_mut(id.index())
                {
                    symbol.is_close = true;
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
        self.visit_signature_annotations(&function_decl.body);
        self.visit_function_scope(&function_decl.body, function_decl.name.method.is_some());
    }

    fn visit_if_stmt(&mut self, if_stmt: &IfStatement) {
        self.visit_expr(&if_stmt.condition);
        self.visit_scoped_block(ScopeKind::Block, &if_stmt.block);
        for clause in &if_stmt.elseif_clauses {
            self.visit_expr(&clause.condition);
            self.visit_scoped_block(ScopeKind::Block, &clause.block);
        }
        if let Some(else_clause) = &if_stmt.else_clause {
            self.visit_scoped_block(ScopeKind::Block, &else_clause.block);
        }
    }

    fn visit_numeric_for(&mut self, numeric_for: &NumericFor) {
        if let Some(annotation) = &numeric_for.type_annotation {
            self.visit_type(annotation);
        }
        self.visit_expr(&numeric_for.start);
        self.visit_expr(&numeric_for.limit);
        if let Some(step) = &numeric_for.step {
            self.visit_expr(step);
        }
        self.push_scope(ScopeKind::Loop);
        if let TokenKind::Identifier(name) = &numeric_for.name.kind {
            self.declare_local(name, numeric_for.name.span, SymbolKind::NumericForVariable);
        }
        self.visit_block(&numeric_for.block);
        self.pop_scope();
    }

    fn visit_generic_for(&mut self, generic_for: &GenericFor) {
        // Annotations and iterator expressions can't see the loop variables.
        for binding in generic_for.names.iter() {
            if let Some(annotation) = &binding.type_annotation {
                self.visit_type(annotation);
            }
        }
        for expr in generic_for.exprs.iter() {
            self.visit_expr(expr);
        }
        self.push_scope(ScopeKind::Loop);
        for binding in generic_for.names.iter() {
            if let TokenKind::Identifier(name) = &binding.name.kind {
                self.declare_local(name, binding.name.span, SymbolKind::IteratorVariable);
            }
        }
        self.visit_block(&generic_for.block);
        self.pop_scope();
    }
}

impl<'ast> Visitor<'ast> for ScopeTreeBuilder {
    fn visit_block(&mut self, block: &'ast Block) {
        // Luau: a type function sees every type of its block, earlier or later.
        if self.tree.version.has_type_syntax() {
            for stmt in &block.stmts {
                if let Statement::TypeDeclaration(declaration) = stmt
                    && let TokenKind::Identifier(name) = &declaration.name.kind
                {
                    self.type_names.insert(name.clone());
                }
            }
        }
        self.walk_block(block);
    }

    fn visit_function_body(&mut self, body: &'ast FunctionBody) {
        self.visit_signature_annotations(body);
        self.visit_function_scope(body, false);
    }

    fn visit_stmt(&mut self, stmt: &'ast Statement) {
        match stmt {
            Statement::LocalAssignment(local) => self.visit_local_assignment(local),
            Statement::Assignment(assignment) => {
                for expr in assignment.values.iter() {
                    self.visit_expr(expr);
                }
                for var in assignment.targets.iter() {
                    self.reference_var(var, ReferenceKind::Write);
                }
            }
            Statement::LocalFunction(function) => {
                self.visit_signature_annotations(&function.body);
                // Declared before the body so the function can call itself.
                if let TokenKind::Identifier(name) = &function.name.kind {
                    let id = self.declare_local(name, function.name.span, SymbolKind::FunctionName);
                    if function.is_exported {
                        self.mark_exported(id); // Luau
                    }
                }
                self.visit_function_scope(&function.body, false);
            }
            Statement::FunctionDecl(function_decl) => self.visit_function_decl(function_decl),
            Statement::DoBlock(do_block) => {
                self.visit_scoped_block(ScopeKind::Block, &do_block.block);
            }
            Statement::WhileLoop(while_loop) => {
                self.visit_expr(&while_loop.condition);
                self.visit_scoped_block(ScopeKind::Loop, &while_loop.block);
            }
            Statement::RepeatLoop(repeat_loop) => {
                self.push_scope(ScopeKind::Loop);
                self.visit_block(&repeat_loop.block);
                // The `until` condition can see the body's locals.
                self.visit_expr(&repeat_loop.condition);
                self.pop_scope();
            }
            Statement::IfStatement(if_stmt) => self.visit_if_stmt(if_stmt),
            Statement::NumericFor(numeric_for) => self.visit_numeric_for(numeric_for),
            Statement::GenericFor(generic_for) => self.visit_generic_for(generic_for),
            // Luau
            Statement::CompoundAssignment(compound) => {
                self.visit_expr(&compound.expr);
                self.reference_var(&compound.var, ReferenceKind::ReadWrite);
            }
            // Lua 5.5: the initializers can't see the names being declared,
            // and a name with an initializer is assigned.
            Statement::GlobalDeclaration(global_declaration) => {
                if let Some(exprs) = &global_declaration.exprs {
                    for expr in exprs.iter() {
                        self.visit_expr(expr);
                    }
                }
                for attributed in global_declaration.names.iter() {
                    if let TokenKind::Identifier(name) = &attributed.name.kind {
                        self.tree.declared_globals.insert(name.clone());
                        if global_declaration.exprs.is_some() {
                            self.tree.assigned_globals.insert(name.clone());
                        }
                        self.declare_global(name);
                    }
                }
            }
            // Lua 5.5: declared before the body, so the body reads the global.
            Statement::GlobalFunction(global_function) => {
                if let TokenKind::Identifier(name) = &global_function.name.kind {
                    self.tree.declared_globals.insert(name.clone());
                    self.tree.assigned_globals.insert(name.clone());
                    self.declare_global(name);
                }
                self.visit_function_body(&global_function.body);
            }
            // Lua 5.5
            Statement::GlobalStar(_) => {
                self.declare_global(&CompactString::const_new(GLOBAL_STAR_KEY));
            }
            // Luau: the body of a type function, unlike a type alias, runs in
            // the type runtime.
            Statement::TypeDeclaration(declaration) => {
                let is_enclosed_by_type_function = self.is_in_type_function;
                self.is_in_type_function |=
                    matches!(declaration.type_value, TypeDeclarationValue::TypeFunction(_));
                self.walk_stmt(stmt);
                self.is_in_type_function = is_enclosed_by_type_function;
            }
            // None of these declares a local, so the default walk records
            // their references: call arguments and a type alias's
            // `typeof(expr)`.
            Statement::FunctionCall(_)
            | Statement::Goto(_)
            | Statement::Label(_)
            | Statement::EmptyStatement(_)
            | Statement::Break(_)
            | Statement::Error(_) => self.walk_stmt(stmt),
        }
    }

    // A method name is a field selector, not a variable, so no walk reaches
    // it here.
    fn visit_var(&mut self, var: &'ast Var) {
        self.reference_var(var, ReferenceKind::Read);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scope_tree_for(source: &str, version: LuaVersion) -> ScopeTree {
        let parsed = luck_parser::parse(source, version);
        assert!(parsed.errors.is_empty(), "parse errors: {:?}", parsed.errors);
        build(&parsed.block, version)
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
    fn global_stmts_record_their_declared_names() {
        let tree = scope_tree_for(
            "global a, b <const> = 1, 2\nglobal c\nglobal function d() end\n",
            LuaVersion::Lua55,
        );
        for name in ["a", "b", "c", "d"] {
            assert!(tree.declared_globals.contains(name), "{name}");
        }
        assert_eq!(tree.declared_globals.len(), 4);
    }

    #[test]
    fn local_env_makes_free_names_its_fields() {
        let tree = scope_tree("local _ENV = {}\nx = 1\nprint(y)");
        let env_id = symbol(&tree, "_ENV").id;
        assert_eq!(references(&tree, "y")[0].env, Some(env_id));
        assert_eq!(tree.unresolved_references().count(), 0);
        assert!(tree.assigned_globals.is_empty(), "`x = 1` writes the local table");
    }

    #[test]
    fn local_env_is_an_ordinary_local_before_52() {
        for version in [LuaVersion::Lua51, LuaVersion::Luau] {
            let tree = scope_tree_for("local _ENV = {}\nprint(y)", version);
            assert_eq!(references(&tree, "y")[0].env, None, "{version:?}");
            assert_eq!(tree.unresolved_references().count(), 2, "{version:?}");
            assert!(!tree.is_env_binding(symbol(&tree, "_ENV").id), "{version:?}");
        }
    }

    #[test]
    fn assigning_the_chunk_env_redirects_every_free_name() {
        let tree = scope_tree("_ENV = {}\nprint(y)");
        assert!(tree.is_env_assigned);
        assert_eq!(tree.unresolved_references().count(), 0);

        let lua51 = scope_tree_for("_ENV = {}\nprint(y)", LuaVersion::Lua51);
        assert!(!lua51.is_env_assigned);
        assert_eq!(lua51.unresolved_references().count(), 3);
    }

    #[test]
    fn assigned_globals_records_free_name_writes_only() {
        let tree = scope_tree("x = 1\nfunction f() end\nfunction t.m() end\nlocal y\ny = 2");
        assert!(tree.assigned_globals.contains("x"));
        assert!(tree.assigned_globals.contains("f"));
        assert!(!tree.assigned_globals.contains("t"), "`t.m` only reads `t`");
        assert!(!tree.assigned_globals.contains("y"), "`y` is a local");
    }

    #[test]
    fn global_declaration_ends_with_its_block() {
        let tree = scope_tree_for("local x = 1\ndo global x end\nprint(x)", LuaVersion::Lua55);
        let last_reference = *references(&tree, "x").last().expect("print(x) references x");
        assert_eq!(last_reference.resolved, Some(symbol(&tree, "x").id));
    }

    #[test]
    fn global_declaration_initializer_reads_the_enclosing_local() {
        let tree = scope_tree_for("local x = 1\nglobal x = x\nprint(x)", LuaVersion::Lua55);
        let x_references = references(&tree, "x");
        assert_eq!(x_references[0].resolved, Some(symbol(&tree, "x").id));
        assert_eq!(x_references[1].resolved, None);
        assert!(tree.assigned_globals.contains("x"));
    }

    #[test]
    fn declared_global_without_initializer_is_not_assigned() {
        let tree = scope_tree_for("global x\nglobal *\nprint(y)", LuaVersion::Lua55);
        assert!(tree.assigned_globals.is_empty());
    }

    #[test]
    fn global_star_covers_free_names_until_its_block_ends() {
        let tree = scope_tree_for("do global *\na = 1 end\nb = 2", LuaVersion::Lua55);
        assert!(references(&tree, "a")[0].is_declared_global);
        assert!(!references(&tree, "b")[0].is_declared_global);
    }

    #[test]
    fn global_star_does_not_void_enclosing_locals() {
        let tree = scope_tree_for("local x = 1\nglobal *\nprint(x)", LuaVersion::Lua55);
        assert_eq!(references(&tree, "x")[0].resolved, Some(symbol(&tree, "x").id));
    }

    #[test]
    fn explicit_self_stays_a_parameter() {
        let tree = scope_tree("local t = {}\nfunction t.m(self) return self end");
        assert_eq!(symbol(&tree, "self").kind, SymbolKind::Parameter);
    }

    #[test]
    fn method_self_is_implicit_with_an_empty_span() {
        let tree = scope_tree("local t = {}\nfunction t:m() return self end");
        let self_symbol = symbol(&tree, "self");
        assert_eq!(self_symbol.kind, SymbolKind::ImplicitSelf);
        assert_eq!(self_symbol.definition_span.start, self_symbol.definition_span.end);
    }

    #[test]
    fn type_function_body_sees_a_later_type() {
        let tree = scope_tree_for(
            "type function f(t) return g(t) end\ntype function g(t) return t end",
            LuaVersion::Luau,
        );
        assert_eq!(references(&tree, "g").len(), 0);
    }

    #[test]
    fn type_runtime_names_are_only_known_in_type_functions() {
        let tree = scope_tree_for("type T = number\nprint(types, T)", LuaVersion::Luau);
        assert_eq!(tree.unresolved_references().count(), 3);
    }

    #[test]
    fn references_by_span_finds_each_reference() {
        let tree = scope_tree("local x = 1\nprint(x)");
        for reference in &tree.references {
            let id = tree.references_by_span[&reference.span];
            assert_eq!(tree.reference(id).name, reference.name);
        }
    }

    #[test]
    fn resolves_to_local_checks_the_name_at_the_span() {
        let tree = scope_tree("local x = 1\nprint(x)\nprint(y)");
        let local_x = references(&tree, "x")[0].span;
        let global_y = references(&tree, "y")[0].span;
        assert!(tree.resolves_to_local("x", local_x));
        assert!(!tree.resolves_to_local("y", local_x));
        assert!(!tree.resolves_to_local("y", global_y));
    }

    #[test]
    fn type_alias_typeof_reads_enclosing_local() {
        let tree = scope_tree_for("local x = 1\ntype T = typeof(x)", LuaVersion::Luau);
        let x_references = references(&tree, "x");
        assert_eq!(x_references.len(), 1);
        assert_eq!(x_references[0].resolved, Some(symbol(&tree, "x").id));
    }

    #[test]
    fn implicit_self_span_does_not_cover_the_method_body() {
        let tree = scope_tree("local t = {}\nfunction t:m()\n  local x = 1\n  return x\nend");
        let self_span = symbol(&tree, "self").definition_span;
        let x_span = symbol(&tree, "x").definition_span;
        assert!(
            self_span.end <= x_span.start || x_span.end <= self_span.start,
            "implicit self must not claim the span of the declarations in its method body, \
             or a cursor on any of them resolves to self: self {self_span:?}, x {x_span:?}"
        );
    }

    #[test]
    fn global_declaration_shadows_enclosing_local() {
        let tree = scope_tree_for("local x = 1\nglobal print, x\nprint(x)", LuaVersion::Lua55);
        assert_eq!(
            references(&tree, "x")[0].resolved,
            None,
            "after `global x`, the name x refers to the global and voids the enclosing local"
        );
    }

    #[test]
    fn global_function_shadows_enclosing_local() {
        let tree = scope_tree_for("local f = 1\nglobal function f() end\nf()", LuaVersion::Lua55);
        let last_reference = *references(&tree, "f").last().expect("f() references f");
        assert_eq!(
            last_reference.resolved, None,
            "after `global function f`, the name f refers to the global and voids the enclosing local"
        );
    }

    #[test]
    fn global_function_body_refers_to_the_global() {
        let tree =
            scope_tree_for("local f = 1\nglobal function f() return f end", LuaVersion::Lua55);
        assert!(
            !symbol(&tree, "f").is_upvalue,
            "the body of `global function f` sees the global f, so it captures no local f"
        );
    }

    #[test]
    fn param_annotation_typeof_does_not_see_parameters() {
        let tree = scope_tree_for(
            "local a = 1\nlocal function f(a: number, b: typeof(a)) return b end",
            LuaVersion::Luau,
        );
        assert_eq!(
            references(&tree, "a")[0].resolved,
            Some(symbol(&tree, "a").id),
            "Luau parses parameter annotations before binding any parameter, so typeof(a) \
             names the outer local"
        );
    }

    #[test]
    fn return_annotation_typeof_does_not_see_parameters() {
        let tree = scope_tree_for(
            "local a = 1\nlocal function f(a: number): typeof(a) return a end",
            LuaVersion::Luau,
        );
        assert_eq!(
            references(&tree, "a")[0].resolved,
            Some(symbol(&tree, "a").id),
            "Luau parses the return annotation before binding the parameters, so typeof(a) \
             names the outer local"
        );
    }

    #[test]
    fn local_annotation_typeof_does_not_see_sibling_name() {
        let tree = scope_tree_for(
            "local a = 1\ndo\n  local a, b: typeof(a) = 2, 3\n  print(a, b)\nend",
            LuaVersion::Luau,
        );
        assert_eq!(
            references(&tree, "a")[0].resolved,
            Some(symbol(&tree, "a").id),
            "the names of a local statement bind after the statement, so typeof(a) in its \
             annotation names the outer local"
        );
    }

    #[test]
    fn numeric_for_annotation_typeof_does_not_see_loop_variable() {
        let tree = scope_tree_for(
            "local i = 1\nfor i: typeof(i) = 1, 2 do print(i) end",
            LuaVersion::Luau,
        );
        assert_eq!(
            references(&tree, "i")[0].resolved,
            Some(symbol(&tree, "i").id),
            "the loop variable binds after the loop header, so typeof(i) in its annotation \
             names the outer local"
        );
    }

    #[test]
    fn generic_for_annotation_typeof_does_not_see_loop_variables() {
        let tree = scope_tree_for(
            "local k = 1\nfor k, v: typeof(k) in pairs({}) do print(k, v) end",
            LuaVersion::Luau,
        );
        assert_eq!(
            references(&tree, "k")[0].resolved,
            Some(symbol(&tree, "k").id),
            "the loop variables bind after the loop header, so typeof(k) in an annotation \
             names the outer local"
        );
    }

    #[test]
    fn local_function_annotation_typeof_does_not_see_its_name() {
        let tree = scope_tree_for(
            "local g = 1\nlocal function g(z: typeof(g)) return z end",
            LuaVersion::Luau,
        );
        assert_eq!(
            references(&tree, "g")[0].resolved,
            Some(symbol(&tree, "g").id),
            "Luau binds a local function's name after parsing its signature, so typeof(g) \
             in a parameter annotation names the outer local"
        );
    }
}
