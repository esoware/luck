use rustc_hash::{FxHashMap, FxHashSet};

use luck_ast::expr::*;
use luck_ast::shared::*;
use luck_ast::stmt::*;
use luck_ast::transform::AstTransform;
use luck_core::types::LuaTarget;
use luck_token::CompactString;

use crate::expr::ident_name;
use crate::name_gen::name_for_index;
use crate::tokens::make_ident;

pub fn rename(block: Block, target: LuaTarget, rename_globals: bool) -> Block {
    // Renaming file-defined globals pollutes `_G` under different keys -
    // strictly opt-in (TransformConfig::rename_globals).
    let (func_globals, assign_globals) = if rename_globals {
        let func_globals = collect_toplevel_function_globals(&block);
        let assign_globals = collect_assignfirst_globals(&block, &func_globals);
        (func_globals, assign_globals)
    } else {
        (FxHashSet::default(), FxHashSet::default())
    };

    // True globals come from real binding resolution: the analyzer walks
    // the same positional scoping rules as the renamer (declaration
    // order, shadowing, repeat-until conditions, function name scoping),
    // so any reference that resolves to no binding is a global. A flat
    // name-set that excluded any name used as a local ANYWHERE in the
    // file let a global read be captured by a renamed local sharing its
    // name.
    let mut analyzer = Analyzer::new(&func_globals, &assign_globals);
    analyzer.analyze_block(&block);
    analyzer.propagate_globals();

    let (slot_count, slot_liveness) = analyzer.assign_slots();

    let slot_names = analyzer.generate_names(slot_count, target.keywords(), &slot_liveness);

    let binding_new_names: Vec<CompactString> = analyzer
        .bindings
        .iter()
        .map(|binding| {
            if binding.is_fixed || binding.slot == u32::MAX {
                binding.original_name.clone()
            } else {
                slot_names[binding.slot as usize].clone()
            }
        })
        .collect();

    let mut renamer = AstRenamer::new(binding_new_names, &func_globals, &assign_globals);
    renamer.pre_declare_root_block(&block);
    renamer.transform_block(block)
}

/// Scope-id set as a bitset: scope ids are small and dense, and the hot
/// operations are whole-set union and intersection.
#[derive(Default)]
struct ScopeSet(Vec<u64>);

impl ScopeSet {
    fn insert(&mut self, scope: usize) {
        let word = scope / 64;
        if word >= self.0.len() {
            self.0.resize(word + 1, 0);
        }
        self.0[word] |= 1 << (scope % 64);
    }

    fn contains(&self, scope: usize) -> bool {
        self.0
            .get(scope / 64)
            .is_some_and(|word| word & (1 << (scope % 64)) != 0)
    }

    fn union_with(&mut self, other: &ScopeSet) {
        if other.0.len() > self.0.len() {
            self.0.resize(other.0.len(), 0);
        }
        for (dst, src) in self.0.iter_mut().zip(&other.0) {
            *dst |= src;
        }
    }

    fn intersects(&self, other: &ScopeSet) -> bool {
        self.0.iter().zip(&other.0).any(|(a, b)| a & b != 0)
    }

    fn iter(&self) -> impl Iterator<Item = usize> + '_ {
        self.0.iter().enumerate().flat_map(|(word_idx, &word)| {
            (0..64)
                .filter(move |bit| word & (1 << bit) != 0)
                .map(move |bit| word_idx * 64 + bit)
        })
    }
}

struct ScopeNode {
    parent: Option<usize>,
    children: Vec<usize>,
    binding_ids: Vec<usize>,
    // after propagate_globals: includes globals from all descendant scopes too
    global_refs: FxHashSet<CompactString>,
}

struct BindingInfo {
    original_name: CompactString,
    scope_id: usize,
    ref_count: usize,
    slot: u32,
    is_fixed: bool,
    // declaring scope + reference scopes + all scopes on the path between them
    live_scopes: ScopeSet,
}

struct Analyzer<'globals> {
    scopes: Vec<ScopeNode>,
    bindings: Vec<BindingInfo>,
    // NonEmptyStack layout: the top lives in `current_scope_id` so reading
    // it is branchless; `outer_scope_ids` holds only the enclosing scopes.
    current_scope_id: usize,
    outer_scope_ids: Vec<usize>,
    // original name -> stack of binding IDs; innermost binding on top
    name_stack: FxHashMap<CompactString, Vec<usize>>,
    func_globals: &'globals FxHashSet<CompactString>,
    assign_globals: &'globals FxHashSet<CompactString>,
}

impl<'globals> Analyzer<'globals> {
    fn new(
        func_globals: &'globals FxHashSet<CompactString>,
        assign_globals: &'globals FxHashSet<CompactString>,
    ) -> Self {
        let root_scope = ScopeNode {
            parent: None,
            children: Vec::new(),
            binding_ids: Vec::new(),
            global_refs: FxHashSet::default(),
        };
        Self {
            scopes: vec![root_scope],
            bindings: Vec::new(),
            current_scope_id: 0,
            outer_scope_ids: Vec::new(),
            name_stack: FxHashMap::default(),
            func_globals,
            assign_globals,
        }
    }

    fn current_scope(&self) -> usize {
        self.current_scope_id
    }

    fn enter_scope(&mut self) -> usize {
        let parent = self.current_scope();
        let new_id = self.scopes.len();
        self.scopes.push(ScopeNode {
            parent: Some(parent),
            children: Vec::new(),
            binding_ids: Vec::new(),
            global_refs: FxHashSet::default(),
        });
        self.scopes[parent].children.push(new_id);
        self.outer_scope_ids
            .push(std::mem::replace(&mut self.current_scope_id, new_id));
        new_id
    }

    fn exit_scope(&mut self) {
        let scope_id = self.current_scope_id;
        self.current_scope_id = self.outer_scope_ids.pop().expect("scope stack underflow");
        let Self {
            scopes,
            bindings,
            name_stack,
            ..
        } = self;
        for &binding_id in scopes[scope_id].binding_ids.iter().rev() {
            let name = &bindings[binding_id].original_name;
            if let Some(stack) = name_stack.get_mut(name) {
                stack.pop();
                if stack.is_empty() {
                    name_stack.remove(name);
                }
            }
        }
    }

    fn declare_binding(&mut self, name: &str, is_fixed: bool) -> usize {
        let scope_id = self.current_scope();
        let binding_id = self.bindings.len();
        let mut live_scopes = ScopeSet::default();
        live_scopes.insert(scope_id);
        let name = CompactString::from(name);
        self.bindings.push(BindingInfo {
            original_name: name.clone(),
            scope_id,
            ref_count: 0,
            slot: u32::MAX,
            is_fixed,
            live_scopes,
        });
        self.scopes[scope_id].binding_ids.push(binding_id);
        self.name_stack.entry(name).or_default().push(binding_id);
        binding_id
    }

