use core::mem;

use luck_ast::{
    Assignment, AttributedName, Block, Expression, FunctionBody, FunctionDef, GenericFor,
    IfStatement, LocalAssignment, LocalAssignmentKind, LocalFunction, LocalFunctionKind,
    NumericFor, Punctuated, RepeatLoop, Statement, Transform, Type, Var,
};
use luck_token::{CompactString, LuaVersion, MAX_ACTIVE_LOCALS, Span};
use rustc_hash::FxHashSet;

use crate::binding::{has_fixed_binding, identifier_name, identifier_token};
use crate::limits::{
    GENERIC_FOR_STATE_LOCALS, MAX_REGISTERS, NUMERIC_FOR_STATE_LOCALS, parameter_locals,
};

/// Runs after rename, which gives same-named bindings non-overlapping
/// lifetimes. Slot reuse can still give a child local its parent's name, so a
/// name already bound in an enclosing scope is left alone.
///
/// There's deliberately no per-lift size check, even though a lone lift can
/// cost a few bytes for the hoisted declaration. Each lift reshapes scopes,
/// the next rename redistributes names, and that unblocks further lifts.
/// Skipping the unprofitable lone lifts stops that cascade early, and it
/// benchmarked hundreds of bytes worse across the corpus than lifting
/// everything.
pub(crate) fn lift(block: Block, version: LuaVersion) -> Block {
    let mut lifter =
        Lifter { version, frames: Vec::new(), scans: Vec::new(), hidden_scan_count: 0 };
    lifter.transform_block(block)
}

/// Scans every function body the walk is in, and lifts each as it leaves.
///
/// By then the body's scan has seen all of it, its closures included. A
/// closure is lifted before the body around it, whose own lift reads nothing
/// that changes.
struct Lifter {
    version: LuaVersion,
    /// One name set per live scope, innermost last.
    frames: Vec<FxHashSet<CompactString>>,
    /// The scan of each function body the walk is in, innermost last.
    scans: Vec<BodyScan>,
    /// How many of `scans`, outermost first, are of bodies around a type the
    /// walk is in. Types never run, so those bodies capture no name in one
    /// and count no expression of it, and a function in a `typeof` is no
    /// closure of theirs.
    hidden_scan_count: usize,
}

/// What a body's walk learns for its lift: the names a declaration hoisted
/// to the top of the body must never wear, and the body's peak locals and
/// registers, which bound what that declaration can add.
///
/// The names are found by resolving scopes over the body:
///
/// - names free in the body (globals and upvalues), since the hoisted `local`
///   would capture every reference to them;
/// - names bound in the body but captured by a nested closure, since lifting
///   merges same-named bindings into one function-scope local that the
///   closure would then share;
/// - `_ENV` on targets where it's an upvalue.
///
/// A name a closure binds itself resolves inside that closure, so merely
/// mentioning a name in a closure doesn't block its lift.
struct BodyScan {
    /// The body's parameter frame in `Lifter::frames`, where its scopes
    /// start.
    first_frame: usize,
    ineligible_names: FxHashSet<CompactString>,
    /// Locals the walk has active in the body. A closure's don't count: it
    /// has its own 200.
    active_locals: usize,
    peak_active_locals: usize,
    /// Expressions seen in the body's current statement, each at most one
    /// register beyond the locals.
    statement_expr_nodes: usize,
    peak_registers: usize,
}

impl Lifter {
    /// The scan of the body the walk is directly in, unless a type hides it.
    fn scan(&mut self) -> Option<&mut BodyScan> {
        if self.scans.len() > self.hidden_scan_count { self.scans.last_mut() } else { None }
    }

    fn declare(&mut self, name: &str) {
        if self.scan().is_none() {
            return;
        }
        if let Some(frame) = self.frames.last_mut() {
            frame.insert(name.into());
        }
        self.add_active_locals(1);
    }

    fn add_active_locals(&mut self, count: usize) {
        if let Some(scan) = self.scan() {
            scan.active_locals = scan.active_locals.saturating_add(count);
            scan.peak_active_locals = scan.peak_active_locals.max(scan.active_locals);
        }
    }

    fn in_new_frame<Walked>(&mut self, walk: impl FnOnce(&mut Self) -> Walked) -> Walked {
        let outer_active_locals = self.scan().map(|scan| scan.active_locals);
        self.frames.push(FxHashSet::default());
        let walked = walk(self);
        self.frames.pop();
        if let Some(scan) = self.scan()
            && let Some(active_locals) = outer_active_locals
        {
            scan.active_locals = active_locals;
        }
        walked
    }

