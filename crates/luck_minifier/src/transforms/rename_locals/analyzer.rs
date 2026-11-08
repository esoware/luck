//! Scope resolution for the renamer: which binding each name reads, where
//! each binding is live, and which short name each slot of bindings wears.

use core::mem;

use luck_ast::{
    Block, Expression, Field, FunctionArgs, FunctionBody, FunctionCall, IfStatement, LastStatement,
    Punctuated, Statement, TypeArgs, Var, Visitor,
};
use luck_token::{CompactString, LuaVersion};
use rustc_hash::{FxHashMap, FxHashSet};

use super::candidate_pool::CandidatePool;
use super::scope_set::ScopeSet;
use crate::binding::{identifier_name, is_env_binding};

struct ScopeNode {
    parent: Option<usize>,
    binding_ids: Vec<usize>,
    // After `propagate_globals`, also holds every descendant scope's globals.
    referenced_globals: FxHashSet<CompactString>,
}

struct BindingInfo {
    original_name: CompactString,
    scope_id: usize,
    reference_count: usize,
    // `None` until `assign_slots` runs, and always `None` for a fixed binding.
    slot: Option<usize>,
    is_fixed: bool,
    // Lua 5.5: a `global` declaration. It scopes and shadows like a local, but
    // its references still reach the real global.
    is_global: bool,
    // The declaring scope, the reference scopes, and every scope on the path
    // between them.
    live_scopes: ScopeSet,
}

/// Hands the expressions a type embeds back to the analyzer.
///
/// `Visitor::walk_type` reaches them in the same order as
/// `Transform::walk_type`, and the renamer's binding ids follow that order.
struct TypeExpressions<'analyzer>(&'analyzer mut Analyzer);

impl<'ast> Visitor<'ast> for TypeExpressions<'_> {
    fn visit_expression(&mut self, expr: &'ast Expression) {
        self.0.analyze_expr(expr);
    }

    fn visit_function_body(&mut self, body: &'ast FunctionBody) {
        self.0.analyze_function_body(body);
    }
}

/// Resolves every name in one walk, in the order the renamer later visits
/// the same bindings.
pub(super) struct Analyzer {
    scopes: Vec<ScopeNode>,
    bindings: Vec<BindingInfo>,
    // The top of the scope stack lives in `current_scope_id`, so reading it
    // doesn't branch; `outer_scope_ids` holds only the enclosing scopes.
    current_scope_id: usize,
    outer_scope_ids: Vec<usize>,
    // Original name to its stack of binding ids, innermost binding on top.
    name_stacks: FxHashMap<CompactString, Vec<usize>>,
    version: LuaVersion,
}

impl Analyzer {
    pub(super) fn new(version: LuaVersion) -> Self {
        let root_scope = ScopeNode {
            parent: None,
            binding_ids: Vec::new(),
            referenced_globals: FxHashSet::default(),
        };
        Self {
            scopes: vec![root_scope],
            bindings: Vec::new(),
            current_scope_id: 0,
            outer_scope_ids: Vec::new(),
            name_stacks: FxHashMap::default(),
            version,
        }
    }

    /// Declares the globals being renamed as root bindings, ahead of the
    /// walk, so every reference to one resolves to it.
    pub(super) fn declare_root_globals(&mut self, names: &[&str]) {
        for name in names {
            self.declare_binding(name, false);
        }
    }

    /// The name each binding wears after renaming, by binding id.
    pub(super) fn binding_names(mut self) -> Vec<CompactString> {
        self.propagate_globals();
        let live_scopes_by_slot = self.assign_slots();
        let slot_names = self.generate_names(&live_scopes_by_slot);
        self.bindings
            .into_iter()
            .map(|binding| {
                binding
                    .slot
                    .and_then(|slot| slot_names.get(slot).cloned())
                    .unwrap_or(binding.original_name)
            })
            .collect()
    }

    const fn current_scope(&self) -> usize {
        self.current_scope_id
    }

