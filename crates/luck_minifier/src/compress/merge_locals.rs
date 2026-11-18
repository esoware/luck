//! Merging neighboring `local`s, and neighboring assignments to proven
//! locals, into one statement.

use core::mem;

use luck_allocator::List;
use luck_ast::{
    Assignment, AttributedName, Expression, LocalAssignment, LocalAssignmentKind, Name, Statement,
    Var, Visit,
};
use luck_token::LuaVersion;

use super::Compressor;
use crate::binding::{Bindings, has_fixed_binding, is_env_binding, local_of};
use crate::expr::{is_pure_modulo_name_reads, is_single_register_value};
use crate::limits::assignment_target_limit;

/// The binding of a lone `local name = value` whose value a group may
/// evaluate beside its neighbors, the only kind of `local` an initialized
/// group merges.
fn single_local(stmt: &Statement<'_>, version: LuaVersion) -> Option<usize> {
    let Statement::LocalAssignment(local) = stmt else {
        return None;
    };
    // Fixed bindings stay unmerged. Merging a `const` with plain locals, for
    // one, would extend or drop its const-ness.
    if has_fixed_binding(local, version) {
        return None;
    }
    if let ([attributed], [expr]) = (&*local.names, &*local.exprs)
        && is_pure_modulo_name_reads(expr)
    {
        return local_of(&attributed.name);
    }
    None
}

pub(super) fn is_bare_local(stmt: &Statement<'_>, version: LuaVersion) -> bool {
    matches!(stmt, Statement::LocalAssignment(local) if local.exprs.is_empty() && !has_fixed_binding(local, version))
}

/// Finds a read or write of any of a group's bindings, closure bodies
/// included.
struct ReferenceFinder<'group> {
    group: &'group [usize],
    bindings: &'group Bindings,
    has_reference: bool,
}

impl ReferenceFinder<'_> {
    fn check(&mut self, name: &Name<'_>) {
        if let Some(id) = local_of(name) {
            let resolved = self.bindings.resolve(id);
            self.has_reference |=
                self.group.iter().any(|&member| self.bindings.resolve(member) == resolved);
        }
    }
}

impl<'tree> Visit<'tree> for ReferenceFinder<'_> {
    fn visit_var(&mut self, var: &'tree Var<'tree>) {
        if self.has_reference {
            return;
        }
        if let Var::Name(name) = var {
            self.check(name);
            return;
        }
        self.walk_var(var);
    }

    fn visit_expr(&mut self, expr: &'tree Expression<'tree>) {
        if self.has_reference {
            return;
        }
        if let Expression::Name(name) = expr {
            self.check(name);
            return;
        }
        self.walk_expr(expr);
    }

    fn visit_stmt(&mut self, stmt: &'tree Statement<'tree>) {
        if self.has_reference {
            return;
        }
        if let Statement::FunctionDecl(function_decl) = stmt
            && let Some(root) = function_decl.name.names.first()
        {
            self.check(root);
        }
        self.walk_stmt(stmt);
    }
}