    /// Resolves `name` once for every body that can see the read: the one
    /// that binds it, and each closure between that body and the read.
    fn reference(&mut self, name: &str) {
        let scans = self.scans.get_mut(self.hidden_scan_count..).unwrap_or_default();
        let Some(outermost) = scans.first() else {
            return;
        };
        let first_frame = outermost.first_frame;
        let binding_frame = self
            .frames
            .get(first_frame..)
            .unwrap_or_default()
            .iter()
            .rposition(|frame| frame.contains(name))
            .map(|frame| frame.saturating_add(first_frame));
        let mut is_read_in_closure = false;
        for scan in scans.iter_mut().rev() {
            let is_bound_here = binding_frame.is_some_and(|frame| frame >= scan.first_frame);
            // Free in this body, or bound in it and captured.
            if !is_bound_here || is_read_in_closure {
                scan.ineligible_names.insert(name.into());
            }
            if is_bound_here {
                break;
            }
            is_read_in_closure = true;
        }
    }

    fn scan_repeat_loop(&mut self, mut repeat_loop: Box<RepeatLoop>) -> Statement {
        // The condition sees the block's locals.
        self.in_new_frame(|lifter| {
            repeat_loop.block = lifter.walk_block(repeat_loop.block.take());
            repeat_loop.condition = lifter.transform_expr(repeat_loop.condition.take());
        });
        Statement::RepeatLoop(repeat_loop)
    }

    // A body's scan opens and closes in functions of their own, so its
    // temporaries stay off the stack under the functions nested in it.
    #[inline(never)]
    fn open_body(&mut self, body: &FunctionBody) {
        let parameter_count = parameter_locals(body);
        self.scans.push(BodyScan {
            first_frame: self.frames.len(),
            ineligible_names: if self.version.has_env_upvalue() {
                FxHashSet::from_iter([CompactString::from("_ENV")])
            } else {
                FxHashSet::default()
            },
            active_locals: parameter_count,
            peak_active_locals: parameter_count,
            statement_expr_nodes: 0,
            peak_registers: parameter_count,
        });
        self.frames.push(param_names(body));
    }

    #[inline(never)]
    fn close_body(&mut self, body: &mut FunctionBody) {
        self.frames.pop();
        if let Some(scan) = self.scans.pop() {
            lift_body(body, &scan, self.version);
        }
    }

    fn scan_numeric_for(&mut self, mut numeric_for: Box<NumericFor>) -> Statement {
        if let Some(loop_variable_type) = &mut numeric_for.type_annotation {
            *loop_variable_type = self.transform_type(loop_variable_type.take());
        }
        numeric_for.start = self.transform_expr(numeric_for.start.take());
        numeric_for.limit = self.transform_expr(numeric_for.limit.take());
        if let Some(step) = &mut numeric_for.step {
            *step = self.transform_expr(step.take());
        }
        self.in_new_frame(|lifter| {
            lifter.add_active_locals(NUMERIC_FOR_STATE_LOCALS);
            lifter.declare(identifier_name(&numeric_for.name));
            numeric_for.block = lifter.walk_block(numeric_for.block.take());
        });
        Statement::NumericFor(numeric_for)
    }

    fn scan_generic_for(&mut self, mut generic_for: Box<GenericFor>) -> Statement {
        let GenericFor { names, exprs, block, .. } = &mut *generic_for;
        for param in &mut names.items {
            if let Some(param_type) = &mut param.type_annotation {
                *param_type = self.transform_type(param_type.take());
            }
        }
        for expr in &mut exprs.items {
            *expr = self.transform_expr(expr.take());
        }
        self.in_new_frame(|lifter| {
            lifter.add_active_locals(GENERIC_FOR_STATE_LOCALS);
            for param in names.iter() {
                lifter.declare(identifier_name(&param.name));
            }
            *block = lifter.walk_block(block.take());
        });
        Statement::GenericFor(generic_for)
    }
}

impl Transform for Lifter {
    fn transform_block(&mut self, block: Block) -> Block {
        self.in_new_frame(|lifter| lifter.walk_block(block))
    }

