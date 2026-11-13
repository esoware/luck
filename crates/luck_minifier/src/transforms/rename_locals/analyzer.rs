use core::mem;

use luck_ast::{Block, Expression, FunctionBody, Punctuated, Statement, Type, Var, Visitor};
use luck_token::{CompactString, LuaVersion};
use rustc_hash::{FxHashMap, FxHashSet};

use super::candidate_pool::CandidatePool;
use super::renamer::RenamedBinding;
use super::scope_claims::ScopeClaims;
use crate::binding::{
    IMPLICIT_VARARG_TABLE_NAME, METHOD_RECEIVER_NAME, has_implicit_arg, identifier_name,
    is_env_binding,
};

struct ScopeNode {
    parent: Option<usize>,
    binding_ids: Vec<usize>,
    // After `propagate_globals`, also holds every descendant scope's globals.
    referenced_globals: FxHashSet<CompactString>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum BindingKind {
    Renamable,
    /// Keeps its name: a function's implicit `self` or 5.1 `arg`, a 5.2+
    /// `_ENV`, or a Luau export.
    Fixed,
    /// Lua 5.5: a `global` declaration. It keeps its name and scopes and
    /// shadows like a local, but its references still reach the real global.
    Global,
}

struct BindingInfo {
    original_name: CompactString,
    reference_count: usize,
    // `None` until `assign_slots` runs, and always `None` unless renamable.
    slot: Option<usize>,
    kind: BindingKind,
    // The declaring scope, the reference scopes, and every scope on the path
    // between them, in ascending order.
    live_scopes: Vec<usize>,
}

/// The walk is `Visitor`'s, which reaches nodes in the same order as the
/// renamer's `Transform` walk, so binding ids line up.
///
/// Both sides walk function bodies and the statements that bind or open a scope by hand, so
/// annotations and the names they bind come in the same order on each.
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