    fn enter_scope(&mut self) {
        let parent = self.current_scope();
        let new_id = self.scopes.len();
        self.scopes.push(ScopeNode {
            parent: Some(parent),
            binding_ids: Vec::new(),
            referenced_globals: FxHashSet::default(),
        });
        self.outer_scope_ids.push(mem::replace(&mut self.current_scope_id, new_id));
    }

    fn exit_scope(&mut self) {
        let Some(outer_scope_id) = self.outer_scope_ids.pop() else {
            return;
        };
        let scope_id = mem::replace(&mut self.current_scope_id, outer_scope_id);
        let Self { scopes, bindings, name_stacks, .. } = self;
        let Some(scope) = scopes.get(scope_id) else {
            return;
        };
        let exiting_bindings =
            scope.binding_ids.iter().rev().filter_map(|&binding_id| bindings.get(binding_id));
        for binding in exiting_bindings {
            if let Some(stack) = name_stacks.get_mut(&binding.original_name) {
                stack.pop();
                if stack.is_empty() {
                    name_stacks.remove(&binding.original_name);
                }
            }
        }
    }

    fn declare_binding(&mut self, name: &str, is_fixed: bool) -> usize {
        let scope_id = self.current_scope();
        let binding_id = self.bindings.len();
        let mut live_scopes = ScopeSet::default();
        live_scopes.insert(scope_id);
        let original_name = CompactString::from(name);
        self.bindings.push(BindingInfo {
            original_name: original_name.clone(),
            scope_id,
            reference_count: 0,
            slot: None,
            is_fixed,
            is_global: false,
            live_scopes,
        });
        if let Some(scope) = self.scopes.get_mut(scope_id) {
            scope.binding_ids.push(binding_id);
        }
        self.name_stacks.entry(original_name).or_default().push(binding_id);
        binding_id
    }

    /// Lua 5.5: declares `name` as a global from here to the end of the scope.
    ///
    /// Lookup finds the newest declaration, so a local renamed to this name and
    /// used after it would resolve to the global instead. Declaring therefore
    /// counts as a global read, even if nothing reads the name.
    fn declare_global(&mut self, name: &str) {
        let binding_id = self.declare_binding(name, true);
        if let Some(binding) = self.bindings.get_mut(binding_id) {
            binding.is_global = true;
        }
        self.reference_name(name);
    }

    fn reference_name(&mut self, name: &str) {
        let current_scope = self.current_scope();
        let Self { scopes, bindings, name_stacks, .. } = self;
        let innermost_binding = name_stacks
            .get(name)
            .and_then(|stack| stack.last())
            .and_then(|&binding_id| bindings.get_mut(binding_id));
        if let Some(binding) = innermost_binding {
            binding.reference_count = binding.reference_count.saturating_add(1);
            let mut next_scope = Some(current_scope);
            while let Some(scope_id) = next_scope {
                binding.live_scopes.insert(scope_id);
                if scope_id == binding.scope_id {
                    break;
                }
                next_scope = scopes.get(scope_id).and_then(|scope| scope.parent);
            }
            if !binding.is_global {
                return;
            }
        }
        // An unresolved name or a `global` binding reads the real global. The
        // globals being renamed are pre-declared as root bindings, so their
        // references resolve above and never get here.
        if let Some(scope) = scopes.get_mut(current_scope) {
            scope.referenced_globals.insert(name.into());
        }
    }

    pub(super) fn analyze_block(&mut self, block: &Block) {
        for stmt in &block.stmts {
            self.analyze_stmt(stmt);
        }
        if let Some(last) = &block.last_stmt {
            self.analyze_last_stmt(last);
        }
    }

    fn analyze_scoped_block(&mut self, block: &Block) {
        self.enter_scope();
        self.analyze_block(block);
        self.exit_scope();
    }

    fn analyze_if(&mut self, if_stmt: &IfStatement) {
        self.analyze_expr(&if_stmt.condition);
        self.analyze_scoped_block(&if_stmt.block);
        for clause in &if_stmt.elseif_clauses {
            self.analyze_expr(&clause.condition);
            self.analyze_scoped_block(&clause.block);
        }
        if let Some(else_clause) = &if_stmt.else_clause {
            self.analyze_scoped_block(&else_clause.block);
        }
    }