    fn reference_name(&mut self, name: &str) {
        if let Some(&binding_id) = self.name_stack.get(name).and_then(|s| s.last()) {
            self.bindings[binding_id].ref_count += 1;
            // mark every scope from here up to the declaration as live
            let declaring_scope = self.bindings[binding_id].scope_id;
            let mut scope = self.current_scope();
            loop {
                self.bindings[binding_id].live_scopes.insert(scope);
                if scope == declaring_scope {
                    break;
                }
                match self.scopes[scope].parent {
                    Some(parent) => scope = parent,
                    None => break,
                }
            }
            return;
        }
        // Unresolved means global: func/assign pseudo-globals are
        // pre-declared as root bindings, so every reference to them
        // resolves above and never lands here.
        let scope_id = self.current_scope();
        self.scopes[scope_id].global_refs.insert(name.into());
    }

    fn analyze_block(&mut self, block: &Block) {
        if self.outer_scope_ids.is_empty() && self.bindings.is_empty() {
            self.pre_declare_root_bindings(block);
        }

        for stmt in &block.stmts {
            self.analyze_stmt(stmt);
        }
        if let Some(last) = &block.last_stmt {
            self.analyze_last_stmt(last);
        }
    }

    fn pre_declare_root_bindings(&mut self, block: &Block) {
        for stmt in &block.stmts {
            if let Statement::FunctionDecl(func_decl) = stmt
                && func_decl.name.names.len() == 1
                && func_decl.name.method.is_none()
            {
                let name = ident_name(&func_decl.name.names[0]);
                if self.func_globals.contains(name) {
                    self.declare_binding(name, false);
                }
            }
        }
        for stmt in &block.stmts {
            if let Statement::Assignment(assign) = stmt {
                for var in assign.targets.iter() {
                    if let Var::Name(name_tok) = var {
                        let name = ident_name(name_tok);
                        if self.assign_globals.contains(name) && !self.name_stack.contains_key(name)
                        {
                            self.declare_binding(name, false);
                        }
                    }
                }
            }
        }
    }

    fn analyze_stmt(&mut self, stmt: &Statement) {
        match stmt {
            Statement::LocalAssignment(local) => {
                // RHS before LHS: `local x = x` reads the outer x
                if let Some(exprs) = &local.exprs {
                    for expr in exprs.iter() {
                        self.analyze_expr(expr);
                    }
                }
                for name_tok in local.names.iter() {
                    let name = ident_name(&name_tok.name);
                    let is_fixed = name == "self" || name == "_ENV" || local.is_exported;
                    self.declare_binding(name, is_fixed);
                }
            }
            Statement::LocalFunction(local_func) => {
                let name = ident_name(&local_func.name);
                let is_fixed = name == "self" || name == "_ENV" || local_func.is_exported;
                self.declare_binding(name, is_fixed);
                self.analyze_function_body(&local_func.body);
            }
            Statement::FunctionDecl(func_decl) => {
                self.analyze_function_body(&func_decl.body);
                if !func_decl.name.names.is_empty() {
                    let name = ident_name(&func_decl.name.names[0]);
                    self.reference_name(name);
                }
            }
            Statement::Assignment(assign) => {
                for expr in assign.values.iter() {
                    self.analyze_expr(expr);
                }
                for var in assign.targets.iter() {
                    self.analyze_var(var);
                }
            }
            Statement::FunctionCall(call_stmt) => {
                self.analyze_function_call(&call_stmt.call);
            }
            Statement::DoBlock(do_block) => {
                self.enter_scope();
                self.analyze_block(&do_block.block);
                self.exit_scope();
            }
            Statement::WhileLoop(while_loop) => {
                self.analyze_expr(&while_loop.condition);
                self.enter_scope();
                self.analyze_block(&while_loop.block);
                self.exit_scope();
            }
            Statement::RepeatLoop(repeat_loop) => {
                self.enter_scope();
                self.analyze_block(&repeat_loop.block);
                self.analyze_expr(&repeat_loop.condition);
                self.exit_scope();
            }
            Statement::IfStatement(if_stmt) => {
                self.analyze_expr(&if_stmt.condition);
                self.enter_scope();
                self.analyze_block(&if_stmt.block);
                self.exit_scope();
                for clause in &if_stmt.elseif_clauses {
                    self.analyze_expr(&clause.condition);
                    self.enter_scope();
                    self.analyze_block(&clause.block);
                    self.exit_scope();
                }
                if let Some(else_clause) = &if_stmt.else_clause {
                    self.enter_scope();
                    self.analyze_block(&else_clause.block);
                    self.exit_scope();
                }
            }
            Statement::NumericFor(numeric_for) => {
                self.analyze_expr(&numeric_for.start);
                self.analyze_expr(&numeric_for.limit);
                if let Some(step) = &numeric_for.step {
                    self.analyze_expr(step);
                }
                self.enter_scope();
                self.declare_binding(ident_name(&numeric_for.name), false);
                self.analyze_block(&numeric_for.block);
                self.exit_scope();
            }
            Statement::GenericFor(generic_for) => {
                for expr in generic_for.exprs.iter() {
                    self.analyze_expr(expr);
                }
                self.enter_scope();
                for binding in generic_for.names.iter() {
                    self.declare_binding(ident_name(&binding.name), false);
                }
                self.analyze_block(&generic_for.block);
                self.exit_scope();
            }
            Statement::CompoundAssignment(ca) => {
                self.analyze_var(&ca.var);
                self.analyze_expr(&ca.expr);
            }
            // Lua 5.5: the global function name is itself a global, but its body
            // declares params/locals that AstRenamer renames via walk_function_body,
            // so the analyzer must match that walk to keep binding indices in sync.
            Statement::GlobalFunction(global_func) => {
                self.analyze_function_body(&global_func.body);
            }
            Statement::GlobalDeclaration(global_decl) => {
                if let Some(exprs) = &global_decl.exprs {
                    for expr in exprs.iter() {
                        self.analyze_expr(expr);
                    }
                }
            }
            // Luau `type function` bodies are ordinary code that AstRenamer
            // walks via walk_function_body; the analyzer must match or the
            // binding indices desync (and the renamer panics).
            Statement::TypeDeclaration(type_decl) => {
                if let TypeDeclarationValue::TypeFunction(body) = &type_decl.type_value {
                    self.analyze_function_body(body);
                }
            }
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
        if let LastStatement::Return(ret) = last {
            for expr in ret.exprs.iter() {
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
            Expression::FunctionDef(func) => self.analyze_function_body(&func.body),
            Expression::TableConstructor(table) => {
                for field in table.fields.iter() {
                    match field {
                        Field::Bracketed { key, value, .. } => {
                            self.analyze_expr(key);
                            self.analyze_expr(value);
                        }
                        Field::Named { value, .. } => self.analyze_expr(value),
                        Field::Positional { value, .. } => self.analyze_expr(value),
                    }
                }
            }
            Expression::InterpolatedString(interp) => {
                for seg in &interp.segments {
                    if let Some(ref expr) = seg.expr {
                        self.analyze_expr(expr);
                    }
                }
            }
            Expression::IfExpression(if_expr) => {
                self.analyze_expr(&if_expr.condition);
                self.analyze_expr(&if_expr.then_expr);
                for clause in &if_expr.elseif_clauses {
                    self.analyze_expr(&clause.condition);
                    self.analyze_expr(&clause.expr);
                }
                self.analyze_expr(&if_expr.else_expr);
            }
            Expression::TypeCast(cast) => self.analyze_expr(&cast.expr),
            Expression::TypeInstantiation(instantiation) => {
                self.analyze_expr(&instantiation.expr);
            }
            // Literal leaves: no sub-expressions, no name references.
            Expression::Nil(_)
            | Expression::False(_)
            | Expression::True(_)
            | Expression::Number(_)
            | Expression::Integer(_) // Luau
            | Expression::StringLiteral(_)
            | Expression::VarArg(_)
            | Expression::Error(_) => {}
        }
    }

    fn analyze_var(&mut self, var: &Var) {
        match var {
            Var::Name(name) => self.reference_name(ident_name(name)),
            Var::FieldAccess(fa) => self.analyze_expr(&fa.prefix),
            Var::Index(ie) => {
                self.analyze_expr(&ie.prefix);
                self.analyze_expr(&ie.index);
            }
        }
    }

    fn analyze_function_call(&mut self, call: &FunctionCall) {
        self.analyze_expr(&call.callee);
        match &call.args {
            FunctionArgs::Parenthesized { args, .. } => {
                for arg in args.iter() {
                    self.analyze_expr(arg);
                }
            }
            FunctionArgs::TableConstructor(table) => {
                for field in table.fields.iter() {
                    match field {
                        Field::Bracketed { key, value, .. } => {
                            self.analyze_expr(key);
                            self.analyze_expr(value);
                        }
                        Field::Named { value, .. } => self.analyze_expr(value),
                        Field::Positional { value, .. } => self.analyze_expr(value),
                    }
                }
            }
            FunctionArgs::StringLiteral(_) => {}
        }
    }

    fn analyze_function_body(&mut self, body: &FunctionBody) {
        self.enter_scope();
        // explicit params are always renameable - implicit `self` from `:` syntax
        // never appears in the params list
        for param in body.params.iter() {
            let name = ident_name(&param.name);
            self.declare_binding(name, name == "_ENV");
        }
        self.analyze_block(&body.block);
        self.exit_scope();
    }
}

impl Analyzer<'_> {
    fn assign_slots(&mut self) -> (usize, Vec<ScopeSet>) {
        let mut slot_liveness: Vec<ScopeSet> = Vec::new();
        let mut total_slots: usize = 0;
        self.assign_slots_dfs(0, &mut slot_liveness, &mut total_slots);
        (total_slots, slot_liveness)
    }

    fn assign_slots_dfs(
        &mut self,
        scope_id: usize,
        slot_liveness: &mut Vec<ScopeSet>,
        total_slots: &mut usize,
    ) {
        for binding_idx in 0..self.scopes[scope_id].binding_ids.len() {
            let binding_id = self.scopes[scope_id].binding_ids[binding_idx];
            if self.bindings[binding_id].is_fixed {
                continue;
            }

            // Find a reusable slot: one not live in this scope
            let mut found_slot = None;
            for (slot_idx, liveness) in slot_liveness.iter().enumerate() {
                if !liveness.contains(scope_id) {
                    found_slot = Some(slot_idx as u32);
                    break;
                }
            }

            let slot = match found_slot {
                Some(s) => s,
                None => {
                    let s = slot_liveness.len() as u32;
                    slot_liveness.push(ScopeSet::default());
                    s
                }
            };

            self.bindings[binding_id].slot = slot;
            *total_slots = (*total_slots).max((slot + 1) as usize);

            slot_liveness[slot as usize].union_with(&self.bindings[binding_id].live_scopes);
        }

        for child_idx in 0..self.scopes[scope_id].children.len() {
            let child_id = self.scopes[scope_id].children[child_idx];
            self.assign_slots_dfs(child_id, slot_liveness, total_slots);
        }
    }
}

impl Analyzer<'_> {
    fn propagate_globals(&mut self) {
        // reverse order so children are processed before parents
        for scope_id in (0..self.scopes.len()).rev() {
            for child_idx in 0..self.scopes[scope_id].children.len() {
                let child_id = self.scopes[scope_id].children[child_idx];
                // Scopes are created parent-first, so child_id > scope_id.
                let (head, tail) = self.scopes.split_at_mut(child_id);
                head[scope_id]
                    .global_refs
                    .extend(tail[0].global_refs.iter().cloned());
            }
        }
    }