    pub(super) fn renamed_bindings(mut self) -> Vec<RenamedBinding> {
        self.propagate_globals();
        let live_scopes_by_slot = self.assign_slots();
        let slot_names = self.generate_names(&live_scopes_by_slot);
        self.bindings
            .into_iter()
            .map(|binding| {
                let new_name = binding
                    .slot
                    .and_then(|slot| slot_names.get(slot).cloned())
                    .unwrap_or_else(|| binding.original_name.clone());
                RenamedBinding::new(binding.original_name, new_name)
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

    /// Declares a binding. One that keeps its name counts as read in its own
    /// scope, even if nothing reads it, since a local renamed to that name
    /// would otherwise capture every use of it.
    fn declare_binding(&mut self, name: &str, kind: BindingKind) {
        let scope_id = self.current_scope_id;
        let binding_id = self.bindings.len();
        let original_name = CompactString::from(name);
        self.bindings.push(BindingInfo {
            original_name: original_name.clone(),
            reference_count: 0,
            slot: None,
            kind,
            live_scopes: vec![scope_id],
        });
        if let Some(scope) = self.scopes.get_mut(scope_id) {
            scope.binding_ids.push(binding_id);
        }
        self.name_stacks.entry(original_name).or_default().push(binding_id);
        if kind != BindingKind::Renamable {
            self.reference_name(name);
        }
    }

    fn declare_local(&mut self, name: &str, is_exported: bool) {
        let is_fixed = is_env_binding(name, self.version) || is_exported;
        let kind = if is_fixed { BindingKind::Fixed } else { BindingKind::Renamable };
        self.declare_binding(name, kind);
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
            // A scope that's live already brought its ancestors in with it,
            // and the declaring scope is live from the start. A scope not yet
            // live was entered after every one that is, so the new ones go on
            // the end, outermost first.
            let live_count = binding.live_scopes.len();
            let mut next_scope = Some(current_scope);
            while let Some(scope_id) = next_scope {
                let is_live = binding
                    .live_scopes
                    .get(..live_count)
                    .is_some_and(|live_scopes| live_scopes.binary_search(&scope_id).is_ok());
                if is_live {
                    break;
                }
                binding.live_scopes.push(scope_id);
                next_scope = scopes.get(scope_id).and_then(|scope| scope.parent);
            }
            if let Some(new_scopes) = binding.live_scopes.get_mut(live_count..) {
                new_scopes.reverse();
            }
            if binding.kind == BindingKind::Renamable {
                return;
            }
        }
        // An unresolved name, or a binding that keeps its name, reads a name
        // no slot live here may wear. The globals being renamed are
        // pre-declared as root bindings, so their references resolve above
        // and never get here.
        if let Some(scope) = scopes.get_mut(current_scope) {
            scope.referenced_globals.insert(name.into());
        }
    }

    fn visit_exprs(&mut self, exprs: &Punctuated<Expression>) {
        for expr in exprs.iter() {
            self.visit_expr(expr);
        }
    }

    fn visit_annotation(&mut self, annotation: Option<&Type>) {
        if let Some(ty) = annotation {
            self.visit_type(ty);
        }
    }

    /// Luau: a function's signature annotations, in the order the renamer
    /// reaches them. Parameters and a `local function`'s own name bind after
    /// the whole signature parses, so a `typeof` here reads the enclosing
    /// scope.
    fn visit_signature_annotations(&mut self, body: &FunctionBody) {
        if let Some(generics) = &body.generics {
            self.visit_generic_type_list(generics);
        }
        for param in body.params.iter() {
            self.visit_annotation(param.type_annotation.as_ref());
        }
        if let Some(vararg) = &body.vararg {
            self.visit_annotation(vararg.type_annotation.as_ref());
        }
        self.visit_annotation(body.return_type.as_ref());
    }

    /// Scopes a function's parameters and implicit names, in the order the
    /// renamer declares them, and visits its block.
    fn visit_function_scope(&mut self, body: &FunctionBody, is_method: bool) {
        self.enter_scope();
        if is_method {
            self.declare_binding(METHOD_RECEIVER_NAME, BindingKind::Fixed);
        }
        for param in body.params.iter() {
            self.declare_local(identifier_name(&param.name), false);
        }
        if has_implicit_arg(body, self.version) {
            self.declare_binding(IMPLICIT_VARARG_TABLE_NAME, BindingKind::Fixed);
        }
        // Lua 5.5
        if let Some(vararg_name) = body.vararg.as_ref().and_then(|vararg| vararg.name.as_ref()) {
            self.declare_local(identifier_name(vararg_name), false);
        }
        self.visit_block(&body.block);
        self.exit_scope();
    }

    fn visit_scoped_block(&mut self, block: &Block) {
        self.enter_scope();
        self.visit_block(block);
        self.exit_scope();
    }

    /// Returns each slot's live scopes.
    fn assign_slots(&mut self) -> Vec<Vec<usize>> {
        let mut live_scopes_by_slot: Vec<Vec<usize>> = Vec::new();
        let mut live_slots = ScopeClaims::default();
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
                let slot = live_slots.first_free(scope_id);
                if slot == live_scopes_by_slot.len() {
                    live_scopes_by_slot.push(Vec::new());
                }
                live_slots.claim(&binding.live_scopes, slot);
                if let Some(live_scopes) = live_scopes_by_slot.get_mut(slot) {
                    live_scopes.extend_from_slice(&binding.live_scopes);
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

    fn generate_names(&self, live_scopes_by_slot: &[Vec<usize>]) -> Vec<CompactString> {
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
        let mut slots_by_frequency: Vec<(usize, usize, &Vec<usize>)> = slot_frequencies
            .iter()
            .zip(live_scopes_by_slot)
            .enumerate()
            .map(|(slot, (&frequency, live_scopes))| (slot, frequency, live_scopes))
            .collect();
        slots_by_frequency.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));

        // A slot may not wear the name of a global read in or below a scope
        // it's live in.
        let reads_global = |scope_id: usize, name: &str| {
            self.scopes.get(scope_id).is_some_and(|scope| scope.referenced_globals.contains(name))
        };
        let mut pool = CandidatePool::new(self.version);
        let mut slot_names = vec![CompactString::default(); live_scopes_by_slot.len()];
        for (slot, _, live_scopes) in slots_by_frequency {
            if let Some(name) = slot_names.get_mut(slot) {
                *name = pool.pick(live_scopes, reads_global);
            }
        }

        slot_names
    }
}

impl<'ast> Visitor<'ast> for Analyzer {
    fn visit_stmt(&mut self, stmt: &'ast Statement) {
        match stmt {
            Statement::LocalAssignment(local) => {
                // RHS and annotations before LHS: in
                // `local x: typeof(x) = x` both read the outer x.
                if let Some(exprs) = &local.exprs {
                    self.visit_exprs(exprs);
                }
                for attributed in local.names.iter() {
                    self.visit_annotation(attributed.type_annotation.as_ref());
                }
                for attributed in local.names.iter() {
                    self.declare_local(identifier_name(&attributed.name), local.is_exported);
                }
            }
            Statement::LocalFunction(local_function) => {
                self.visit_signature_annotations(&local_function.body);
                self.declare_local(
                    identifier_name(&local_function.name),
                    local_function.is_exported,
                );
                self.visit_function_scope(&local_function.body, false);
            }
            Statement::FunctionDecl(function_declaration) => {
                self.visit_signature_annotations(&function_declaration.body);
                self.visit_function_scope(
                    &function_declaration.body,
                    function_declaration.name.method.is_some(),
                );
                if let Some(root_name) = function_declaration.name.names.first() {
                    self.reference_name(identifier_name(root_name));
                }
            }
            Statement::DoBlock(do_block) => self.visit_scoped_block(&do_block.block),
            Statement::WhileLoop(while_loop) => {
                self.visit_expr(&while_loop.condition);
                self.visit_scoped_block(&while_loop.block);
            }
            Statement::RepeatLoop(repeat_loop) => {
                // The condition sees the block's locals.
                self.enter_scope();
                self.visit_block(&repeat_loop.block);
                self.visit_expr(&repeat_loop.condition);
                self.exit_scope();
            }
            Statement::IfStatement(if_stmt) => {
                self.visit_expr(&if_stmt.condition);
                self.visit_scoped_block(&if_stmt.block);
                for clause in &if_stmt.elseif_clauses {
                    self.visit_expr(&clause.condition);
                    self.visit_scoped_block(&clause.block);
                }
                if let Some(else_clause) = &if_stmt.else_clause {
                    self.visit_scoped_block(&else_clause.block);
                }
            }
            Statement::NumericFor(numeric_for) => {
                self.visit_expr(&numeric_for.start);
                self.visit_expr(&numeric_for.limit);
                if let Some(step) = &numeric_for.step {
                    self.visit_expr(step);
                }
                // Luau
                self.visit_annotation(numeric_for.type_annotation.as_ref());
                self.enter_scope();
                self.declare_local(identifier_name(&numeric_for.name), false);
                self.visit_block(&numeric_for.block);
                self.exit_scope();
            }
            Statement::GenericFor(generic_for) => {
                self.visit_exprs(&generic_for.exprs);
                // Luau: every name binds after the whole list parses.
                for param in generic_for.names.iter() {
                    self.visit_annotation(param.type_annotation.as_ref());
                }
                self.enter_scope();
                for param in generic_for.names.iter() {
                    self.declare_local(identifier_name(&param.name), false);
                }
                self.visit_block(&generic_for.block);
                self.exit_scope();
            }
            // Lua 5.5: as with `local function`, the body sees the name.
            Statement::GlobalFunction(global_function) => {
                self.declare_binding(identifier_name(&global_function.name), BindingKind::Global);
                self.visit_function_body(&global_function.body);
            }
            // Lua 5.5: as with `local`, the initializers see the outer scope.
            Statement::GlobalDeclaration(global_declaration) => {
                if let Some(exprs) = &global_declaration.exprs {
                    self.visit_exprs(exprs);
                }
                for attributed in global_declaration.names.iter() {
                    self.declare_binding(identifier_name(&attributed.name), BindingKind::Global);
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
            | Statement::Error(_) => self.walk_stmt(stmt),
        }
    }

    fn visit_var(&mut self, var: &'ast Var) {
        if let Var::Name(name) = var {
            self.reference_name(identifier_name(name));
        }
        self.walk_var(var);
    }

    fn visit_function_body(&mut self, body: &'ast FunctionBody) {
        self.visit_signature_annotations(body);
        self.visit_function_scope(body, false);
    }
}

#[cfg(test)]
mod tests {
    use core::fmt::Write as _;

    use super::*;
    use crate::fixtures::parse;

    #[test]
    fn a_binding_holds_only_the_scopes_it_is_live_in() {
        // A set sized by scope id, one per binding, takes memory quadratic in
        // the number of sibling scopes.
        let mut source = String::new();
        for scope in 0..2_000u32 {
            let _unused = writeln!(source, "do local x{scope} = f() g(x{scope}) end");
        }
        let mut analyzer = Analyzer::new(LuaVersion::Lua54);
        analyzer.visit_block(&parse(&source, LuaVersion::Lua54));
        assert_eq!(analyzer.bindings.len(), 2_000);
        assert!(analyzer.bindings.iter().all(|binding| binding.live_scopes.len() == 1));
    }

    #[test]
    fn live_scopes_stay_sorted_whatever_order_references_come_in() {
        let source = "local x do do f(x) end end do g(x) do do h(x) end end end do end f(x)";
        let mut analyzer = Analyzer::new(LuaVersion::Lua54);
        analyzer.visit_block(&parse(source, LuaVersion::Lua54));
        let live_scopes = &analyzer.bindings.first().expect("x is declared").live_scopes;
        assert_eq!(live_scopes, &[0, 1, 2, 3, 4, 5]);
    }
}