fn references_any(expr: &Expression<'_>, group: &[usize], bindings: &Bindings) -> bool {
    let mut finder = ReferenceFinder { group, bindings, has_reference: false };
    finder.visit_expr(expr);
    finder.has_reference
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
        let mut kept = 0usize;
        let mut index = 0usize;
        let mut group = mem::take(&mut self.group);
        while index < stmts.len() {
            let group_end = self.merge_group_end(stmts, index, &mut group);
            if group_end > index.saturating_add(1) {
                self.merge_group(stmts, index, group_end);
                self.has_changed = true;
            }
            stmts.swap(kept, index);
            if let Some(stmt) = stmts.get(kept) {
                self.live_registers = self.live_registers.saturating_add(declared_count(stmt));
            }
            kept = kept.saturating_add(1);
            index = group_end;
        }
        self.group = group;
        stmts.truncate(kept);
    }

    /// The end of the run of statements from `start` that merge into it.
    fn merge_group_end(
        &self,
        stmts: &[Statement<'ast>],
        start: usize,
        group: &mut Vec<usize>,
    ) -> usize {
        let Some(first) = stmts.get(start) else {
            return start;
        };
        let followers = stmts.get(start.saturating_add(1)..).unwrap_or_default();
        group.clear();
        let bindings = &*self.bindings;
        let follower_count = if let Some(id) = single_local(first, self.version) {
            group.push(id);
            followers
                .iter()
                .take_while(|stmt| {
                    single_local(stmt, self.version).is_some_and(|follower| {
                        // A Luau annotation binds before the name, as the
                        // value does.
                        let mut finder = ReferenceFinder {
                            group: group.as_slice(),
                            bindings,
                            has_reference: false,
                        };
                        finder.visit_stmt(stmt);
                        let can_join = !finder.has_reference;
                        if can_join {
                            group.push(follower);
                        }
                        can_join
                    })
                })
                .count()
        } else if is_bare_local(first, self.version) {
            followers.iter().take_while(|stmt| is_bare_local(stmt, self.version)).count()
        } else if let Statement::Assignment(assignment) = first
            && let Some((id, _)) = self.single_assignment(assignment)
        {
            let follower_limit = assignment_target_limit(self.live_registers).saturating_sub(1);
            group.push(id);
            followers
                .iter()
                .take(follower_limit)
                .take_while(|stmt| {
                    let Statement::Assignment(follower) = stmt else {
                        return false;
                    };
                    self.single_assignment(follower).is_some_and(|(follower_id, value)| {
                        // PUC Lua stores a multiple assignment right to left,
                        // so a repeated target would keep its first value.
                        let target = bindings.resolve(follower_id);
                        let can_join =
                            !group.iter().any(|&member| bindings.resolve(member) == target)
                                && !references_any(value, group, bindings);
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
        start.saturating_add(1).saturating_add(follower_count)
    }

    /// Moves the names and values of `stmts[start + 1..end]` into
    /// `stmts[start]`, all of one kind, and leaves the followers empty.
    fn merge_group(&self, stmts: &mut List<'ast, Statement<'ast>>, start: usize, end: usize) {
        let allocator = self.allocator;
        let Some((first, followers)) = stmts.get_mut(start..end).and_then(<[_]>::split_first_mut)
        else {
            return;
        };
        match first {
            Statement::LocalAssignment(local) => {
                let mut names: Vec<AttributedName<'ast>> =
                    mem::take(&mut local.names).into_iter().collect();
                let mut exprs: Vec<Expression<'ast>> =
                    mem::take(&mut local.exprs).into_iter().collect();
                for follower in followers {
                    if let Statement::LocalAssignment(next) = follower {
                        names.extend(mem::take(&mut next.names));
                        exprs.extend(mem::take(&mut next.exprs));
                    }
                }
                local.names = List::from_iter_in(names, allocator);
                local.exprs = List::from_iter_in(exprs, allocator);
                local.kind = LocalAssignmentKind::Local;
            }
            Statement::Assignment(assignment) => {
                let mut targets: Vec<Var<'ast>> =
                    mem::take(&mut assignment.targets).into_iter().collect();
                let mut values: Vec<Expression<'ast>> =
                    mem::take(&mut assignment.values).into_iter().collect();
                for follower in followers {
                    if let Statement::Assignment(next) = follower {
                        targets.extend(mem::take(&mut next.targets));
                        values.extend(mem::take(&mut next.values));
                    }
                }
                assignment.targets = List::from_iter_in(targets, allocator);
                assignment.values = List::from_iter_in(values, allocator);
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

    /// A group stores right to left, so only a local may join, since any
    /// other store can run `__newindex`. A group also holds one register per
    /// value until the stores run, and only a leaf value needs exactly one.
    fn single_assignment<'tree>(
        &self,
        assignment: &'tree Assignment<'ast>,
    ) -> Option<(usize, &'tree Expression<'ast>)> {
        if let ([Var::Name(name)], [expr]) = (&*assignment.targets, &*assignment.values)
            && is_single_register_value(expr)
            && !is_env_binding(name.text.as_str(), self.version)
        {
            return local_of(name).map(|id| (id, expr));
        }
        None
    }

    /// Fuses `local a, b` plus `a, b = X, Y` into `local a, b = X, Y`, and
    /// says whether it fused any.
    fn fuse_bare_locals(&mut self, stmts: &mut List<'ast, Statement<'ast>>) -> bool {
        let mut has_fused = false;
        let mut kept = 0usize;
        let mut index = 0usize;
        while index < stmts.len() {
            let can_fuse = if let (Some(stmt), Some(Statement::Assignment(assignment))) =
                (stmts.get(index), stmts.get(index.saturating_add(1)))
                && let Statement::LocalAssignment(local) = stmt
            {
                is_bare_local(stmt, self.version)
                    && can_fuse_assignment(local, assignment, self.bindings)
            } else {
                false
            };
            if can_fuse
                && let Some([Statement::LocalAssignment(local), Statement::Assignment(assignment)]) =
                    stmts.get_mut(index..index.saturating_add(2))
            {
                // A bare local is never const or exported, so fusing only adds
                // the values.
                local.exprs = mem::take(&mut assignment.values);
                // The walk is past the initialized local this makes, which
                // goes or inlines if it's read once or never.
                self.needs_revisit |= local.names.iter().any(|attributed| {
                    local_of(&attributed.name)
                        .and_then(|id| self.bindings.get(id))
                        .is_some_and(|binding| binding.reads <= 1)
                });
                has_fused = true;
                stmts.swap(kept, index);
                kept = kept.saturating_add(1);
                index = index.saturating_add(2);
                continue;
            }
            stmts.swap(kept, index);
            kept = kept.saturating_add(1);
            index = index.saturating_add(1);
        }
        stmts.truncate(kept);
        self.has_changed |= has_fused;
        has_fused
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
        | Statement::EmptyStatement(_)
        | Statement::Goto(_)
        | Statement::Label(_)
        | Statement::GlobalDeclaration(_)
        | Statement::GlobalFunction(_)
        | Statement::GlobalStar(_)
        | Statement::Break(_)
        | Statement::CompoundAssignment(_)
        | Statement::TypeDeclaration(_)
        | Statement::Error(_) => 0,
    }
}

/// Whether `assignment` assigns exactly the bindings a bare `local`
/// declares, each once, so the two can fuse into one initialized
/// declaration.
fn can_fuse_assignment(
    local: &LocalAssignment<'_>,
    assignment: &Assignment<'_>,
    bindings: &Bindings,
) -> bool {
    // Every target must be the matching declared local, or a field store like
    // the `t.x` in `a, t.x = 1, 2` would be lost in the fused `local`.
    if local.names.len() != assignment.targets.len() {
        return false;
    }
    let mut declared = Vec::with_capacity(local.names.len());
    for (attributed, target) in local.names.iter().zip(assignment.targets.iter()) {
        let (Some(id), Var::Name(name)) = (local_of(&attributed.name), target) else {
            return false;
        };
        let resolved = bindings.resolve(id);
        // `local a, a; a, a = 1, 2` stores both values in the second `a`,
        // right to left, but `local a, a = 1, 2` gives it the last.
        if local_of(name).map(|assigned| bindings.resolve(assigned)) != Some(resolved)
            || declared.contains(&resolved)
        {
            return false;
        }
        declared.push(resolved);
    }
    // The values must not reference the declared locals: in
    // `local f; f = function() f() end` the body's `f` is the new local, but
    // fused into `local f = function() ... end` it would resolve to an outer
    // `f`.
    !assignment.values.iter().any(|value| references_any(value, &declared, bindings))
}

#[cfg(test)]
mod tests {
    use luck_allocator::Allocator;
    use luck_token::LuaVersion;

    use crate::fixtures::{NO_PASSES, assert_outputs, minify_with, parse};
    use crate::transform_config::TransformConfig;

    const MERGING: TransformConfig = TransformConfig { merge_locals: true, ..NO_PASSES };

    fn merge(source: &str) -> String {
        minify_with(source, LuaVersion::Lua54, &MERGING)
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
            "local _ENV=f()x=1 y=2",
            "local _ENV,y=f()_ENV=1 y=2",
            "x=1 x=2",
            "local a,a a,a=1,2",
        ] {
            assert_eq!(merge(source), source);
        }
        assert_eq!(
            minify_with("local x,y=f()global x x=1 y=2", LuaVersion::Lua55, &MERGING),
            "local x,y=f()global x x=1 y=2"
        );
        for source in
            ["local a=1 local b=a::number print(b)", "local a=...local b:typeof(a)=g print(b)"]
        {
            assert_eq!(minify_with(source, LuaVersion::Luau, &MERGING), source);
        }
    }

    /// A fuse runs as the walk leaves a block, so what the local it
    /// initializes allows next, a merge or an inline, has to happen in the
    /// same minify for the output to minify to itself.
    #[test]
    fn a_fused_local_takes_what_it_allows_in_one_minify() {
        let every_pass =
            |source: &str| minify_with(source, LuaVersion::Lua54, &TransformConfig::default());
        assert_outputs(
            every_pass,
            &[
                ("local u = tive local util util = require", "local l,u=tive,require"),
                ("local q = f local b b = g print(b, b)", "local u,l=f,g print(l,l)"),
                ("local a a = 1 print(a)", "print(1)"),
                ("local a local b a, b = 1, 2 print(a, b)", "print(1,2)"),
                ("local a local z = 1 a = 2 print(a)", "print(2)"),
                ("local a a = 1", ""),
                ("local a a = {} a.x = 1 return a", "local l={}l.x=1 return l"),
            ],
        );
    }

    #[test]
    fn tracks_scopes_under_more_locals_than_a_lookup_scans() {
        let names = |prefix: char| {
            (0..100u32).map(|i| format!("{prefix}{i}")).collect::<Vec<_>>().join(",")
        };
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
        parse(&allocator, output, LuaVersion::Lua54)
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
        let names = (0..100u32).map(|i| format!("g{i}")).collect::<Vec<_>>().join(",");
        let assignments = (0..100u32).map(|i| format!("g{i} = {i}\n")).collect::<Vec<_>>().concat();
        let global_count =
            largest_assignment_target_count(&merge(&format!("local {names} = f()\n{assignments}")));
        assert!((2..=195).contains(&global_count), "{global_count} targets");

        let locals =
            (0..190u32).map(|i| format!("local v{i} = x{i}\n")).collect::<Vec<_>>().concat();
        let stores = (0..70u32).map(|i| format!("v{i} = {i}\n")).collect::<Vec<_>>().concat();
        let local_count = largest_assignment_target_count(&merge(&format!("{locals}{stores}")));
        assert!((2..=60).contains(&local_count), "{local_count} targets");
    }
}
