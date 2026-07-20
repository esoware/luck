use std::collections::{HashMap, HashSet};

use luck_ast::expr::*;
use luck_ast::shared::*;
use luck_ast::stmt::*;
use luck_ast::transform::AstTransform;
use luck_core::types::LuaTarget;

use crate::expr::ident_name_string;
use crate::name_gen::NameGenerator;
use crate::tokens::make_ident;

pub fn rename(block: Block, target: LuaTarget, rename_globals: bool) -> Block {
    // True globals come from real binding resolution: every reference
    // that resolves to no local. The old flat name-set excluded any name
    // used as a local ANYWHERE in the file, so a global read could be
    // captured by a renamed local sharing its name.
    let analysis = luck_semantic::analyze_with_environment(
        &block,
        target.lua_version(),
        target.stdlib_environment(),
    );
    let global_names: HashSet<String> = analysis
        .scope_tree
        .references
        .iter()
        .filter(|reference| reference.resolved.is_none())
        .map(|reference| reference.name.to_string())
        .collect();
    // Renaming file-defined globals pollutes `_G` under different keys -
    // strictly opt-in (TransformConfig::rename_globals).
    let (func_globals, assign_globals) = if rename_globals {
        let func_globals = collect_toplevel_function_globals(&block);
        let assign_globals = collect_assignfirst_globals(&block, &func_globals);
        (func_globals, assign_globals)
    } else {
        (HashSet::new(), HashSet::new())
    };

    let mut true_globals = global_names;
    for name in func_globals.iter().chain(assign_globals.iter()) {
        true_globals.remove(name);
    }

    let mut analyzer = Analyzer::new(&true_globals, &func_globals, &assign_globals);
    analyzer.analyze_block(&block);
    analyzer.propagate_globals();

    let (slot_count, slot_liveness) = analyzer.assign_slots();

    let slot_names =
        analyzer.generate_names(slot_count, target.keywords(), &true_globals, &slot_liveness);

    let binding_new_names: Vec<String> = analyzer
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

    let mut renamer = AstRenamer::new(
        binding_new_names,
        &true_globals,
        &func_globals,
        &assign_globals,
    );
    renamer.pre_declare_root_block(&block);
    renamer.transform_block(block)
}

struct ScopeNode {
    parent: Option<usize>,
    children: Vec<usize>,
    binding_ids: Vec<usize>,
    // after propagate_globals: includes globals from all descendant scopes too
    global_refs: HashSet<String>,
}

struct BindingInfo {
    original_name: String,
    scope_id: usize,
    ref_count: usize,
    slot: u32,
    is_fixed: bool,
    // declaring scope + reference scopes + all scopes on the path between them
    live_scopes: HashSet<usize>,
}

struct Analyzer {
    scopes: Vec<ScopeNode>,
    bindings: Vec<BindingInfo>,
    // NonEmptyStack layout: the top lives in `current_scope_id` so reading
    // it is branchless; `outer_scope_ids` holds only the enclosing scopes.
    current_scope_id: usize,
    outer_scope_ids: Vec<usize>,
    // original name -> stack of binding IDs; innermost binding on top
    name_stack: HashMap<String, Vec<usize>>,
    true_globals: HashSet<String>,
    func_globals: HashSet<String>,
    assign_globals: HashSet<String>,
}

