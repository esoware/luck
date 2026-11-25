//! Merging neighboring `local`s, and neighboring assignments to proven
//! locals, into one statement.

use std::mem;

use luck_allocator::List;
use luck_ast::{
    Assignment, Expression, LocalAssignmentKind, Name, Statement, TypedName, Var, Visit,
};
use luck_semantic::{Scoping, SymbolId};
use luck_value::{MayHaveSideEffects as _, ValueContext};

use super::Compressor;
use crate::binding::has_fixed_binding;
use crate::limits::assignment_target_limit;

/// A context in which no name's read counts as an effect, since a group
/// evaluates its values in the order its statements did, so a global's
/// `__index` sees the same reads in the same order.
struct ReadsInOrder;

impl ValueContext for ReadsInOrder {
    fn is_global_reference(&self, _name: &Name<'_>) -> bool {
        false
    }
}

pub(super) fn is_bare_local(stmt: &Statement<'_>) -> bool {
    matches!(stmt, Statement::LocalAssignment(local) if local.exprs.is_empty() && !has_fixed_binding(local))
}

/// Whether `expr` evaluates into exactly one register with no temporaries.
const fn is_single_register_value(expr: &Expression<'_>) -> bool {
    match expr {
        Expression::Number(_)
        | Expression::Integer(_)
        | Expression::StringLiteral(_)
        | Expression::Nil(_)
        | Expression::True(_)
        | Expression::False(_)
        | Expression::VarArg(_)
        | Expression::FunctionDef(_) => true,
        Expression::Name(_)
        | Expression::Suffixed(_)
        | Expression::Parenthesized(_)
        | Expression::BinaryOp(_)
        | Expression::UnaryOp(_)
        | Expression::TableConstructor(_)
        | Expression::IfExpression(_)
        | Expression::InterpolatedString(_)
        | Expression::TypeCast(_)
        | Expression::Error(_) => false,
    }
}

/// Finds a reference to any of a group's bindings, closure bodies and
/// annotations included.
struct ReferenceFinder<'group> {
    group: &'group [SymbolId],
    scoping: &'group Scoping,
    has_reference: bool,
}

impl<'tree> Visit<'tree> for ReferenceFinder<'_> {
    fn visit_stmt(&mut self, stmt: &'tree Statement<'tree>) {
        if !self.has_reference {
            self.walk_stmt(stmt);
        }
    }

    fn visit_expr(&mut self, expr: &'tree Expression<'tree>) {
        if !self.has_reference {
            self.walk_expr(expr);
        }
    }

    fn visit_name(&mut self, name: &'tree Name<'tree>) {
        self.has_reference |= self
            .scoping
            .resolved_symbol_of(name)
            .is_some_and(|symbol| self.group.contains(&symbol));
    }
}