    fn analyze_stmt(&mut self, stmt: &Statement) {
        match stmt {
            Statement::LocalAssignment(local) => {
                // RHS before LHS: `local x = x` reads the outer x.
                if let Some(exprs) = &local.exprs {
                    for expr in exprs.iter() {
                        self.analyze_expr(expr);
                    }
                }
                for attributed in local.names.iter() {
                    let name = identifier_name(&attributed.name);
                    let is_fixed =
                        name == "self" || is_env_binding(name, self.version) || local.is_exported;
                    self.declare_binding(name, is_fixed);
                }
            }
            Statement::LocalFunction(local_function) => {
                let name = identifier_name(&local_function.name);
                let is_fixed = name == "self"
                    || is_env_binding(name, self.version)
                    || local_function.is_exported;
                self.declare_binding(name, is_fixed);
                self.analyze_function_body(&local_function.body);
            }
            Statement::FunctionDecl(function_declaration) => {
                self.analyze_function_body(&function_declaration.body);
                if let Some(root_name) = function_declaration.name.names.first() {
                    self.reference_name(identifier_name(root_name));
                }
            }
            Statement::Assignment(assignment) => {
                for expr in assignment.values.iter() {
                    self.analyze_expr(expr);
                }
                for var in assignment.targets.iter() {
                    self.analyze_var(var);
                }
            }
            Statement::FunctionCall(call_stmt) => self.analyze_function_call(call_stmt),
            Statement::DoBlock(do_block) => self.analyze_scoped_block(&do_block.block),
            Statement::WhileLoop(while_loop) => {
                self.analyze_expr(&while_loop.condition);
                self.analyze_scoped_block(&while_loop.block);
            }
            Statement::RepeatLoop(repeat_loop) => {
                self.enter_scope();
                self.analyze_block(&repeat_loop.block);
                self.analyze_expr(&repeat_loop.condition);
                self.exit_scope();
            }
            Statement::IfStatement(if_stmt) => self.analyze_if(if_stmt),
            Statement::NumericFor(numeric_for) => {
                self.analyze_expr(&numeric_for.start);
                self.analyze_expr(&numeric_for.limit);
                if let Some(step) = &numeric_for.step {
                    self.analyze_expr(step);
                }
                self.enter_scope();
                let name = identifier_name(&numeric_for.name);
                self.declare_binding(name, is_env_binding(name, self.version));
                self.analyze_block(&numeric_for.block);
                self.exit_scope();
            }
            Statement::GenericFor(generic_for) => {
                for expr in generic_for.exprs.iter() {
                    self.analyze_expr(expr);
                }
                self.enter_scope();
                for param in generic_for.names.iter() {
                    let name = identifier_name(&param.name);
                    self.declare_binding(name, is_env_binding(name, self.version));
                }
                self.analyze_block(&generic_for.block);
                self.exit_scope();
            }
            // Luau
            Statement::CompoundAssignment(compound_assignment) => {
                self.analyze_var(&compound_assignment.var);
                self.analyze_expr(&compound_assignment.expr);
            }
            // Lua 5.5: as with `local function`, the body sees the name.
            Statement::GlobalFunction(global_function) => {
                self.declare_global(identifier_name(&global_function.name));
                self.analyze_function_body(&global_function.body);
            }
            // Lua 5.5: as with `local`, the initializers see the outer scope.
            Statement::GlobalDeclaration(global_declaration) => {
                if let Some(exprs) = &global_declaration.exprs {
                    for expr in exprs.iter() {
                        self.analyze_expr(expr);
                    }
                }
                for attributed in global_declaration.names.iter() {
                    self.declare_global(identifier_name(&attributed.name));
                }
            }
            // Luau: `type function` bodies and `typeof` are ordinary code that
            // `Renamer` walks too. If the analyzer skipped them, the binding
            // ids would fall out of step and the renamer would panic.
            Statement::TypeDeclaration(_) => TypeExpressions(self).walk_statement(stmt),
            // Leaves with no bindings or references the renamer touches.
            Statement::EmptyStatement(_)
            | Statement::Goto(_)
            | Statement::Label(_)
            | Statement::GlobalStar(_)
            | Statement::Break(_)
            | Statement::Error(_) => {}
        }
    }