    // Each loop is scanned in a function of its own, so a level of nesting
    // holds only that loop's temporaries on the stack.
    fn transform_stmt(&mut self, stmt: Statement) -> Statement {
        if let Some(scan) = self.scan() {
            scan.statement_expr_nodes = 0;
        }
        match stmt {
            Statement::LocalAssignment(_) => {
                // RHS before LHS: `local x = x` reads the outer x.
                let walked = self.walk_stmt(stmt);
                if let Statement::LocalAssignment(local) = &walked {
                    for attributed in local.names.iter() {
                        self.declare(identifier_name(&attributed.name));
                    }
                }
                walked
            }
            Statement::LocalFunction(local_function) => {
                self.declare(identifier_name(&local_function.name));
                self.walk_stmt(Statement::LocalFunction(local_function))
            }
            Statement::FunctionDecl(_) => {
                let walked = self.walk_stmt(stmt);
                if let Statement::FunctionDecl(function_decl) = &walked
                    && let Some(root_name) = function_decl.name.names.first()
                {
                    self.reference(identifier_name(root_name));
                }
                walked
            }
            Statement::RepeatLoop(repeat_loop) => self.scan_repeat_loop(repeat_loop),
            Statement::NumericFor(numeric_for) => self.scan_numeric_for(numeric_for),
            Statement::GenericFor(generic_for) => self.scan_generic_for(generic_for),
            // These bind nothing themselves, and a block of theirs opens its
            // scope in `transform_block`. A `global function` or `type
            // function` body is reached as a closure.
            other @ (Statement::Assignment(_)
            | Statement::FunctionCall(_)
            | Statement::DoBlock(_)
            | Statement::WhileLoop(_)
            | Statement::IfStatement(_)
            | Statement::CompoundAssignment(_)
            | Statement::GlobalFunction(_)
            | Statement::GlobalDeclaration(_)
            | Statement::TypeDeclaration(_)
            | Statement::EmptyStatement(_)
            | Statement::Goto(_)
            | Statement::Label(_)
            | Statement::GlobalStar(_)
            | Statement::Break(_)
            | Statement::Error(_)) => self.walk_stmt(other),
        }
    }

    fn transform_expr(&mut self, expr: Expression) -> Expression {
        if let Some(scan) = self.scan() {
            scan.statement_expr_nodes = scan.statement_expr_nodes.saturating_add(1);
            let registers = scan.active_locals.saturating_add(scan.statement_expr_nodes);
            scan.peak_registers = scan.peak_registers.max(registers);
        }
        if let Expression::Name(name) = &expr {
            self.reference(identifier_name(name));
        }
        self.walk_expr(expr)
    }

    fn transform_assignment_target(&mut self, target: Var) -> Var {
        if let Var::Name(name) = &target {
            self.reference(identifier_name(name));
        }
        self.walk_assignment_target(target)
    }

    /// The bodies the walk is already in see this one's bindings as a
    /// closure's.
    fn transform_function_body(&mut self, mut body: FunctionBody) -> FunctionBody {
        self.open_body(&body);
        body.block = self.walk_block(body.block);
        self.close_body(&mut body);
        body
    }

    fn transform_type(&mut self, type_value: Type) -> Type {
        let outer_hidden_scan_count = mem::replace(&mut self.hidden_scan_count, self.scans.len());
        let walked = self.walk_type(type_value);
        self.hidden_scan_count = outer_hidden_scan_count;
        walked
    }
}

/// Hoists the declarations nested in `body` that its finished scan allows.
fn lift_body(body: &mut FunctionBody, scan: &BodyScan, version: LuaVersion) {
    let mut collector = LiftableCollector {
        scope_names: vec![param_names(body)],
        ineligible_names: &scan.ineligible_names,
        liftable_names: FxHashSet::default(),
        ordered_liftable_names: Vec::new(),
        // Each hoisted name adds at most one local and one register
        // anywhere in the body, so the body's own peaks bound what fits.
        hoist_capacity: MAX_ACTIVE_LOCALS
            .saturating_sub(scan.peak_active_locals)
            .min(MAX_REGISTERS.saturating_sub(scan.peak_registers)),
        version,
    };
    collector.collect_block(&body.block);
    let LiftableCollector { liftable_names, ordered_liftable_names, .. } = collector;
    if liftable_names.is_empty() {
        return;
    }

    let mut rewriter =
        LiftRewriter { liftable_names: &liftable_names, scope_names: Vec::new(), version };
    body.block = rewriter.rewrite_block_in_frame(body.block.take(), FxHashSet::default());
    prepend_declaration(&mut body.block, &ordered_liftable_names);
}