impl<'ast> Compressor<'_, 'ast> {
    /// `local a = 1 local b = 2` becomes `local a, b = 1, 2`, `a = 1 b = 2`
    /// becomes `a, b = 1, 2` where both are locals, `local a local b`
    /// becomes `local a, b`, and `local a, b` and then `a, b = X, Y` becomes
    /// `local a, b = X, Y`.
    pub(super) fn merge_stmts(&mut self, stmts: &mut List<'ast, Statement<'ast>>) {
        let outer_registers = self.live_registers;
        self.merge_groups(stmts);
        // A local a fuse initializes can merge with its neighbors.
        while self.fuse_bare_locals(stmts) {
            self.live_registers = outer_registers;
            self.merge_groups(stmts);
        }
    }

    fn merge_groups(&mut self, stmts: &mut List<'ast, Statement<'ast>>) {
        let mut kept = 0;
        let mut index = 0;
        let mut group = mem::take(&mut self.group);
        while index < stmts.len() {
            let group_end = self.merge_group_end(stmts, index, &mut group);
            if group_end > index + 1 {
                self.merge_group(stmts, index, group_end);
                self.has_changed = true;
            }
            stmts.swap(kept, index);
            self.live_registers += declared_count(&stmts[kept]);
            kept += 1;
            index = group_end;
        }
        self.group = group;
        stmts.truncate(kept);
    }

    const fn reference_finder<'group>(
        &'group self,
        group: &'group [SymbolId],
    ) -> ReferenceFinder<'group> {
        ReferenceFinder { group, scoping: self.scoping, has_reference: false }
    }

    /// The symbol of a lone `local name = value` whose value a group may
    /// evaluate beside its neighbors, the only kind of `local` an initialized
    /// group merges.
    fn single_local(&self, stmt: &Statement<'_>) -> Option<SymbolId> {
        let Statement::LocalAssignment(local) = stmt else {
            return None;
        };
        // Fixed bindings stay unmerged. Merging a `const` with plain locals,
        // for one, would extend or drop its const-ness.
        if has_fixed_binding(local) {
            return None;
        }
        if let ([local_name], [expr]) = (&*local.names, &*local.exprs)
            && !expr.may_have_side_effects(&ReadsInOrder)
        {
            return self.scoping.symbol_of(&local_name.name);
        }
        None
    }

    /// The end of the run of statements from `start` that merge into it.
    fn merge_group_end(
        &self,
        stmts: &[Statement<'ast>],
        start: usize,
        group: &mut Vec<SymbolId>,
    ) -> usize {
        let first = &stmts[start];
        let followers = &stmts[start + 1..];
        group.clear();
        let follower_count = if let Some(symbol) = self.single_local(first) {
            group.push(symbol);
            followers
                .iter()
                .take_while(|stmt| {
                    self.single_local(stmt).is_some_and(|follower| {
                        // An annotation binds before the name, as the value
                        // does.
                        let mut finder = self.reference_finder(group);
                        finder.visit_stmt(stmt);
                        let can_join = !finder.has_reference;
                        if can_join {
                            group.push(follower);
                        }
                        can_join
                    })
                })
                .count()
        } else if is_bare_local(first) {
            followers.iter().take_while(|stmt| is_bare_local(stmt)).count()
        } else if let Statement::Assignment(assignment) = first
            && let Some((symbol, _)) = self.single_assignment(assignment)
        {
            let follower_limit = assignment_target_limit(self.live_registers).saturating_sub(1);
            group.push(symbol);
            followers
                .iter()
                .take(follower_limit)
                .take_while(|stmt| {
                    let Statement::Assignment(follower) = stmt else {
                        return false;
                    };
                    self.single_assignment(follower).is_some_and(|(target, value)| {
                        // The language leaves open the order a multiple
                        // assignment stores in, so a repeated target could
                        // keep either value.
                        let can_join =
                            !group.contains(&target) && !self.references_any(value, group);
                        if can_join {
                            group.push(target);
                        }
                        can_join
                    })
                })
                .count()
        } else {
            0
        };
        start + 1 + follower_count
    }

    /// Moves the names and values of `stmts[start + 1..end]` into
    /// `stmts[start]`, all of one kind, and leaves the followers empty.
    fn merge_group(&self, stmts: &mut List<'ast, Statement<'ast>>, start: usize, end: usize) {
        let allocator = self.allocator;
        let (first, followers) =
            stmts[start..end].split_first_mut().expect("a group holds at least two statements");
        match first {
            Statement::LocalAssignment(local) => {
                for follower in followers {
                    if let Statement::LocalAssignment(next) = follower {
                        for name in mem::take(&mut next.names) {
                            local.names.push(name, allocator);
                        }
                        for expr in mem::take(&mut next.exprs) {
                            local.exprs.push(expr, allocator);
                        }
                    }
                }
                local.kind = LocalAssignmentKind::Local;
            }
            Statement::Assignment(assignment) => {
                for follower in followers {
                    if let Statement::Assignment(next) = follower {
                        for target in mem::take(&mut next.targets) {
                            assignment.targets.push(target, allocator);
                        }
                        for value in mem::take(&mut next.values) {
                            assignment.values.push(value, allocator);
                        }
                    }
                }
            }
            Statement::FunctionCall(_)
            | Statement::DoBlock(_)
            | Statement::WhileLoop(_)
            | Statement::RepeatLoop(_)
            | Statement::IfStatement(_)
            | Statement::NumericFor(_)
            | Statement::GenericFor(_)
            | Statement::FunctionDecl(_)
            | Statement::LocalFunction(_)
            | Statement::Return(_)
            | Statement::Break(_)
            | Statement::Continue(_)
            | Statement::CompoundAssignment(_)
            | Statement::TypeDeclaration(_)
            | Statement::Error(_) => {}
        }
    }

    /// Only a local may join a group, since any other store can run
    /// `__newindex`, and the language leaves open the order a group stores
    /// in. A group also holds one register per value until the stores run,
    /// and only a leaf value needs exactly one.
    fn single_assignment<'tree>(
        &self,
        assignment: &'tree Assignment<'ast>,
    ) -> Option<(SymbolId, &'tree Expression<'ast>)> {
        if let ([Var::Name(name)], [expr]) = (&*assignment.targets, &*assignment.values)
            && is_single_register_value(expr)
        {
            let symbol = self.scoping.resolved_symbol_of(name)?;
            return Some((symbol, expr));
        }
        None
    }

    /// Whether `expr` references any of `group`.
    fn references_any(&self, expr: &Expression<'_>, group: &[SymbolId]) -> bool {
        let mut finder = self.reference_finder(group);
        finder.visit_expr(expr);
        finder.has_reference
    }

    /// Fuses `local a, b` plus `a, b = X, Y` into `local a, b = X, Y`, and
    /// says whether it fused any.
    fn fuse_bare_locals(&mut self, stmts: &mut List<'ast, Statement<'ast>>) -> bool {
        let mut has_fused = false;
        let mut kept = 0;
        let mut index = 0;
        let mut declared = mem::take(&mut self.group);
        while index < stmts.len() {
            let stmt = &stmts[index];
            let can_fuse = if let Statement::LocalAssignment(local) = stmt
                && let Some(Statement::Assignment(assignment)) = stmts.get(index + 1)
            {
                is_bare_local(stmt) && self.can_fuse(&local.names, assignment, &mut declared)
            } else {
                false
            };
            if can_fuse
                && let Some([Statement::LocalAssignment(local), Statement::Assignment(assignment)]) =
                    stmts.get_mut(index..index + 2)
            {
                // A bare local is never const or exported, so fusing only adds
                // the values, and the writes go.
                local.exprs = mem::take(&mut assignment.values);
                for target in &assignment.targets {
                    self.dropper().visit_var(target);
                }
                has_fused = true;
                stmts.swap(kept, index);
                kept += 1;
                index += 2;
                continue;
            }
            stmts.swap(kept, index);
            kept += 1;
            index += 1;
        }
        self.group = declared;
        stmts.truncate(kept);
        self.has_changed |= has_fused;
        has_fused
    }

    /// Whether `assignment` assigns exactly the locals `names` declares, each
    /// once, so the two can fuse into one initialized declaration. Those
    /// locals go in `declared`.
    fn can_fuse(
        &self,
        names: &[TypedName<'_>],
        assignment: &Assignment<'_>,
        declared: &mut Vec<SymbolId>,
    ) -> bool {
        // Every target must be the matching declared local, or a field store
        // like the `t.x` in `a, t.x = 1, 2` would be lost in the fused
        // `local`.
        if names.len() != assignment.targets.len() {
            return false;
        }
        declared.clear();
        for (local_name, target) in names.iter().zip(assignment.targets.iter()) {
            let (Some(symbol), Var::Name(name)) =
                (self.scoping.symbol_of(&local_name.name), target)
            else {
                return false;
            };
            // `local a, a; a, a = 1, 2` stores both values in the second `a`,
            // right to left, but `local a, a = 1, 2` gives it the last.
            if self.scoping.resolved_symbol_of(name) != Some(symbol) || declared.contains(&symbol) {
                return false;
            }
            declared.push(symbol);
        }
        // The values must not reference the declared locals: in
        // `local f; f = function() f() end` the body's `f` is the new local,
        // but fused into `local f = function() ... end` it would resolve to
        // an outer `f`.
        !assignment.values.iter().any(|value| self.references_any(value, declared))
    }
}