    fn analyze_last_stmt(&mut self, last: &LastStatement) {
        if let LastStatement::Return(return_stmt) = last {
            for expr in return_stmt.exprs.iter() {
                self.analyze_expr(expr);
            }
        }
    }

    fn analyze_expr(&mut self, expr: &Expression) {
        match expr {
            Expression::Var(var) => self.analyze_var(var),
            Expression::FunctionCall(call) => self.analyze_function_call(call),
            Expression::BinaryOp(binop) => {
                self.analyze_expr(&binop.left);
                self.analyze_expr(&binop.right);
            }
            Expression::UnaryOp(unop) => self.analyze_expr(&unop.operand),
            Expression::Parenthesized(paren) => self.analyze_expr(&paren.expr),
            Expression::FunctionDef(function) => self.analyze_function_body(&function.body),
            Expression::TableConstructor(table) => self.analyze_fields(&table.fields),
            // Luau
            Expression::InterpolatedString(interpolated) => {
                for segment in &interpolated.segments {
                    if let Some(segment_expr) = &segment.expr {
                        self.analyze_expr(segment_expr);
                    }
                }
            }
            // Luau
            Expression::IfExpression(if_expr) => {
                self.analyze_expr(&if_expr.condition);
                self.analyze_expr(&if_expr.then_expr);
                for clause in &if_expr.elseif_clauses {
                    self.analyze_expr(&clause.condition);
                    self.analyze_expr(&clause.expr);
                }
                self.analyze_expr(&if_expr.else_expr);
            }
            // Luau
            Expression::TypeCast(cast) => {
                self.analyze_expr(&cast.expr);
                TypeExpressions(self).visit_type(&cast.type_annotation);
            }
            // Luau
            Expression::TypeInstantiation(instantiation) => {
                self.analyze_expr(&instantiation.expr);
                self.analyze_type_args(&instantiation.type_args);
            }
            // Leaves with no sub-expressions or name references.
            Expression::Nil(_)
            | Expression::False(_)
            | Expression::True(_)
            | Expression::Number(_)
            | Expression::Integer(_)
            | Expression::StringLiteral(_)
            | Expression::VarArg(_)
            | Expression::Error(_) => {}
        }
    }

    fn analyze_var(&mut self, var: &Var) {
        match var {
            Var::Name(name) => self.reference_name(identifier_name(name)),
            Var::FieldAccess(field_access) => self.analyze_expr(&field_access.prefix),
            Var::Index(index_expr) => {
                self.analyze_expr(&index_expr.prefix);
                self.analyze_expr(&index_expr.index);
            }
        }
    }

    fn analyze_type_args(&mut self, type_args: &TypeArgs) {
        for type_arg in type_args.args.iter() {
            TypeExpressions(self).visit_type(type_arg);
        }
    }

    fn analyze_function_call(&mut self, call: &FunctionCall) {
        self.analyze_expr(&call.callee);
        if let Some(type_args) = &call.explicit_type_args {
            self.analyze_type_args(type_args);
        }
        match &call.args {
            FunctionArgs::Parenthesized { args, .. } => {
                for arg in args.iter() {
                    self.analyze_expr(arg);
                }
            }
            FunctionArgs::TableConstructor(table) => self.analyze_fields(&table.fields),
            FunctionArgs::StringLiteral(_) => {}
        }
    }

    fn analyze_fields(&mut self, fields: &Punctuated<Field>) {
        for field in fields.iter() {
            match field {
                Field::Bracketed { key, value, .. } => {
                    self.analyze_expr(key);
                    self.analyze_expr(value);
                }
                Field::Named { value, .. } | Field::Positional { value, .. } => {
                    self.analyze_expr(value);
                }
            }
        }
    }