impl Analyzer {
    fn new(
        true_globals: &HashSet<String>,
        func_globals: &HashSet<String>,
        assign_globals: &HashSet<String>,
    ) -> Self {
        let root_scope = ScopeNode {
            parent: None,
            children: Vec::new(),
            binding_ids: Vec::new(),
            global_refs: HashSet::new(),
        };
        Self {
            scopes: vec![root_scope],
            bindings: Vec::new(),
            current_scope_id: 0,
            outer_scope_ids: Vec::new(),
            name_stack: HashMap::new(),
            true_globals: true_globals.clone(),
            func_globals: func_globals.clone(),
            assign_globals: assign_globals.clone(),
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
            global_refs: HashSet::new(),
        });
        self.scopes[parent].children.push(new_id);
        self.outer_scope_ids
            .push(std::mem::replace(&mut self.current_scope_id, new_id));
        new_id
    }

    fn exit_scope(&mut self) {
        let scope_id = self.current_scope_id;
        self.current_scope_id = self.outer_scope_ids.pop().expect("scope stack underflow");
        let binding_ids: Vec<usize> = self.scopes[scope_id].binding_ids.clone();
        for &binding_id in binding_ids.iter().rev() {
            let name = &self.bindings[binding_id].original_name;
            if let Some(stack) = self.name_stack.get_mut(name) {
                stack.pop();
                if stack.is_empty() {
                    self.name_stack.remove(name);
                }
            }
        }
    }

    fn declare_binding(&mut self, name: &str, is_fixed: bool) -> usize {
        let scope_id = self.current_scope();
        let binding_id = self.bindings.len();
        let mut live_scopes = HashSet::new();
        live_scopes.insert(scope_id);
        self.bindings.push(BindingInfo {
            original_name: name.to_string(),
            scope_id,
            ref_count: 0,
            slot: u32::MAX,
            is_fixed,
            live_scopes,
        });
        self.scopes[scope_id].binding_ids.push(binding_id);
        self.name_stack
            .entry(name.to_string())
            .or_default()
            .push(binding_id);
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
        if self.true_globals.contains(name) {
            let scope_id = self.current_scope();
            self.scopes[scope_id].global_refs.insert(name.to_string());
        }
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
                let name = ident_name_string(&func_decl.name.names[0]);
                if self.func_globals.contains(&name) {
                    self.declare_binding(&name, false);
                }
            }
        }
        for stmt in &block.stmts {
            if let Statement::Assignment(assign) = stmt {
                for var in assign.targets.iter() {
                    if let Var::Name(name_tok) = var {
                        let name = ident_name_string(name_tok);
                        if self.assign_globals.contains(&name)
                            && !self.name_stack.contains_key(&name)
                        {
                            self.declare_binding(&name, false);
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
                    let name = ident_name_string(&name_tok.name);
                    let is_fixed = name == "self" || name == "_ENV";
                    self.declare_binding(&name, is_fixed);
                }
            }
            Statement::LocalFunction(local_func) => {
                let name = ident_name_string(&local_func.name);
                let is_fixed = name == "self" || name == "_ENV";
                self.declare_binding(&name, is_fixed);
                self.analyze_function_body(&local_func.body);
            }
            Statement::FunctionDecl(func_decl) => {
                self.analyze_function_body(&func_decl.body);
                if !func_decl.name.names.is_empty() {
                    let name = ident_name_string(&func_decl.name.names[0]);
                    self.reference_name(&name);
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
                self.declare_binding(&ident_name_string(&numeric_for.name), false);
                self.analyze_block(&numeric_for.block);
                self.exit_scope();
            }
            Statement::GenericFor(generic_for) => {
                for expr in generic_for.exprs.iter() {
                    self.analyze_expr(expr);
                }
                self.enter_scope();
                for binding in generic_for.names.iter() {
                    self.declare_binding(&ident_name_string(&binding.name), false);
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
            // Literal leaves: no sub-expressions, no name references.
            Expression::Nil(_)
            | Expression::False(_)
            | Expression::True(_)
            | Expression::Number(_)
            | Expression::StringLiteral(_)
            | Expression::VarArg(_)
            | Expression::Error(_) => {}
        }
    }

    fn analyze_var(&mut self, var: &Var) {
        match var {
            Var::Name(name) => self.reference_name(&ident_name_string(name)),
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
            let name = ident_name_string(&param.name);
            self.declare_binding(&name, name == "_ENV");
        }
        self.analyze_block(&body.block);
        self.exit_scope();
    }
}

impl Analyzer {
    fn assign_slots(&mut self) -> (usize, Vec<HashSet<usize>>) {
        let mut slot_liveness: Vec<HashSet<usize>> = Vec::new();
        let mut total_slots: usize = 0;
        self.assign_slots_dfs(0, &mut slot_liveness, &mut total_slots);
        (total_slots, slot_liveness)
    }

    fn assign_slots_dfs(
        &mut self,
        scope_id: usize,
        slot_liveness: &mut Vec<HashSet<usize>>,
        total_slots: &mut usize,
    ) {
        let binding_ids: Vec<usize> = self.scopes[scope_id].binding_ids.clone();

        for binding_id in &binding_ids {
            if self.bindings[*binding_id].is_fixed {
                continue;
            }

            // Find a reusable slot: one not live in this scope
            let mut found_slot = None;
            for (slot_idx, liveness) in slot_liveness.iter().enumerate() {
                if !liveness.contains(&scope_id) {
                    found_slot = Some(slot_idx as u32);
                    break;
                }
            }

            let slot = match found_slot {
                Some(s) => s,
                None => {
                    let s = slot_liveness.len() as u32;
                    slot_liveness.push(HashSet::new());
                    s
                }
            };

            self.bindings[*binding_id].slot = slot;
            *total_slots = (*total_slots).max((slot + 1) as usize);

            // Merge this binding's liveness into the slot
            let live_scopes = self.bindings[*binding_id].live_scopes.clone();
            slot_liveness[slot as usize].extend(live_scopes);
        }

        let children: Vec<usize> = self.scopes[scope_id].children.clone();
        for child_id in children {
            self.assign_slots_dfs(child_id, slot_liveness, total_slots);
        }
    }
}

impl Analyzer {
    fn propagate_globals(&mut self) {
        // reverse order so children are processed before parents
        for scope_id in (0..self.scopes.len()).rev() {
            let children: Vec<usize> = self.scopes[scope_id].children.clone();
            for child_id in children {
                let child_globals: Vec<String> =
                    self.scopes[child_id].global_refs.iter().cloned().collect();
                self.scopes[scope_id].global_refs.extend(child_globals);
            }
        }
    }

    fn generate_names(
        &self,
        slot_count: usize,
        keywords: &[&'static str],
        _true_globals: &HashSet<String>,
        slot_liveness: &[HashSet<usize>],
    ) -> Vec<String> {
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

        let mut slot_forbidden: Vec<HashSet<&str>> = Vec::with_capacity(slot_count);
        for slot_idx in 0..slot_count {
            let mut forbidden = HashSet::new();
            if slot_idx < slot_liveness.len() {
                for &scope_id in &slot_liveness[slot_idx] {
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

        let keyword_set: HashSet<&str> = keywords.iter().copied().collect();
        let name_gen = NameGenerator::new(keywords);
        let mut slot_to_name = vec![String::new(); slot_count];
        // Every slot scans candidates from index 0, so the generated names
        // (and their keyword-ness) are computed once and shared.
        let mut candidates: Vec<String> = Vec::new();
        let mut is_keyword: Vec<bool> = Vec::new();
        // Per candidate: bitset over scope ids where the name is already
        // worn by some slot. "Live scope has the bit set" is the same
        // conflict predicate as the pairwise liveness intersection, but
        // the hot reject path costs one bit test instead of hashing.
        let mut taken_scopes: Vec<Vec<u64>> = Vec::new();
        let scope_words = self.scopes.len().div_ceil(64);

        for &(slot, _) in &active_slots {
            let forbidden = &slot_forbidden[slot as usize];
            let slot_live = &slot_liveness[slot as usize];
            let mut idx = 0;
            loop {
                if idx == candidates.len() {
                    let name = name_gen.index_to_name(idx);
                    is_keyword.push(keyword_set.contains(name.as_str()));
                    candidates.push(name);
                    taken_scopes.push(vec![0u64; scope_words]);
                }
                let candidate_idx = idx;
                idx += 1;
                // non-overlapping slots can share the same name
                let conflicts = slot_live.iter().any(|&scope| {
                    taken_scopes[candidate_idx][scope / 64] & (1u64 << (scope % 64)) != 0
                });
                if conflicts || is_keyword[candidate_idx] {
                    continue;
                }
                let candidate = &candidates[candidate_idx];
                if forbidden.contains(candidate.as_str()) {
                    continue;
                }
                for &scope in slot_live {
                    taken_scopes[candidate_idx][scope / 64] |= 1u64 << (scope % 64);
                }
                slot_to_name[slot as usize] = candidate.clone();
                break;
            }
        }

        // Zero-frequency slots (declared but never referenced) still wear
        // a name in the output, so they run through the same liveness
        // conflict check: handing them the first short name regardless
        // captured co-live slots (an unreferenced param stole the name of
        // an upvalue used inside the same function body).
        for slot in 0..slot_count {
            if !slot_to_name[slot].is_empty() {
                continue;
            }
            let forbidden = &slot_forbidden[slot];
            let slot_live = &slot_liveness[slot];
            let mut idx = 0;
            loop {
                if idx == candidates.len() {
                    let name = name_gen.index_to_name(idx);
                    is_keyword.push(keyword_set.contains(name.as_str()));
                    candidates.push(name);
                    taken_scopes.push(vec![0u64; scope_words]);
                }
                let candidate_idx = idx;
                idx += 1;
                let conflicts = slot_live.iter().any(|&scope| {
                    taken_scopes[candidate_idx][scope / 64] & (1u64 << (scope % 64)) != 0
                });
                if conflicts || is_keyword[candidate_idx] {
                    continue;
                }
                let candidate = &candidates[candidate_idx];
                if forbidden.contains(candidate.as_str()) {
                    continue;
                }
                for &scope in slot_live {
                    taken_scopes[candidate_idx][scope / 64] |= 1u64 << (scope % 64);
                }
                slot_to_name[slot] = candidate.clone();
                break;
            }
        }

        slot_to_name
    }
}

struct AstRenamer {
    binding_new_names: Vec<String>,
    next_binding_id: usize,
    name_stack: HashMap<String, Vec<String>>,
    scope_binding_names: Vec<Vec<String>>,

    func_globals: HashSet<String>,
    assign_globals: HashSet<String>,
}

impl AstRenamer {
    fn new(
        binding_new_names: Vec<String>,
        _true_globals: &HashSet<String>,
        func_globals: &HashSet<String>,
        assign_globals: &HashSet<String>,
    ) -> Self {
        Self {
            binding_new_names,
            next_binding_id: 0,
            name_stack: HashMap::new(),
            scope_binding_names: vec![Vec::new()],
            func_globals: func_globals.clone(),
            assign_globals: assign_globals.clone(),
        }
    }

    fn declare_binding(&mut self, original: &str) -> String {
        let new_name = self.binding_new_names[self.next_binding_id].clone();
        self.next_binding_id += 1;
        self.name_stack
            .entry(original.to_string())
            .or_default()
            .push(new_name.clone());
        if let Some(scope_names) = self.scope_binding_names.last_mut() {
            scope_names.push(original.to_string());
        }
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

    fn resolve_name(&self, original: &str) -> String {
        if let Some(stack) = self.name_stack.get(original) {
            if let Some(new_name) = stack.last() {
                return new_name.clone();
            }
        }
        original.to_string()
    }

    // must match Analyzer::pre_declare_root_bindings exactly
    fn pre_declare_root_block(&mut self, block: &Block) {
        for stmt in &block.stmts {
            if let Statement::FunctionDecl(func_decl) = stmt
                && func_decl.name.names.len() == 1
                && func_decl.name.method.is_none()
            {
                let name = ident_name_string(&func_decl.name.names[0]);
                if self.func_globals.contains(&name) {
                    self.declare_binding(&name);
                }
            }
        }
        for stmt in &block.stmts {
            if let Statement::Assignment(assign) = stmt {
                for var in assign.targets.iter() {
                    if let Var::Name(name_tok) = var {
                        let name = ident_name_string(name_tok);
                        if self.assign_globals.contains(&name)
                            && !self.name_stack.contains_key(&name)
                        {
                            self.declare_binding(&name);
                        }
                    }
                }
            }
        }
    }
}

impl AstTransform for AstRenamer {
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
                let original = ident_name_string(&local_func.name);
                let new_name = self.declare_binding(&original);
                local_func.name = make_ident(&new_name);
                local_func.body = self.walk_function_body(local_func.body);
                Statement::LocalFunction(local_func)
            }
            Statement::FunctionDecl(mut func_decl) => {
                func_decl.body = self.walk_function_body(func_decl.body);
                if !func_decl.name.names.is_empty() {
                    let original = ident_name_string(&func_decl.name.names[0]);
                    let resolved = self.resolve_name(&original);
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
                let original = ident_name_string(&numeric_for.name);
                let new_name = self.declare_binding(&original);
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
                let original = ident_name_string(&name);
                let resolved = self.resolve_name(&original);
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
                let original = ident_name_string(&param.name);
                let new_name = self.declare_binding(&original);
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
            Expression::Var(var) => match *var {
                Var::Name(name) => {
                    let original = ident_name_string(&name);
                    let resolved = self.resolve_name(&original);
                    Expression::Var(Box::new(Var::Name(make_ident(&resolved))))
                }
                other => {
                    let var = self.transform_var(other);
                    Expression::Var(Box::new(var))
                }
            },
            // Any non-Var callee (call chains, parens, etc.) resolves its names
            // through the normal expression transform.
            callee @ (Expression::Nil(_)
            | Expression::False(_)
            | Expression::True(_)
            | Expression::Number(_)
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
            | Expression::Error(_)) => self.transform_expression(callee),
        };
        call.args = self.walk_function_args(call.args);
        call
    }
}

/// Rename every declared local while keeping its `<attrib>` attached.
fn rename_attributed_names(
    declare: &mut dyn FnMut(&str) -> String,
    names: Punctuated<AttributedName>,
) -> Punctuated<AttributedName> {
    let rename = |attributed: AttributedName| {
        let original = ident_name_string(&attributed.name);
        let new_name = declare(&original);
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
    declare: &mut dyn FnMut(&str) -> String,
    mut names: Punctuated<luck_ast::Parameter>,
) -> Punctuated<luck_ast::Parameter> {
    names.items = names
        .items
        .into_iter()
        .map(|mut binding| {
            let original = ident_name_string(&binding.name);
            let new_name = declare(&original);
            // Renaming a loop binding never changes its declared type
            binding.name = make_ident(&new_name);
            binding
        })
        .collect();
    names
}

fn collect_toplevel_function_globals(block: &Block) -> HashSet<String> {
    let mut top_locals = HashSet::new();
    for stmt in &block.stmts {
        match stmt {
            Statement::LocalAssignment(local) => {
                for attributed in local.names.iter() {
                    top_locals.insert(ident_name_string(&attributed.name));
                }
            }
            Statement::LocalFunction(local_func) => {
                top_locals.insert(ident_name_string(&local_func.name));
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

    let mut func_globals = HashSet::new();
    for stmt in &block.stmts {
        if let Statement::FunctionDecl(func_decl) = stmt
            && func_decl.name.names.len() == 1
            && func_decl.name.method.is_none()
        {
            let name = ident_name_string(&func_decl.name.names[0]);
            if !top_locals.contains(&name) {
                func_globals.insert(name);
            }
        }
    }

    func_globals
}

fn collect_assignfirst_globals(block: &Block, func_globals: &HashSet<String>) -> HashSet<String> {
    let mut read_globals: HashSet<String> = HashSet::new();
    let mut assign_first: HashSet<String> = HashSet::new();

    let mut all_locals = HashSet::new();
    collect_top_level_locals(block, &mut all_locals);

    for stmt in &block.stmts {
        match stmt {
            Statement::Assignment(assign) => {
                for expr in assign.values.iter() {
                    collect_name_reads_from_expr(expr, &all_locals, &mut read_globals);
                }
                for var in assign.targets.iter() {
                    if let Var::Name(name) = var {
                        let var_name = ident_name_string(name);
                        if !all_locals.contains(&var_name)
                            && !func_globals.contains(&var_name)
                            && !read_globals.contains(&var_name)
                            && !assign_first.contains(&var_name)
                        {
                            assign_first.insert(var_name);
                        }
                    }
                    if let Var::FieldAccess(fa) = var
                        && let Expression::Var(inner) = &fa.prefix
                        && let Var::Name(name) = inner.as_ref()
                    {
                        let var_name = ident_name_string(name);
                        if !all_locals.contains(&var_name) {
                            read_globals.insert(var_name);
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

fn collect_top_level_locals(block: &Block, locals: &mut HashSet<String>) {
    for stmt in &block.stmts {
        match stmt {
            Statement::LocalAssignment(local) => {
                for attributed in local.names.iter() {
                    locals.insert(ident_name_string(&attributed.name));
                }
            }
            Statement::LocalFunction(local_func) => {
                locals.insert(ident_name_string(&local_func.name));
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
                locals.insert(ident_name_string(&nf.name));
                collect_top_level_locals(&nf.block, locals);
            }
            Statement::GenericFor(gf) => {
                for binding in gf.names.iter() {
                    locals.insert(ident_name_string(&binding.name));
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
    locals: &HashSet<String>,
    reads: &mut HashSet<String>,
) {
    match expr {
        Expression::Var(var) => match var.as_ref() {
            Var::Name(name) => {
                let var_name = ident_name_string(name);
                if !locals.contains(&var_name) {
                    reads.insert(var_name);
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
        // A function body's reads belong to its own scope, not this one.
        Expression::FunctionDef(_) => {}
        // Literal leaves: no sub-expressions, no name reads.
        Expression::Nil(_)
        | Expression::False(_)
        | Expression::True(_)
        | Expression::Number(_)
        | Expression::StringLiteral(_)
        | Expression::VarArg(_)
        | Expression::Error(_) => {}
    }
}

fn collect_name_reads_from_func_args(
    args: &FunctionArgs,
    locals: &HashSet<String>,
    reads: &mut HashSet<String>,
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
    locals: &HashSet<String>,
    reads: &mut HashSet<String>,
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
                        let var_name = ident_name_string(name);
                        if !locals.contains(&var_name) {
                            reads.insert(var_name);
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
                    let var_name = ident_name_string(name);
                    if !locals.contains(&var_name) {
                        reads.insert(var_name);
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