/// How many locals `stmt` declares.
pub(super) fn declared_count(stmt: &Statement<'_>) -> usize {
    match stmt {
        Statement::LocalAssignment(local) => local.names.len(),
        Statement::LocalFunction(_) => 1,
        Statement::Assignment(_)
        | Statement::FunctionCall(_)
        | Statement::DoBlock(_)
        | Statement::WhileLoop(_)
        | Statement::RepeatLoop(_)
        | Statement::IfStatement(_)
        | Statement::NumericFor(_)
        | Statement::GenericFor(_)
        | Statement::FunctionDecl(_)
        | Statement::Return(_)
        | Statement::Break(_)
        | Statement::Continue(_)
        | Statement::CompoundAssignment(_)
        | Statement::TypeDeclaration(_)
        | Statement::Error(_) => 0,
    }
}

#[cfg(test)]
mod tests {
    use luck_allocator::Allocator;

    use crate::fixtures::{NO_PASSES, assert_outputs, minify_with, parse};
    use crate::transform_config::TransformConfig;

    const MERGING: TransformConfig = TransformConfig { merge_locals: true, ..NO_PASSES };

    fn merge(source: &str) -> String {
        minify_with(source, &MERGING)
    }

    #[test]
    fn merges_neighboring_locals_and_assignments() {
        assert_outputs(
            merge,
            &[
                (
                    "local a = 1 local b = 2 local c = 3 print(a, b, c)",
                    "local a,b,c=1,2,3 print(a,b,c)",
                ),
                ("local x = 1 local x = 2", "local x,x=1,2"),
                ("local a local b", "local a,b"),
                ("local a, b a, b = 1, 2", "local a,b=1,2"),
                ("local x x = 1 x = 2", "local x=1 x=2"),
                ("local a, b = f() a = 1 b = 2", "local a,b=f()a,b=1,2"),
                ("local function g(a, b) a = 1 b = 2 end", "local function g(a,b)a,b=1,2 end"),
                (
                    "local a, b = f() local function g() a = 1 b = 2 end",
                    "local a,b=f()local function g()a,b=1,2 end",
                ),
                ("local x, y = f() x = 1 y = 2 x = 3", "local x,y=f()x,y=1,2 x=3"),
            ],
        );
    }