    fn analyze_function_body(&mut self, body: &FunctionBody) {
        self.enter_scope();
        // Unlike a local, a param named `self` gets renamed. The implicit
        // `self` of a `:` method is never in the params list.
        for param in body.params.iter() {
            let name = identifier_name(&param.name);
            self.declare_binding(name, is_env_binding(name, self.version));
        }
        self.analyze_block(&body.block);
        self.exit_scope();
    }

    /// Gives every non-fixed binding a slot and returns each slot's liveness.
    fn assign_slots(&mut self) -> Vec<ScopeSet> {
        let mut live_scopes_by_slot: Vec<ScopeSet> = Vec::new();
        let Self { scopes, bindings, .. } = self;
        // Scope ids come from one depth-first walk, so they're in preorder:
        // parents before their children.
        for (scope_id, scope) in scopes.iter().enumerate() {
            for &binding_id in &scope.binding_ids {
                let Some(binding) = bindings.get_mut(binding_id) else {
                    continue;
                };
                if binding.is_fixed {
                    continue;
                }

                // Reuse the first slot that isn't live in this scope.
                let slot = live_scopes_by_slot
                    .iter()
                    .position(|live_scopes| !live_scopes.contains(scope_id))
                    .unwrap_or(live_scopes_by_slot.len());
                if slot == live_scopes_by_slot.len() {
                    live_scopes_by_slot.push(ScopeSet::default());
                }
                if let Some(live_scopes) = live_scopes_by_slot.get_mut(slot) {
                    live_scopes.union_with(&binding.live_scopes);
                }
                binding.slot = Some(slot);
            }
        }
        live_scopes_by_slot
    }

    fn propagate_globals(&mut self) {
        // Reverse order, so children are processed before parents. Scopes are
        // created parent-first, so a scope's parent is always in `earlier`.
        let mut remaining = self.scopes.as_mut_slice();
        while let Some((scope, earlier)) = remaining.split_last_mut() {
            if let Some(parent) = scope.parent.and_then(|parent_id| earlier.get_mut(parent_id)) {
                parent.referenced_globals.extend(scope.referenced_globals.iter().cloned());
            }
            remaining = earlier;
        }
    }

    /// Names a slot may not wear: every global read in or below a scope it's
    /// live in.
    fn forbidden_names(&self, live_scopes: &ScopeSet) -> FxHashSet<&str> {
        live_scopes
            .iter()
            .filter_map(|scope_id| self.scopes.get(scope_id))
            .flat_map(|scope| &scope.referenced_globals)
            .map(CompactString::as_str)
            .collect()
    }

    fn generate_names(&self, live_scopes_by_slot: &[ScopeSet]) -> Vec<CompactString> {
        if live_scopes_by_slot.is_empty() {
            return Vec::new();
        }

        let mut slot_frequencies = vec![0usize; live_scopes_by_slot.len()];
        for binding in &self.bindings {
            if let Some(frequency) = binding.slot.and_then(|slot| slot_frequencies.get_mut(slot)) {
                *frequency = frequency.saturating_add(binding.reference_count);
            }
        }

        // Most-referenced slots pick first and get the shortest names.
        // Unreferenced slots sort last but still appear in the output, so they
        // go through the same conflict check. Otherwise an unused param could
        // take the name of an upvalue read in the same function body.
        let mut slots_by_frequency: Vec<(usize, usize, &ScopeSet)> = slot_frequencies
            .iter()
            .zip(live_scopes_by_slot)
            .enumerate()
            .map(|(slot, (&frequency, live_scopes))| (slot, frequency, live_scopes))
            .collect();
        slots_by_frequency
            .sort_by(|left, right| right.1.cmp(&left.1).then_with(|| left.0.cmp(&right.0)));

        let mut pool = CandidatePool::new(self.version);
        let mut slot_names = vec![CompactString::default(); live_scopes_by_slot.len()];
        for (slot, _, live_scopes) in slots_by_frequency {
            if let Some(name) = slot_names.get_mut(slot) {
                *name = pool.pick(live_scopes, &self.forbidden_names(live_scopes));
            }
        }

        slot_names
    }
}