    fn generate_names(
        &self,
        slot_count: usize,
        keywords: &[&'static str],
        slot_liveness: &[ScopeSet],
    ) -> Vec<CompactString> {
        if slot_count == 0 {
            return Vec::new();
        }

        let mut slot_frequencies: Vec<(u32, usize)> =
            (0..slot_count).map(|s| (s as u32, 0_usize)).collect();

        for binding in &self.bindings {
            if binding.is_fixed || binding.slot == u32::MAX {
                continue;
            }
            slot_frequencies[binding.slot as usize].1 += binding.ref_count;
        }

        let mut slot_forbidden: Vec<FxHashSet<&str>> = Vec::with_capacity(slot_count);
        for slot_idx in 0..slot_count {
            let mut forbidden = FxHashSet::default();
            if slot_idx < slot_liveness.len() {
                for scope_id in slot_liveness[slot_idx].iter() {
                    for global_name in &self.scopes[scope_id].global_refs {
                        forbidden.insert(global_name.as_str());
                    }
                }
            }
            slot_forbidden.push(forbidden);
        }

        let mut active_slots: Vec<(u32, usize)> = slot_frequencies
            .iter()
            .copied()
            .filter(|&(_, freq)| freq > 0)
            .collect();

        active_slots.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));

        let mut pool = CandidatePool::new(keywords);
        let mut slot_to_name = vec![CompactString::default(); slot_count];

        for &(slot, _) in &active_slots {
            slot_to_name[slot as usize] = pool.pick(
                &slot_liveness[slot as usize],
                &slot_forbidden[slot as usize],
            );
        }

        // Zero-frequency slots (declared but never referenced) still wear
        // a name in the output, so they run through the same liveness
        // conflict check: handing them the first short name regardless
        // captured co-live slots (an unreferenced param stole the name of
        // an upvalue used inside the same function body).
        for slot in 0..slot_count {
            if slot_to_name[slot].is_empty() {
                slot_to_name[slot] = pool.pick(&slot_liveness[slot], &slot_forbidden[slot]);
            }
        }