/// A name qualifies when it isn't already bound in this or an enclosing scope
/// of the body (rename can give a child local its parent's name) and isn't
/// ineligible (see [`BodyScan`]).
///
/// A `local` statement also has to pass `can_lift_assignment`, which rules out attributes such as
/// `<const>` and `<close>`.
struct LiftableCollector<'names> {
    scope_names: Vec<FxHashSet<CompactString>>,
    ineligible_names: &'names FxHashSet<CompactString>,
    liftable_names: FxHashSet<CompactString>,
    ordered_liftable_names: Vec<CompactString>,
    hoist_capacity: usize,
    version: LuaVersion,
}

impl LiftableCollector<'_> {
    const fn has_hoist_room(&self, name_count: usize) -> bool {
        self.ordered_liftable_names.len().saturating_add(name_count) <= self.hoist_capacity
    }

    fn declare(&mut self, name: &str) {
        if let Some(frame) = self.scope_names.last_mut() {
            frame.insert(name.into());
        }
    }

    fn is_safe_to_lift(&self, name: &str) -> bool {
        !self.scope_names.iter().any(|frame| frame.contains(name))
            && !self.ineligible_names.contains(name)
    }

    fn mark_liftable(&mut self, name: &str) {
        if self.liftable_names.insert(name.into()) {
            self.ordered_liftable_names.push(name.into());
        }
    }

    fn collect_block(&mut self, block: &Block) {
        for stmt in &block.stmts {
            self.collect_stmt(stmt);
        }
    }

    fn collect_block_in_frame(&mut self, block: &Block, frame: FxHashSet<CompactString>) {
        self.scope_names.push(frame);
        self.collect_block(block);
        self.scope_names.pop();
    }

    fn collect_scoped_block(&mut self, block: &Block) {
        self.collect_block_in_frame(block, FxHashSet::default());
    }

    fn collect_local(&mut self, local: &LocalAssignment) {
        let names = local.names.items.iter().map(|attributed| identifier_name(&attributed.name));
        if can_lift_assignment(local, self.version)
            && self.has_hoist_room(local.names.len())
            && names.clone().all(|name| self.is_safe_to_lift(name))
        {
            for name in names.clone() {
                self.mark_liftable(name);
            }
        }
        for name in names {
            self.declare(name);
        }
    }

    fn collect_stmt(&mut self, stmt: &Statement) {
        match stmt {
            Statement::LocalAssignment(local) => self.collect_local(local),
            Statement::LocalFunction(local_function) => {
                let name = identifier_name(&local_function.name);
                if local_function.kind == LocalFunctionKind::Local
                    && self.has_hoist_room(1)
                    && self.is_safe_to_lift(name)
                {
                    self.mark_liftable(name);
                }
                self.declare(name);
                // The function body gets its own lift pass, so stop here.
            }
            Statement::DoBlock(do_block) => self.collect_scoped_block(&do_block.block),
            Statement::WhileLoop(while_loop) => self.collect_scoped_block(&while_loop.block),
            Statement::RepeatLoop(repeat_loop) => self.collect_scoped_block(&repeat_loop.block),
            Statement::IfStatement(if_stmt) => {
                self.collect_scoped_block(&if_stmt.block);
                for clause in &if_stmt.elseif_clauses {
                    self.collect_scoped_block(&clause.block);
                }
                if let Some(else_clause) = &if_stmt.else_clause {
                    self.collect_scoped_block(&else_clause.block);
                }
            }
            Statement::NumericFor(numeric_for) => {
                let frame = FxHashSet::from_iter([identifier_name(&numeric_for.name).into()]);
                self.collect_block_in_frame(&numeric_for.block, frame);
            }
            Statement::GenericFor(generic_for) => {
                let frame = generic_for
                    .names
                    .iter()
                    .map(|param| identifier_name(&param.name).into())
                    .collect();
                self.collect_block_in_frame(&generic_for.block, frame);
            }
            // Nothing to lift at this level. Function bodies get their own
            // pass through `transform_function_body`.
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

fn can_lift_assignment(local: &LocalAssignment, version: LuaVersion) -> bool {
    // A bare `local x` resets x to nil, which a lifted one wouldn't once a
    // sibling has used the shared slot. A type annotation would be lost,
    // since the hoisted declaration is a bare `local x`.
    local.exprs.is_some()
        && !has_fixed_binding(local, version)
        && local.names.iter().all(|attributed| attributed.type_annotation.is_none())
        && has_distinct_names(&local.names.items)
}

/// Past [`MAX_ACTIVE_LOCALS`] names, more than one function holds, none count
/// as distinct.
fn has_distinct_names(names: &[AttributedName]) -> bool {
    names.len() <= MAX_ACTIVE_LOCALS
        && names.iter().enumerate().all(|(i, attributed)| {
            names
                .iter()
                .take(i)
                .all(|earlier| identifier_name(&earlier.name) != identifier_name(&attributed.name))
        })
}

/// A Lua 5.5 `...name` vararg is one too, and a hoisted declaration sharing its name would
/// capture its references.
fn param_names(body: &FunctionBody) -> FxHashSet<CompactString> {
    let mut names: FxHashSet<CompactString> =
        body.params.iter().map(|param| identifier_name(&param.name).into()).collect();
    if let Some(vararg) = &body.vararg
        && let Some(name) = &vararg.name
    {
        names.insert(identifier_name(name).into());
    }
    names
}

/// Matching is by name, so it tracks every declaration in scope, lifted or
/// not. A declaration under a same-named binding stays `local`, since its
/// assignment would write to that binding instead.
struct LiftRewriter<'names> {
    liftable_names: &'names FxHashSet<CompactString>,
    scope_names: Vec<FxHashSet<CompactString>>,
    version: LuaVersion,
}

impl LiftRewriter<'_> {
    fn is_shadowed(&self, name: &str) -> bool {
        self.scope_names.iter().any(|frame| frame.contains(name))
    }

    fn rewrite_block_in_frame(
        &mut self,
        mut block: Block,
        frame: FxHashSet<CompactString>,
    ) -> Block {
        self.scope_names.push(frame);
        for stmt in &mut block.stmts {
            *stmt = self.rewrite_stmt(stmt.take());
        }
        self.scope_names.pop();
        block
    }

    fn rewrite_local(&mut self, local: Box<LocalAssignment>) -> Statement {
        let names = local.names.items.iter().map(|attributed| identifier_name(&attributed.name));
        let is_lifted = can_lift_assignment(&local, self.version)
            && names.clone().all(|name| self.liftable_names.contains(name))
            && !names.clone().any(|name| self.is_shadowed(name));

        if let Some(frame) = self.scope_names.last_mut() {
            frame.extend(names.map(CompactString::from));
        }
        // local a,b=X,Y -> a,b=X,Y. `can_lift_assignment` has already
        // rejected bare declarations, so `exprs` is always set here.
        if is_lifted && let Some(values) = local.exprs {
            let targets = Punctuated::from_items(
                local
                    .names
                    .items
                    .into_iter()
                    .map(|attributed| Var::Name(attributed.name))
                    .collect(),
            );
            return Statement::Assignment(Box::new(Assignment {
                span: local.span,
                targets,
                values,
            }));
        }
        Statement::LocalAssignment(local)
    }

    fn rewrite_local_function(&mut self, local_function: Box<LocalFunction>) -> Statement {
        let name = identifier_name(&local_function.name);
        let is_lifted = local_function.kind == LocalFunctionKind::Local
            && self.liftable_names.contains(name)
            && !self.is_shadowed(name);
        if let Some(frame) = self.scope_names.last_mut() {
            frame.insert(name.into());
        }
        if !is_lifted {
            return Statement::LocalFunction(local_function);
        }
        // local function f(x)...end -> f=function(x)...end
        let function_expr = Expression::FunctionDef(Box::new(FunctionDef {
            span: local_function.span,
            attributes: local_function.attributes,
            body: local_function.body,
        }));
        Statement::Assignment(Box::new(Assignment {
            span: local_function.span,
            targets: Punctuated::from_item(Var::Name(local_function.name)),
            values: Punctuated::from_item(function_expr),
        }))
    }

    /// Rewrites a nested block in place, in a frame of the names its
    /// statement binds.
    fn rewrite_nested(&mut self, block: &mut Block, frame: FxHashSet<CompactString>) {
        *block = self.rewrite_block_in_frame(block.take(), frame);
    }

    fn rewrite_if_blocks(&mut self, if_stmt: &mut IfStatement) {
        self.rewrite_nested(&mut if_stmt.block, FxHashSet::default());
        for clause in &mut if_stmt.elseif_clauses {
            self.rewrite_nested(&mut clause.block, FxHashSet::default());
        }
        if let Some(else_clause) = &mut if_stmt.else_clause {
            self.rewrite_nested(&mut else_clause.block, FxHashSet::default());
        }
    }

    // Each arm rewrites its block through `rewrite_nested`, so a level of
    // nesting holds only that call's temporaries on the stack.
    fn rewrite_stmt(&mut self, stmt: Statement) -> Statement {
        match stmt {
            Statement::LocalAssignment(local) => self.rewrite_local(local),
            Statement::LocalFunction(local_function) => self.rewrite_local_function(local_function),
            Statement::DoBlock(mut do_block) => {
                self.rewrite_nested(&mut do_block.block, FxHashSet::default());
                Statement::DoBlock(do_block)
            }
            Statement::WhileLoop(mut while_loop) => {
                self.rewrite_nested(&mut while_loop.block, FxHashSet::default());
                Statement::WhileLoop(while_loop)
            }
            Statement::RepeatLoop(mut repeat_loop) => {
                self.rewrite_nested(&mut repeat_loop.block, FxHashSet::default());
                Statement::RepeatLoop(repeat_loop)
            }
            Statement::IfStatement(mut if_stmt) => {
                self.rewrite_if_blocks(&mut if_stmt);
                Statement::IfStatement(if_stmt)
            }
            Statement::NumericFor(mut numeric_for) => {
                let frame = FxHashSet::from_iter([identifier_name(&numeric_for.name).into()]);
                self.rewrite_nested(&mut numeric_for.block, frame);
                Statement::NumericFor(numeric_for)
            }
            Statement::GenericFor(mut generic_for) => {
                let frame = generic_for
                    .names
                    .iter()
                    .map(|param| identifier_name(&param.name).into())
                    .collect();
                self.rewrite_nested(&mut generic_for.block, frame);
                Statement::GenericFor(generic_for)
            }
            // Nothing to rewrite at this level. Function bodies get their own
            // pass through `transform_function_body`.
            other @ (Statement::Assignment(_)
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
            | Statement::Error(_)) => other,
        }
    }
}