    #[test]
    fn keeps_apart_what_merging_would_change() {
        for source in [
            "local a=1 print(a)local b=2",
            "local x=f()local y=g()print(x,y)",
            "local a=1 local b=a+1 print(b)",
            "local a={}local b=function()function a.x()end end b()print(a.x)",
            "a=1 b=2",
            "local a=f()a=1 b=2",
            "do local a=f()end a=1 b=2",
            "a.b=1 c.d=2",
            "x=1 x=2",
            "local a,a a,a=1,2",
            "local a=1 local b=a::number print(b)",
            "local a=...local b:typeof(a)=g print(b)",
        ] {
            assert_eq!(merge(source), source);
        }
    }

    /// A fuse runs as the walk leaves a block, so what the local it
    /// initializes allows next, a merge or an inline, has to happen in the
    /// same minify for the output to minify to itself.
    #[test]
    fn a_fused_local_takes_what_it_allows_in_one_minify() {
        let every_pass = |source: &str| minify_with(source, &TransformConfig::default());
        assert_outputs(
            every_pass,
            &[
                ("local u = tive local util util = require", "local e,t=tive,require"),
                ("local q = f local b b = g print(b, b)", "local t,e=f,g print(e,e)"),
                ("local a a = 1 print(a)", "print(1)"),
                ("local a local b a, b = 1, 2 print(a, b)", "print(1,2)"),
                ("local a local z = 1 a = 2 print(a)", "print(2)"),
                ("local a a = 1", ""),
                ("local a a = {} a.x = 1 return a", "local e={}e.x=1 return e"),
            ],
        );
    }

    #[test]
    fn tracks_scopes_under_more_locals_than_a_lookup_scans() {
        let names =
            |prefix: char| (0..100).map(|i| format!("{prefix}{i}")).collect::<Vec<_>>().join(",");
        let (outer, inner) = (names('v'), names('w'));
        let source = format!(
            "local {outer}=f()v0=1 v1=2 local c,d=g()c=1 d=2 do local {inner}=h()w0=1 end w0=1 v2=2"
        );
        assert_eq!(
            merge(&source),
            format!(
                "local {outer}=f()v0,v1=1,2 local c,d=g()c,d=1,2 do local {inner}=h()w0=1 end w0=1 v2=2"
            )
        );
    }

    fn largest_assignment_target_count(output: &str) -> usize {
        let allocator = Allocator::new();
        parse(&allocator, output)
            .stmts
            .iter()
            .filter_map(|stmt| {
                if let luck_ast::Statement::Assignment(assignment) = stmt {
                    Some(assignment.targets.len())
                } else {
                    None
                }
            })
            .max()
            .unwrap_or_default()
    }

    #[test]
    fn merged_assignments_stay_within_the_registers_the_targets_need() {
        let names = (0..100).map(|i| format!("g{i}")).collect::<Vec<_>>().join(",");
        let assignments = (0..100).map(|i| format!("g{i} = {i}\n")).collect::<Vec<_>>().concat();
        let global_count =
            largest_assignment_target_count(&merge(&format!("local {names} = f()\n{assignments}")));
        assert!((2..=195).contains(&global_count), "{global_count} targets");

        let locals = (0..190).map(|i| format!("local v{i} = x{i}\n")).collect::<Vec<_>>().concat();
        let stores = (0..70).map(|i| format!("v{i} = {i}\n")).collect::<Vec<_>>().concat();
        let local_count = largest_assignment_target_count(&merge(&format!("{locals}{stores}")));
        assert!((2..=60).contains(&local_count), "{local_count} targets");
    }
}