        slot_to_name
    }
}

/// Shared candidate-name state for [`Analyzer::generate_names`]. Every
/// slot scans candidates from index 0, so the generated names (and their
/// keyword-ness) are computed once and shared.
struct CandidatePool {
    keyword_set: FxHashSet<&'static str>,
    candidates: Vec<CompactString>,
    is_keyword: Vec<bool>,
    // Per candidate: the scopes where the name is already worn by some
    // slot. Non-overlapping slots can share the same name.
    taken_scopes: Vec<ScopeSet>,
}

impl CandidatePool {
    fn new(keywords: &[&'static str]) -> Self {
        Self {
            keyword_set: keywords.iter().copied().collect(),
            candidates: Vec::new(),
            is_keyword: Vec::new(),
            taken_scopes: Vec::new(),
        }
    }

    fn pick(&mut self, slot_live: &ScopeSet, forbidden: &FxHashSet<&str>) -> CompactString {
        let mut idx = 0;
        loop {
            if idx == self.candidates.len() {
                let name = name_for_index(idx);
                self.is_keyword
                    .push(self.keyword_set.contains(name.as_str()));
                self.candidates.push(name);
                self.taken_scopes.push(ScopeSet::default());
            }
            let candidate_idx = idx;
            idx += 1;
            if slot_live.intersects(&self.taken_scopes[candidate_idx])
                || self.is_keyword[candidate_idx]
            {
                continue;
            }
            let candidate = &self.candidates[candidate_idx];
            if forbidden.contains(candidate.as_str()) {
                continue;
            }
            self.taken_scopes[candidate_idx].union_with(slot_live);
            return self.candidates[candidate_idx].clone();
        }
    }
}

struct AstRenamer<'globals> {
    binding_new_names: Vec<CompactString>,
    next_binding_id: usize,
    name_stack: FxHashMap<CompactString, Vec<CompactString>>,
    scope_binding_names: Vec<Vec<CompactString>>,

    func_globals: &'globals FxHashSet<CompactString>,
    assign_globals: &'globals FxHashSet<CompactString>,
}

impl<'globals> AstRenamer<'globals> {
    fn new(
        binding_new_names: Vec<CompactString>,
        func_globals: &'globals FxHashSet<CompactString>,
        assign_globals: &'globals FxHashSet<CompactString>,
    ) -> Self {
        Self {
            binding_new_names,
            next_binding_id: 0,
            name_stack: FxHashMap::default(),
            scope_binding_names: vec![Vec::new()],
            func_globals,
            assign_globals,
        }
    }

    fn declare_binding(&mut self, original: &str) -> CompactString {
        let new_name = self.binding_new_names[self.next_binding_id].clone();
        self.next_binding_id += 1;
        let original = CompactString::from(original);
        if let Some(scope_names) = self.scope_binding_names.last_mut() {
            scope_names.push(original.clone());
        }
        self.name_stack
            .entry(original)
            .or_default()
            .push(new_name.clone());
        new_name
    }

    fn enter_scope(&mut self) {
        self.scope_binding_names.push(Vec::new());
    }

    fn exit_scope(&mut self) {
        if let Some(scope_names) = self.scope_binding_names.pop() {
            for original in scope_names.iter().rev() {
                if let Some(stack) = self.name_stack.get_mut(original) {
                    stack.pop();
                    if stack.is_empty() {
                        self.name_stack.remove(original);
                    }
                }
            }
        }
    }

    fn resolve_name(&self, original: &str) -> CompactString {
        if let Some(stack) = self.name_stack.get(original)
            && let Some(new_name) = stack.last()
        {
            return new_name.clone();
        }
        original.into()
    }

    // must match Analyzer::pre_declare_root_bindings exactly
    fn pre_declare_root_block(&mut self, block: &Block) {
        for stmt in &block.stmts {
            if let Statement::FunctionDecl(func_decl) = stmt
                && func_decl.name.names.len() == 1
                && func_decl.name.method.is_none()
            {
                let name = ident_name(&func_decl.name.names[0]);
                if self.func_globals.contains(name) {
                    self.declare_binding(name);
                }
            }
        }
        for stmt in &block.stmts {
            if let Statement::Assignment(assign) = stmt {
                for var in assign.targets.iter() {
                    if let Var::Name(name_tok) = var {
                        let name = ident_name(name_tok);
                        if self.assign_globals.contains(name) && !self.name_stack.contains_key(name)
                        {
                            self.declare_binding(name);
                        }
                    }
                }
            }
        }
    }
}