fn prepend_declaration(block: &mut Block, names: &[CompactString]) {
    let attributed_names: Vec<AttributedName> = names
        .iter()
        .map(|name| AttributedName {
            name: identifier_token(name.clone()),
            type_annotation: None,
            attribute: None,
        })
        .collect();

    block.stmts.insert(
        0,
        Statement::LocalAssignment(Box::new(LocalAssignment {
            span: Span::default(),
            names: Punctuated::from_items(attributed_names),
            exprs: None,
            kind: LocalAssignmentKind::Local,
        })),
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixtures::{assert_reparses, run_pass};
    use crate::transforms::rename_locals::rename;

    fn apply(source: &str) -> String {
        let output = run_pass(source, LuaVersion::Lua54, |block| lift(block, LuaVersion::Lua54));
        assert_reparses(&output, LuaVersion::Lua54);
        output
    }

    fn rename_and_lift(source: &str) -> String {
        let output = run_pass(source, LuaVersion::Lua54, |block| {
            lift(rename(block, LuaVersion::Lua54, true), LuaVersion::Lua54)
        });
        assert_reparses(&output, LuaVersion::Lua54);
        output
    }

    #[test]
    fn lifts_nested_locals() {
        let result = apply(
            "\
local function f(x)
  local a = 1
  if x then
    local b = 2
    print(a, b)
  end
  return a
end
return f
",
        );
        assert!(
            result.contains("local a,b"),
            "both declarations should hoist to one function-head local: {result}"
        );
        assert!(
            !result.contains("local b"),
            "the nested declaration should lose its `local`: {result}"
        );
    }

    #[test]
    fn does_not_lift_parent_shadowed() {
        // When rename gives the inner `b` the outer `a`'s name, the inner
        // declaration must keep its `local`.
        let result = rename_and_lift(
            "\
local function f()
  local a = 1
  do
    local b = 2
    print(b)
  end
  return a
end
return f
",
        );
        assert!(
            result.contains("do local l=2"),
            "the shadowed inner declaration must keep its `local`: {result}"
        );
    }

    #[test]
    fn does_not_lift_closure_in_loop() {
        // Only function bodies get lifted, so the loop needs a wrapper.
        let result = apply(
            "\
local function f()
  local t = {}
  for i = 1, 10 do
    local x = i
    t[i] = function() return x end
  end
  return t
end
return f
",
        );
        assert!(
            result.contains("local x=i"),
            "closure-captured loop local must keep its per-iteration binding: {result}"
        );
        assert!(result.contains("local t t="), "the uncaptured local should still hoist: {result}");
    }

    #[test]
    fn lifts_loop_local_without_closure() {
        let result = apply(
            "\
local function f()
  local s = 0
  for i = 1, 10 do
    local x = i * 2
    s = s + x
  end
  return s
end
return f
",
        );
        assert!(
            result.contains("local s,x"),
            "an uncaptured loop local should hoist to the function head: {result}"
        );
        assert!(
            !result.contains("local x"),
            "the loop-body declaration should lose its `local`: {result}"
        );
    }

    #[test]
    fn lifts_a_multi_name_local_only_when_its_names_differ() {
        let result = apply(
            "\
local function f(c)
  if c then
    local a, b = 1, 2
    print(a, b)
  end
  if c then
    local d, d = 1, 2
    print(d)
  end
end
return f
",
        );
        assert!(result.contains("then a,b=1,2"), "distinct names should hoist: {result}");
        assert!(result.contains("local d,d=1,2"), "a repeated name should stay local: {result}");
    }

    #[test]
    fn does_not_lift_over_free_reference() {
        // `u[k]` reads the outer `u`, and the inner `local u` shadows it only
        // from its declaration on. Hoisting `local u` to the top of the body
        // would capture that read, which miscompiled roact's `Config:set`.
        let result = apply(
            "\
local u = {}
local function f(k)
  if u[k] == nil then
    local u = 1
    print(u)
  end
end
return f
",
        );
        assert!(
            result.contains("local u=1") || result.contains("local u = 1"),
            "decl sharing a free-referenced name must stay local: {result}"
        );
    }

    #[test]
    fn capture_outside_loop_still_blocks_lift() {
        let result = apply(
            "\
local function f()
  do
    local x = 1
    g = function() return x end
  end
  do
    local x = 2
    h = function() return x end
  end
end
return f
",
        );
        // Hoisting would merge both x bindings into one, letting the two
        // closures observe each other's writes.
        assert!(
            result.matches("local x").count() == 2,
            "captured locals must not merge into one hoisted slot: {result}"
        );
    }

    #[test]
    fn closure_internal_names_do_not_block_lift() {
        // The closure binds its own `x`; that must not veto lifting the
        // unrelated do-block `x`.
        let result = apply(
            "\
local function f()
  local c = function() local x = 1 return x end
  do
    local x = 2
    print(x)
  end
  return c
end
return f
",
        );
        assert!(
            !result.contains("local x=2") && !result.contains("local x = 2"),
            "closure-internal name must not block the lift: {result}"
        );
    }

    #[test]
    fn named_vararg_blocks_same_named_lift() {
        let source = "\
local function f(...args)
  do
    local args = 1
    print(args)
  end
end
return f
";
        let output = run_pass(source, LuaVersion::Lua55, |block| lift(block, LuaVersion::Lua54));
        assert!(
            output.contains("local args=1") || output.contains("local args = 1"),
            "decl sharing the vararg name must stay local: {output}"
        );
    }

    #[test]
    fn shadowed_declaration_is_never_rewritten_to_assignment() {
        // The rewrite matches by name, so a declaration under a same-named
        // binding (here the `for` variable) must stay `local`, or its
        // assignment would write to that binding.
        let result = rename_and_lift(
            "\
function W.new(c)
  for l = 1, 3 do
    if c then
      local l = f()
      print(l)
    end
  end
  while c do
    local l = g()
    print(l)
  end
end
",
        );
        // The declaration under the `for` variable must stay `local`. The
        // one in the while loop may be lifted.
        assert!(
            result.contains("local l=f()") || result.contains("local l = f()"),
            "shadowed decl must stay local: {result}"
        );
    }

    #[test]
    fn hoisted_declaration_leaves_registers_for_a_wide_statement() {
        let body = (0..190u32)
            .map(|i| format!("do local v{i} = g() print(v{i}) end\n"))
            .collect::<Vec<_>>()
            .concat();
        let args = (0..100u32).map(|i| i.to_string()).collect::<Vec<_>>().join(",");
        let result = apply(&format!(
            "\
local function w()
{body}f({args})
end
return w
"
        ));
        let hoisted = result.split("local ").nth(2).expect("hoisted declaration");
        let (names, _) = hoisted.split_once(' ').expect("names end at the next statement");
        let name_count = names.split(',').count();
        assert!(
            name_count + 101 <= 249,
            "{name_count} hoisted locals leave no room for the call's 101 registers"
        );
    }

    #[test]
    fn hoisted_declaration_stays_within_the_local_limit() {
        let body = (0..205u32)
            .map(|i| format!("do local v{i} = g() print(v{i}) end\n"))
            .collect::<Vec<_>>()
            .concat();
        let result = apply(&format!(
            "\
local function w()
{body}end
return w
"
        ));
        let hoisted = result.split("local ").nth(2).expect("hoisted declaration");
        let (names, _) = hoisted.split_once(' ').expect("names end at the next statement");
        let name_count = names.split(',').count();
        assert!(
            name_count <= MAX_ACTIVE_LOCALS,
            "a function may hold {MAX_ACTIVE_LOCALS} locals at once, but {name_count} were hoisted"
        );
    }

    fn apply_luau(source: &str) -> String {
        let output = run_pass(source, LuaVersion::Luau, |block| lift(block, LuaVersion::Luau));
        assert_reparses(&output, LuaVersion::Luau);
        output
    }

    #[test]
    fn a_read_two_closures_down_is_resolved_for_each_body_around_it() {
        let result = apply(
            "\
local function f()
  do
    local x = 1
    g = function()
      do
        local y = 2
        print(y)
      end
      return function()
        do
          local z = 3
          print(z)
        end
        return x
      end
    end
  end
end
return f
",
        );
        assert_eq!(
            result,
            "local function f()do local x=1 g=function()local y do y=2 print(y)end \
             return function()local z do z=3 print(z)end return x end end end end return f"
        );
    }

    #[test]
    fn lifts_a_function_in_a_typeof_wherever_the_walk_reaches_one() {
        let function = "function() do local b = g() print(b) end end";
        let lifted = "typeof(function()local b do b=g()print(b)end end)";
        for source in [
            format!("local v = 1 :: typeof({function})"),
            format!("local v: typeof({function}) = 1"),
            format!("type T = typeof({function})"),
            format!("type T<U = typeof({function})> = U"),
            format!("for i: typeof({function}) = 1, 2 do end"),
            format!("for k: typeof({function}) in p do end"),
            format!("f<<typeof({function})>>()"),
            format!("a:m<<typeof({function})>>()"),
            format!("local function f() local v = 1 :: typeof({function}) return v end"),
            format!("local function f(a) for i: typeof({function}) = a, 2 do end end"),
            format!("local function f(a) for k: typeof({function}) in a do end end"),
        ] {
            let result = apply_luau(&source);
            assert!(result.contains(lifted), "{source}\n{result}");
        }
        // The walk skips a signature, so a function in one keeps its `local`.
        for source in [
            format!("local function f(p: typeof({function})) end"),
            format!("local function f(): typeof({function}) end"),
        ] {
            let result = apply_luau(&source);
            assert!(result.contains("do local b=g()"), "{source}\n{result}");
        }
    }

    #[test]
    fn a_name_read_in_a_type_is_never_captured() {
        for (source, expected) in [
            (
                "\
local function f()
  do
    local x = 1
    local y = g() :: typeof(function() return x end)
    print(x, y)
  end
end
",
                "local function f()local x,y do x=1 y=g()::typeof(function()return x end)\
                 print(x,y)end end",
            ),
            (
                "\
local function f()
  do
    local x = 1
    print(x, function()
      local z = 1 :: typeof(function() return x end)
      return z
    end)
  end
end
",
                "local function f()local x do x=1 print(x,function()local z z=1::\
                 typeof(function()return x end)return z end)end end",
            ),
            (
                "\
local function f()
  do
    local x = 1
    print(x)
  end
  local y = g() :: typeof(function() x = 2 end)
  return y
end
",
                "local function f()local x,y do x=1 print(x)end y=g()::\
                 typeof(function()x=2 end)return y end",
            ),
        ] {
            assert_eq!(apply_luau(source), expected, "{source}");
        }
    }

    #[test]
    fn a_function_in_a_typeof_resolves_its_own_closures() {
        let result = apply_luau(
            "\
local function f()
  do
    local x = 1
    print(x :: typeof(function()
      do
        local n = 1
        h = function() return n end
      end
      do
        local m = 2
        print(m)
      end
    end))
  end
end
",
        );
        assert_eq!(
            result,
            "local function f()local x do x=1 print(x::typeof(function()local m \
             do local n=1 h=function()return n end end do m=2 print(m)end end))end end"
        );
    }

    #[test]
    fn a_type_function_captures_like_a_closure() {
        let result = apply_luau(
            "\
local function f()
  do
    local x = 1
    print(x)
  end
  type function tf(y)
    do
      local c = y
      print(c, x)
    end
    return y
  end
end
",
        );
        assert_eq!(
            result,
            "local function f()do local x=1 print(x)end type function tf(y)local c \
             do c=y print(c,x)end return y end end"
        );
    }
}
