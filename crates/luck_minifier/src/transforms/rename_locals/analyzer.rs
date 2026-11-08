//! Scope resolution for the renamer: which binding each name reads, where
//! each binding is live, and which short name each slot of bindings wears.

use core::mem;

use luck_ast::{Block, FunctionBody, Statement, Var, Visitor};
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

#[derive(Clone, Copy, PartialEq, Eq)]
enum BindingKind {
    /// Gets a slot, and with it a short name.
    Renamable,
    /// Keeps its name: a local named `self`, a 5.2+ `_ENV`, or a Luau export.
    Fixed,
    /// Lua 5.5: a `global` declaration. It keeps its name and scopes and
    /// shadows like a local, but its references still reach the real global.
    Global,
}

struct BindingInfo {
    original_name: CompactString,
    scope_id: usize,
    reference_count: usize,
    // `None` until `assign_slots` runs, and always `None` unless renamable.
    slot: Option<usize>,
    kind: BindingKind,
    // The declaring scope, the reference scopes, and every scope on the path
    // between them.
    live_scopes: ScopeSet,
}

/// Resolves every name in one walk, in the order the renamer later visits
/// the same bindings.
///
/// The walk is `Visitor`'s, which reaches nodes in the same order as the
/// renamer's `Transform` walk, so binding ids line up. Both sides walk
/// function bodies and the statements that bind or open a scope by hand, and
/// neither visits the types those declare.
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
            self.declare_binding(name, BindingKind::Renamable);
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

    fn enter_scope(&mut self) {
        let parent = self.current_scope_id;
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

    fn declare_binding(&mut self, name: &str, kind: BindingKind) {
        let scope_id = self.current_scope_id;
        let binding_id = self.bindings.len();
        let mut live_scopes = ScopeSet::default();
        live_scopes.insert(scope_id);
        let original_name = CompactString::from(name);
        self.bindings.push(BindingInfo {
            original_name: original_name.clone(),
            scope_id,
            reference_count: 0,
            slot: None,
            kind,
            live_scopes,
        });
        if let Some(scope) = self.scopes.get_mut(scope_id) {
            scope.binding_ids.push(binding_id);
        }
        self.name_stacks.entry(original_name).or_default().push(binding_id);
    }

    fn declare_local(&mut self, name: &str, is_exported: bool) {
        let is_fixed = name == "self" || is_env_binding(name, self.version) || is_exported;
        let kind = if is_fixed { BindingKind::Fixed } else { BindingKind::Renamable };
        self.declare_binding(name, kind);
    }

    /// Declares a name a parameter list or a `for` loop binds. Unlike a
    /// local, one named `self` gets renamed. The implicit `self` of a `:`
    /// method is never in the params list.
    fn declare_parameter(&mut self, name: &str) {
        let is_fixed = is_env_binding(name, self.version);
        let kind = if is_fixed { BindingKind::Fixed } else { BindingKind::Renamable };
        self.declare_binding(name, kind);
    }

    /// Lua 5.5: declares `name` as a global from here to the end of the scope.
    ///
    /// Lookup finds the newest declaration, so a local renamed to this name and
    /// used after it would resolve to the global instead. Declaring therefore
    /// counts as a global read, even if nothing reads the name.
    fn declare_global(&mut self, name: &str) {
        self.declare_binding(name, BindingKind::Global);
        self.reference_name(name);
    }

    fn reference_name(&mut self, name: &str) {
        let current_scope = self.current_scope_id;
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
            if binding.kind != BindingKind::Global {
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

    fn visit_scoped_block(&mut self, block: &Block) {
        self.enter_scope();
        self.visit_block(block);
        self.exit_scope();
    }

    /// Gives every renamable binding a slot and returns each slot's liveness.
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
                if binding.kind != BindingKind::Renamable {
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

impl<'ast> Visitor<'ast> for Analyzer {
    fn visit_statement(&mut self, stmt: &'ast Statement) {
        match stmt {
            Statement::LocalAssignment(local) => {
                // RHS before LHS: `local x = x` reads the outer x.
                if let Some(exprs) = &local.exprs {
                    for expr in exprs.iter() {
                        self.visit_expression(expr);
                    }
                }
                for attributed in local.names.iter() {
                    self.declare_local(identifier_name(&attributed.name), local.is_exported);
                }
            }
            Statement::LocalFunction(local_function) => {
                self.declare_local(
                    identifier_name(&local_function.name),
                    local_function.is_exported,
                );
                self.visit_function_body(&local_function.body);
            }
            Statement::FunctionDecl(function_declaration) => {
                self.visit_function_body(&function_declaration.body);
                if let Some(root_name) = function_declaration.name.names.first() {
                    self.reference_name(identifier_name(root_name));
                }
            }
            Statement::DoBlock(do_block) => self.visit_scoped_block(&do_block.block),
            Statement::WhileLoop(while_loop) => {
                self.visit_expression(&while_loop.condition);
                self.visit_scoped_block(&while_loop.block);
            }
            Statement::RepeatLoop(repeat_loop) => {
                // The condition sees the block's locals.
                self.enter_scope();
                self.visit_block(&repeat_loop.block);
                self.visit_expression(&repeat_loop.condition);
                self.exit_scope();
            }
            Statement::IfStatement(if_stmt) => {
                self.visit_expression(&if_stmt.condition);
                self.visit_scoped_block(&if_stmt.block);
                for clause in &if_stmt.elseif_clauses {
                    self.visit_expression(&clause.condition);
                    self.visit_scoped_block(&clause.block);
                }
                if let Some(else_clause) = &if_stmt.else_clause {
                    self.visit_scoped_block(&else_clause.block);
                }
            }
            Statement::NumericFor(numeric_for) => {
                self.visit_expression(&numeric_for.start);
                self.visit_expression(&numeric_for.limit);
                if let Some(step) = &numeric_for.step {
                    self.visit_expression(step);
                }
                self.enter_scope();
                self.declare_parameter(identifier_name(&numeric_for.name));
                self.visit_block(&numeric_for.block);
                self.exit_scope();
            }
            Statement::GenericFor(generic_for) => {
                for expr in generic_for.exprs.iter() {
                    self.visit_expression(expr);
                }
                self.enter_scope();
                for param in generic_for.names.iter() {
                    self.declare_parameter(identifier_name(&param.name));
                }
                self.visit_block(&generic_for.block);
                self.exit_scope();
            }
            // Lua 5.5: as with `local function`, the body sees the name.
            Statement::GlobalFunction(global_function) => {
                self.declare_global(identifier_name(&global_function.name));
                self.visit_function_body(&global_function.body);
            }
            // Lua 5.5: as with `local`, the initializers see the outer scope.
            Statement::GlobalDeclaration(global_declaration) => {
                if let Some(exprs) = &global_declaration.exprs {
                    for expr in exprs.iter() {
                        self.visit_expression(expr);
                    }
                }
                for attributed in global_declaration.names.iter() {
                    self.declare_global(identifier_name(&attributed.name));
                }
            }
            // These bind nothing themselves. The walk still reaches the
            // functions they hold, Luau `type function` bodies and `typeof`
            // included, which `Renamer` walks too: skipping one would put the
            // binding ids out of step and the renamer would panic.
            Statement::Assignment(_)
            | Statement::FunctionCall(_)
            | Statement::CompoundAssignment(_)
            | Statement::TypeDeclaration(_)
            | Statement::EmptyStatement(_)
            | Statement::Goto(_)
            | Statement::Label(_)
            | Statement::GlobalStar(_)
            | Statement::Break(_)
            | Statement::Error(_) => self.walk_statement(stmt),
        }
    }

    fn visit_var(&mut self, var: &'ast Var) {
        if let Var::Name(name) = var {
            self.reference_name(identifier_name(name));
        }
        self.walk_var(var);
    }

    fn visit_function_body(&mut self, body: &'ast FunctionBody) {
        self.enter_scope();
        for param in body.params.iter() {
            self.declare_parameter(identifier_name(&param.name));
        }
        self.visit_block(&body.block);
        self.exit_scope();
    }
}