impl AstTransform for AstRenamer<'_> {
    fn transform_block(&mut self, block: Block) -> Block {
        let new_stmts: Vec<_> = block
            .stmts
            .into_iter()
            .map(|stmt| self.transform_statement(stmt))
            .collect();
        let new_last = block
            .last_stmt
            .map(|last| Box::new(self.transform_last_statement(*last)));
        Block {
            span: block.span,
            stmts: new_stmts,
            last_stmt: new_last,
        }
    }

    fn transform_statement(&mut self, stmt: Statement) -> Statement {
        match stmt {
            Statement::LocalAssignment(mut local) => {
                // Transform expressions first (in outer scope)
                local.exprs = local.exprs.map(|exprs| self.walk_punctuated_exprs(exprs));
                local.names =
                    rename_attributed_names(&mut |orig| self.declare_binding(orig), local.names);
                Statement::LocalAssignment(local)
            }
            Statement::LocalFunction(mut local_func) => {
                let new_name = self.declare_binding(ident_name(&local_func.name));
                local_func.name = make_ident(&new_name);
                local_func.body = self.walk_function_body(local_func.body);
                Statement::LocalFunction(local_func)
            }
            Statement::FunctionDecl(mut func_decl) => {
                func_decl.body = self.walk_function_body(func_decl.body);
                if !func_decl.name.names.is_empty() {
                    let resolved = self.resolve_name(ident_name(&func_decl.name.names[0]));
                    func_decl.name.names[0] = make_ident(&resolved);
                }
                Statement::FunctionDecl(func_decl)
            }
            Statement::DoBlock(mut do_block) => {
                self.enter_scope();
                do_block.block = self.transform_block(do_block.block);
                self.exit_scope();
                Statement::DoBlock(do_block)
            }
            Statement::WhileLoop(mut while_loop) => {
                while_loop.condition = self.transform_expression(while_loop.condition);
                self.enter_scope();
                while_loop.block = self.transform_block(while_loop.block);
                self.exit_scope();
                Statement::WhileLoop(while_loop)
            }
            Statement::RepeatLoop(mut repeat_loop) => {
                self.enter_scope();
                repeat_loop.block = self.transform_block(repeat_loop.block);
                repeat_loop.condition = self.transform_expression(repeat_loop.condition);
                self.exit_scope();
                Statement::RepeatLoop(repeat_loop)
            }
            Statement::IfStatement(mut if_stmt) => {
                if_stmt.condition = self.transform_expression(if_stmt.condition);
                self.enter_scope();
                if_stmt.block = self.transform_block(if_stmt.block);
                self.exit_scope();
                if_stmt.elseif_clauses = if_stmt
                    .elseif_clauses
                    .into_iter()
                    .map(|mut clause| {
                        clause.condition = self.transform_expression(clause.condition);
                        self.enter_scope();
                        clause.block = self.transform_block(clause.block);
                        self.exit_scope();
                        clause
                    })
                    .collect();
                if_stmt.else_clause = if_stmt.else_clause.map(|mut else_clause| {
                    self.enter_scope();
                    else_clause.block = self.transform_block(else_clause.block);
                    self.exit_scope();
                    else_clause
                });
                Statement::IfStatement(if_stmt)
            }
            Statement::NumericFor(mut numeric_for) => {
                numeric_for.start = self.transform_expression(numeric_for.start);
                numeric_for.limit = self.transform_expression(numeric_for.limit);
                numeric_for.step = numeric_for.step.map(|step| self.transform_expression(step));
                self.enter_scope();
                let new_name = self.declare_binding(ident_name(&numeric_for.name));
                numeric_for.name = make_ident(&new_name);
                numeric_for.block = self.transform_block(numeric_for.block);
                self.exit_scope();
                Statement::NumericFor(numeric_for)
            }
            Statement::GenericFor(mut generic_for) => {
                generic_for.exprs = self.walk_punctuated_exprs(generic_for.exprs);
                self.enter_scope();
                generic_for.names = rename_punctuated_names(
                    &mut |orig| self.declare_binding(orig),
                    generic_for.names,
                );
                generic_for.block = self.transform_block(generic_for.block);
                self.exit_scope();
                Statement::GenericFor(generic_for)
            }
            // These declare no new scope of their own here; walk_statement
            // recurses through their expressions/bodies, resolving references
            // in lockstep with Analyzer::analyze_stmt (which visits the same
            // children, including GlobalFunction bodies).
            stmt @ (Statement::Assignment(_)
            | Statement::FunctionCall(_)
            | Statement::CompoundAssignment(_)
            | Statement::GlobalFunction(_)
            | Statement::EmptyStatement(_)
            | Statement::Goto(_)
            | Statement::Label(_)
            | Statement::GlobalDeclaration(_)
            | Statement::GlobalStar(_)
            | Statement::Break(_)
            | Statement::TypeDeclaration(_)
            | Statement::Error(_)) => self.walk_statement(stmt),
        }
    }

    fn transform_var(&mut self, var: Var) -> Var {
        match var {
            Var::Name(name) => {
                let resolved = self.resolve_name(ident_name(&name));
                Var::Name(make_ident(&resolved))
            }
            other => self.walk_var(other),
        }
    }

    fn walk_function_body(&mut self, mut body: FunctionBody) -> FunctionBody {
        self.enter_scope();
        body.params.items = body
            .params
            .items
            .into_iter()
            .map(|mut param| {
                let new_name = self.declare_binding(ident_name(&param.name));
                param.name = make_ident(&new_name);
                param
            })
            .collect();
        body.block = self.transform_block(body.block);
        self.exit_scope();
        body
    }

    fn walk_function_call(&mut self, mut call: FunctionCall) -> FunctionCall {
        call.callee = match call.callee {
            Expression::Var(var) => match var {
                Var::Name(name) => {
                    let resolved = self.resolve_name(ident_name(&name));
                    Expression::Var(Var::Name(make_ident(&resolved)))
                }
                other => Expression::Var(self.transform_var(other)),
            },
            // Any non-Var callee (call chains, parens, etc.) resolves its names
            // through the normal expression transform.
            callee @ (Expression::Nil(_)
            | Expression::False(_)
            | Expression::True(_)
            | Expression::Number(_)
            | Expression::Integer(_)
            | Expression::StringLiteral(_)
            | Expression::VarArg(_)
            | Expression::FunctionDef(_)
            | Expression::FunctionCall(_)
            | Expression::Parenthesized(_)
            | Expression::TableConstructor(_)
            | Expression::BinaryOp(_)
            | Expression::UnaryOp(_)
            | Expression::IfExpression(_)
            | Expression::InterpolatedString(_)
            | Expression::TypeCast(_)
            | Expression::TypeInstantiation(_)
            | Expression::Error(_)) => self.transform_expression(callee),
        };
        call.explicit_type_args = call
            .explicit_type_args
            .map(|type_args| Box::new(self.walk_type_args(*type_args)));
        call.args = self.walk_function_args(call.args);
        call
    }
}

/// Rename every declared local while keeping its `<attrib>` attached.
fn rename_attributed_names(
    declare: &mut dyn FnMut(&str) -> CompactString,
    names: Punctuated<AttributedName>,
) -> Punctuated<AttributedName> {
    let rename = |attributed: AttributedName| {
        let new_name = declare(ident_name(&attributed.name));
        AttributedName {
            name: make_ident(&new_name),
            // Renaming a local never changes its declared type
            type_annotation: attributed.type_annotation,
            attrib: attributed.attrib,
        }
    };
    let mut names = names;
    names.items = names.items.into_iter().map(rename).collect();
    names
}

fn rename_punctuated_names(
    declare: &mut dyn FnMut(&str) -> CompactString,
    mut names: Punctuated<luck_ast::Parameter>,
) -> Punctuated<luck_ast::Parameter> {
    names.items = names
        .items
        .into_iter()
        .map(|mut binding| {
            let new_name = declare(ident_name(&binding.name));
            // Renaming a loop binding never changes its declared type
            binding.name = make_ident(&new_name);
            binding
        })
        .collect();
    names
}

fn collect_toplevel_function_globals(block: &Block) -> FxHashSet<CompactString> {
    let mut top_locals: FxHashSet<CompactString> = FxHashSet::default();
    for stmt in &block.stmts {
        match stmt {
            Statement::LocalAssignment(local) => {
                for attributed in local.names.iter() {
                    top_locals.insert(ident_name(&attributed.name).into());
                }
            }
            Statement::LocalFunction(local_func) => {
                top_locals.insert(ident_name(&local_func.name).into());
            }
            // Only `local` declarations introduce local names at this level;
            // everything else either declares globals or no name binding.
            Statement::Assignment(_)
            | Statement::FunctionCall(_)
            | Statement::DoBlock(_)
            | Statement::WhileLoop(_)
            | Statement::RepeatLoop(_)
            | Statement::IfStatement(_)
            | Statement::NumericFor(_)
            | Statement::GenericFor(_)
            | Statement::FunctionDecl(_)
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

    let mut func_globals = FxHashSet::default();
    for stmt in &block.stmts {
        if let Statement::FunctionDecl(func_decl) = stmt
            && func_decl.name.names.len() == 1
            && func_decl.name.method.is_none()
        {
            let name = ident_name(&func_decl.name.names[0]);
            if !top_locals.contains(name) {
                func_globals.insert(name.into());
            }
        }
    }

    func_globals
}

fn collect_assignfirst_globals(
    block: &Block,
    func_globals: &FxHashSet<CompactString>,
) -> FxHashSet<CompactString> {
    let mut read_globals: FxHashSet<CompactString> = FxHashSet::default();
    let mut assign_first: FxHashSet<CompactString> = FxHashSet::default();

    let mut all_locals = FxHashSet::default();
    collect_top_level_locals(block, &mut all_locals);

    for stmt in &block.stmts {
        match stmt {
            Statement::Assignment(assign) => {
                for expr in assign.values.iter() {
                    collect_name_reads_from_expr(expr, &all_locals, &mut read_globals);
                }
                for var in assign.targets.iter() {
                    if let Var::Name(name) = var {
                        let var_name = ident_name(name);
                        if !all_locals.contains(var_name)
                            && !func_globals.contains(var_name)
                            && !read_globals.contains(var_name)
                            && !assign_first.contains(var_name)
                        {
                            assign_first.insert(var_name.into());
                        }
                    }
                    if let Var::FieldAccess(fa) = var
                        && let Expression::Var(Var::Name(name)) = &fa.prefix
                    {
                        let var_name = ident_name(name);
                        if !all_locals.contains(var_name) {
                            read_globals.insert(var_name.into());
                        }
                    }
                }
            }
            // Every non-assignment statement contributes only reads here;
            // assignment targets are special-cased above for assign-first.
            Statement::FunctionCall(_)
            | Statement::LocalAssignment(_)
            | Statement::DoBlock(_)
            | Statement::WhileLoop(_)
            | Statement::RepeatLoop(_)
            | Statement::IfStatement(_)
            | Statement::NumericFor(_)
            | Statement::GenericFor(_)
            | Statement::FunctionDecl(_)
            | Statement::LocalFunction(_)
            | Statement::GlobalFunction(_)
            | Statement::EmptyStatement(_)
            | Statement::Goto(_)
            | Statement::Label(_)
            | Statement::GlobalDeclaration(_)
            | Statement::GlobalStar(_)
            | Statement::Break(_)
            | Statement::CompoundAssignment(_)
            | Statement::TypeDeclaration(_)
            | Statement::Error(_) => {
                collect_name_reads_from_stmt(stmt, &all_locals, &mut read_globals);
            }
        }
    }

    assign_first
}

fn collect_top_level_locals(block: &Block, locals: &mut FxHashSet<CompactString>) {
    for stmt in &block.stmts {
        match stmt {
            Statement::LocalAssignment(local) => {
                for attributed in local.names.iter() {
                    locals.insert(ident_name(&attributed.name).into());
                }
            }
            Statement::LocalFunction(local_func) => {
                locals.insert(ident_name(&local_func.name).into());
            }
            Statement::DoBlock(d) => collect_top_level_locals(&d.block, locals),
            Statement::WhileLoop(w) => collect_top_level_locals(&w.block, locals),
            Statement::RepeatLoop(r) => collect_top_level_locals(&r.block, locals),
            Statement::IfStatement(if_stmt) => {
                collect_top_level_locals(&if_stmt.block, locals);
                for clause in &if_stmt.elseif_clauses {
                    collect_top_level_locals(&clause.block, locals);
                }
                if let Some(else_clause) = &if_stmt.else_clause {
                    collect_top_level_locals(&else_clause.block, locals);
                }
            }
            Statement::NumericFor(nf) => {
                locals.insert(ident_name(&nf.name).into());
                collect_top_level_locals(&nf.block, locals);
            }
            Statement::GenericFor(gf) => {
                for binding in gf.names.iter() {
                    locals.insert(ident_name(&binding.name).into());
                }
                collect_top_level_locals(&gf.block, locals);
            }
            // Function bodies are separate scopes (a `LocalFunction` name is
            // captured above, but its body is not). Everything else either
            // declares no local at this level or declares a global.
            Statement::Assignment(_)
            | Statement::FunctionCall(_)
            | Statement::FunctionDecl(_)
            | Statement::GlobalFunction(_)
            | Statement::EmptyStatement(_)
            | Statement::Goto(_)
            | Statement::Label(_)
            | Statement::GlobalDeclaration(_)
            | Statement::GlobalStar(_)
            | Statement::Break(_)
            | Statement::CompoundAssignment(_)
            | Statement::TypeDeclaration(_)
            | Statement::Error(_) => {}
        }
    }
}

fn collect_name_reads_from_expr(
    expr: &Expression,
    locals: &FxHashSet<CompactString>,
    reads: &mut FxHashSet<CompactString>,
) {
    match expr {
        Expression::Var(var) => match var {
            Var::Name(name) => {
                let var_name = ident_name(name);
                if !locals.contains(var_name) {
                    reads.insert(var_name.into());
                }
            }
            Var::FieldAccess(fa) => {
                collect_name_reads_from_expr(&fa.prefix, locals, reads);
            }
            Var::Index(ie) => {
                collect_name_reads_from_expr(&ie.prefix, locals, reads);
                collect_name_reads_from_expr(&ie.index, locals, reads);
            }
        },
        Expression::FunctionCall(call) => {
            collect_name_reads_from_expr(&call.callee, locals, reads);
            collect_name_reads_from_func_args(&call.args, locals, reads);
        }
        Expression::BinaryOp(binop) => {
            collect_name_reads_from_expr(&binop.left, locals, reads);
            collect_name_reads_from_expr(&binop.right, locals, reads);
        }
        Expression::UnaryOp(unop) => {
            collect_name_reads_from_expr(&unop.operand, locals, reads);
        }
        Expression::Parenthesized(paren) => {
            collect_name_reads_from_expr(&paren.expr, locals, reads);
        }
        Expression::TableConstructor(table) => {
            for field in table.fields.iter() {
                match field {
                    Field::Bracketed { key, value, .. } => {
                        collect_name_reads_from_expr(key, locals, reads);
                        collect_name_reads_from_expr(value, locals, reads);
                    }
                    Field::Named { value, .. } => {
                        collect_name_reads_from_expr(value, locals, reads);
                    }
                    Field::Positional { value, .. } => {
                        collect_name_reads_from_expr(value, locals, reads);
                    }
                }
            }
        }
        Expression::InterpolatedString(interp) => {
            for seg in &interp.segments {
                if let Some(ref expr) = seg.expr {
                    collect_name_reads_from_expr(expr, locals, reads);
                }
            }
        }
        Expression::IfExpression(if_expr) => {
            collect_name_reads_from_expr(&if_expr.condition, locals, reads);
            collect_name_reads_from_expr(&if_expr.then_expr, locals, reads);
            for clause in &if_expr.elseif_clauses {
                collect_name_reads_from_expr(&clause.condition, locals, reads);
                collect_name_reads_from_expr(&clause.expr, locals, reads);
            }
            collect_name_reads_from_expr(&if_expr.else_expr, locals, reads);
        }
        // Type cast is transparent: the inner expression may read a global.
        Expression::TypeCast(cast) => {
            collect_name_reads_from_expr(&cast.expr, locals, reads);
        }
        Expression::TypeInstantiation(instantiation) => {
            collect_name_reads_from_expr(&instantiation.expr, locals, reads);
        }
        // A function body's reads belong to its own scope, not this one.
        Expression::FunctionDef(_) => {}
        // Literal leaves: no sub-expressions, no name reads.
        Expression::Nil(_)
        | Expression::False(_)
        | Expression::True(_)
        | Expression::Number(_)
        | Expression::Integer(_) // Luau
        | Expression::StringLiteral(_)
        | Expression::VarArg(_)
        | Expression::Error(_) => {}
    }
}

fn collect_name_reads_from_func_args(
    args: &FunctionArgs,
    locals: &FxHashSet<CompactString>,
    reads: &mut FxHashSet<CompactString>,
) {
    match args {
        FunctionArgs::Parenthesized { args, .. } => {
            for arg in args.iter() {
                collect_name_reads_from_expr(arg, locals, reads);
            }
        }
        FunctionArgs::TableConstructor(table) => {
            for field in table.fields.iter() {
                match field {
                    Field::Bracketed { key, value, .. } => {
                        collect_name_reads_from_expr(key, locals, reads);
                        collect_name_reads_from_expr(value, locals, reads);
                    }
                    Field::Named { value, .. } => {
                        collect_name_reads_from_expr(value, locals, reads);
                    }
                    Field::Positional { value, .. } => {
                        collect_name_reads_from_expr(value, locals, reads);
                    }
                }
            }
        }
        // A string-literal argument reads no names.
        FunctionArgs::StringLiteral(_) => {}
    }
}

fn collect_name_reads_from_stmt(
    stmt: &Statement,
    locals: &FxHashSet<CompactString>,
    reads: &mut FxHashSet<CompactString>,
) {
    match stmt {
        Statement::FunctionCall(call_stmt) => {
            collect_name_reads_from_expr(&call_stmt.call.callee, locals, reads);
            collect_name_reads_from_func_args(&call_stmt.call.args, locals, reads);
        }
        Statement::LocalAssignment(local) => {
            if let Some(exprs) = &local.exprs {
                for expr in exprs.iter() {
                    collect_name_reads_from_expr(expr, locals, reads);
                }
            }
        }
        Statement::Assignment(assign) => {
            for expr in assign.values.iter() {
                collect_name_reads_from_expr(expr, locals, reads);
            }
            for var in assign.targets.iter() {
                match var {
                    Var::Name(name) => {
                        let var_name = ident_name(name);
                        if !locals.contains(var_name) {
                            reads.insert(var_name.into());
                        }
                    }
                    Var::FieldAccess(fa) => {
                        collect_name_reads_from_expr(&fa.prefix, locals, reads);
                    }
                    Var::Index(ie) => {
                        collect_name_reads_from_expr(&ie.prefix, locals, reads);
                        collect_name_reads_from_expr(&ie.index, locals, reads);
                    }
                }
            }
        }
        Statement::DoBlock(d) => {
            for s in &d.block.stmts {
                collect_name_reads_from_stmt(s, locals, reads);
            }
        }
        Statement::WhileLoop(w) => {
            collect_name_reads_from_expr(&w.condition, locals, reads);
            for s in &w.block.stmts {
                collect_name_reads_from_stmt(s, locals, reads);
            }
        }
        Statement::RepeatLoop(r) => {
            for s in &r.block.stmts {
                collect_name_reads_from_stmt(s, locals, reads);
            }
            collect_name_reads_from_expr(&r.condition, locals, reads);
        }
        Statement::IfStatement(if_stmt) => {
            collect_name_reads_from_expr(&if_stmt.condition, locals, reads);
            for s in &if_stmt.block.stmts {
                collect_name_reads_from_stmt(s, locals, reads);
            }
            for clause in &if_stmt.elseif_clauses {
                collect_name_reads_from_expr(&clause.condition, locals, reads);
                for s in &clause.block.stmts {
                    collect_name_reads_from_stmt(s, locals, reads);
                }
            }
            if let Some(else_clause) = &if_stmt.else_clause {
                for s in &else_clause.block.stmts {
                    collect_name_reads_from_stmt(s, locals, reads);
                }
            }
        }
        Statement::NumericFor(nf) => {
            collect_name_reads_from_expr(&nf.start, locals, reads);
            collect_name_reads_from_expr(&nf.limit, locals, reads);
            if let Some(step) = &nf.step {
                collect_name_reads_from_expr(step, locals, reads);
            }
            for s in &nf.block.stmts {
                collect_name_reads_from_stmt(s, locals, reads);
            }
        }
        Statement::GenericFor(gf) => {
            for expr in gf.exprs.iter() {
                collect_name_reads_from_expr(expr, locals, reads);
            }
            for s in &gf.block.stmts {
                collect_name_reads_from_stmt(s, locals, reads);
            }
        }
        Statement::CompoundAssignment(ca) => {
            match &ca.var {
                Var::Name(name) => {
                    let var_name = ident_name(name);
                    if !locals.contains(var_name) {
                        reads.insert(var_name.into());
                    }
                }
                Var::FieldAccess(fa) => {
                    collect_name_reads_from_expr(&fa.prefix, locals, reads);
                }
                Var::Index(ie) => {
                    collect_name_reads_from_expr(&ie.prefix, locals, reads);
                }
            }
            collect_name_reads_from_expr(&ca.expr, locals, reads);
        }
        // Function bodies are separate scopes; their reads don't count here.
        Statement::FunctionDecl(_) | Statement::LocalFunction(_) | Statement::GlobalFunction(_) => {
        }
        Statement::GlobalDeclaration(global_decl) => {
            if let Some(exprs) = &global_decl.exprs {
                for expr in exprs.iter() {
                    collect_name_reads_from_expr(expr, locals, reads);
                }
            }
        }
        // Leaves and declarations that read no names at this scope.
        Statement::EmptyStatement(_)
        | Statement::Goto(_)
        | Statement::Label(_)
        | Statement::GlobalStar(_)
        | Statement::Break(_)
        | Statement::TypeDeclaration(_)
        | Statement::Error(_) => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn apply(source: &str) -> String {
        let result = luck_parser::parse(source, luck_token::LuaVersion::Lua54);
        assert!(result.errors.is_empty(), "parse failed");
        let block = rename(result.block, LuaTarget::Lua54, true);
        luck_codegen::compact(&block, source)
    }

    fn reparses(source: &str) -> bool {
        let result = luck_parser::parse(source, luck_token::LuaVersion::Lua54);
        result.errors.is_empty()
    }

    #[test]
    fn renames_locals_to_short_names() {
        let result = apply("local longname = 1\nreturn longname\n");
        assert!(reparses(&result), "Parse errors\nOutput: {result}");
        assert!(
            !result.contains("longname"),
            "Local should be renamed: {result}"
        );
    }

    #[test]
    fn preserves_globals() {
        let result = apply("print(42)\n");
        assert!(
            result.contains("print"),
            "Global should not be renamed: {result}"
        );
    }

    #[test]
    fn renames_function_params() {
        let result = apply("local function foo(longparam)\n  return longparam\nend\nreturn foo\n");
        assert!(reparses(&result), "Parse errors\nOutput: {result}");
        assert!(
            !result.contains("longparam"),
            "Param should be renamed: {result}"
        );
    }

    #[test]
    fn preserves_field_names() {
        let result = apply("local t = {}\nt.field = 1\nreturn t.field\n");
        assert!(
            result.contains("field"),
            "Field name should not be renamed: {result}"
        );
    }

    #[test]
    fn scoped_renames_reparse() {
        let result = apply("local x = 1\ndo\n  local y = 2\n  print(y)\nend\nreturn x\n");
        assert!(reparses(&result), "Parse errors\nOutput: {result}");
    }

    #[test]
    fn most_frequent_local_gets_shortest_name() {
        let result = apply(
            "local rare = 1\nlocal frequent = 2\nprint(frequent)\nprint(frequent)\nprint(frequent)\nprint(rare)\nreturn frequent, rare\n",
        );
        assert!(reparses(&result), "Parse errors\nOutput: {result}");
        assert!(
            !result.contains("frequent"),
            "'frequent' should be renamed: {result}"
        );
        assert!(
            !result.contains("rare"),
            "'rare' should be renamed: {result}"
        );
    }

    #[test]
    fn preserves_self_parameter() {
        let result = apply("local t = {}\nfunction t:method()\n  return self\nend\nreturn t\n");
        assert!(
            result.contains("self"),
            "'self' should not be renamed: {result}"
        );
    }

    #[test]
    fn sibling_scopes_reuse_names() {
        let result = apply(
            "do\n  local longvar = 1\n  print(longvar)\nend\ndo\n  local another = 2\n  print(another)\nend\n",
        );
        assert!(reparses(&result), "Parse errors\nOutput: {result}");
        assert!(
            !result.contains("longvar") && !result.contains("another"),
            "Both should be renamed: {result}"
        );
    }

    #[test]
    fn nested_function_frequency_beats_outer() {
        let result = apply(
            "local outer = 1\nlocal function f()\n  local inner = 2\n  print(inner, inner, inner, inner, inner)\nend\nreturn outer, f\n",
        );
        assert!(reparses(&result), "Parse errors\nOutput: {result}");
    }

    #[test]
    fn locals_can_shadow_unused_globals() {
        // 'l' and 'u' are globals at top level but not inside f
        let result = apply(concat!(
            "l(1)\nu(2)\n",
            "local function f()\n",
            "  local longvar1 = 3\n  local longvar2 = 4\n",
            "  return longvar1 + longvar2\nend\n",
            "return f\n",
        ));
        assert!(reparses(&result), "Parse errors\nOutput: {result}");
        assert!(
            !result.contains("longvar"),
            "locals should be renamed: {result}"
        );
    }

    #[test]
    fn locals_avoid_globals_used_in_same_scope() {
        // 'l' is a global called inside f - locals must not shadow it
        let result = apply(concat!(
            "local function f()\n",
            "  local longvar = 1\n  l(longvar)\n  return longvar\n",
            "end\nreturn f\n",
        ));
        assert!(reparses(&result), "Parse errors\nOutput: {result}");
        assert!(
            result.contains("l("),
            "global 'l' must be preserved: {result}"
        );
        assert!(
            !result.contains("longvar"),
            "longvar should be renamed: {result}"
        );
    }

    fn apply_lua55(source: &str) -> String {
        let result = luck_parser::parse(source, luck_token::LuaVersion::Lua55);
        assert!(
            result.errors.is_empty(),
            "parse failed: {:?}",
            result.errors
        );
        let block = rename(result.block, LuaTarget::Lua55, true);
        luck_codegen::compact(&block, source)
    }

    #[test]
    fn renames_inside_global_function_body() {
        // Regression: the analyzer skipped `global function` bodies while the
        // renamer walked them, leaving binding indices out of sync - the renamer
        // then panicked with an out-of-bounds binding lookup.
        let result = apply_lua55("global function f(longparam)\n  return longparam\nend\n");
        let reparsed = luck_parser::parse(&result, luck_token::LuaVersion::Lua55);
        assert!(reparsed.errors.is_empty(), "must reparse: {result}");
        assert!(
            !result.contains("longparam"),
            "param inside global function should be renamed: {result}"
        );
        assert!(
            result.contains("function f("),
            "global function name must be preserved: {result}"
        );
    }

    fn apply_luau(source: &str) -> String {
        let result = luck_parser::parse(source, luck_token::LuaVersion::Luau);
        assert!(
            result.errors.is_empty(),
            "parse failed: {:?}",
            result.errors
        );
        let block = rename(result.block, LuaTarget::Luau, true);
        luck_codegen::compact(&block, source)
    }

    #[test]
    fn global_read_via_typecast_is_not_assign_first() {
        // `g` is read (inside a type cast) before being assigned, so it is a real
        // global and must not be reclassified as an assign-first pseudo-local and
        // renamed. Regression: the assign-first read collector skipped TypeCast,
        // miscompiling `h=(g::any)g=1 print(h,g)` into renamed locals.
        let result = apply_luau("h = (g :: any)\ng = 1\nprint(h, g)\n");
        let reparsed = luck_parser::parse(&result, luck_token::LuaVersion::Luau);
        assert!(reparsed.errors.is_empty(), "must reparse: {result}");
        assert!(
            result.contains('g'),
            "global 'g' read via typecast must be preserved: {result}"
        );
    }

    #[test]
    fn unreferenced_param_never_captures_live_upvalue() {
        // The zero-frequency name fallback used to hand an unreferenced
        // param the same short name as an upvalue read inside the body,
        // capturing it.
        let result = apply(
            "local cache = string.rep
function W.new(p)
	print(cache)
end
cache = f()
",
        );
        let reparsed = luck_parser::parse(&result, luck_token::LuaVersion::Lua54);
        assert!(reparsed.errors.is_empty(), "must reparse: {result}");
        // The upvalue read inside the body and the outer local must
        // still be the SAME name, and the param a different one.
        let body_start = result.find("function W.new(").expect("decl kept") + 15;
        let param = &result[body_start..body_start + 1];
        let outer = &result[6..7];
        assert_ne!(
            param, outer,
            "param must not shadow the captured upvalue: {result}"
        );
    }
}
