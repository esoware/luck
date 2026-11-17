//! Rewrites of locals: dropping the ones nothing uses, inlining the ones read
//! once, and merging neighbors into one statement.

use core::mem;

use luck_allocator::List;
use luck_ast::{
    Assignment, AttributedName, BinaryOp, Expression, LocalAssignment, LocalAssignmentKind,
    MAX_NESTING_DEPTH, Statement, Var, Visitor, VisitorMut as _, expr_nesting_depth,
};
use luck_token::{BinOp, LuaVersion, MAX_ACTIVE_LOCALS, OperandSide};

use super::Compressor;
use crate::binding::{has_fixed_binding, is_env_binding};
use crate::expr::{
    MAX_ADDED_DEPTH, binds_looser_than_unary, is_nil, is_pure_expr, is_pure_modulo_name_reads,
    is_single_register_value, literal_kind, parenthesize,
};
use crate::limits::assignment_target_limit;
use crate::scan::{DeadLocalAction, is_call, unused_local_action};

/// An inlined local's initializer, waiting for the local's one read.
pub(super) struct Value<'ast> {
    expr: Expression<'ast>,
    /// The inlined locals it took in, its own included. Each is still in
    /// scope where its local is declared, so a chain past
    /// [`MAX_ACTIVE_LOCALS`] of them is longer than one function can hold,
    /// and it stops there.
    locals: usize,
}

#[derive(Clone, Copy)]
pub(super) enum OperandPosition {
    Left(BinOp),
    Right(BinOp),
    Unary,
    Cast,
}

impl OperandPosition {
    pub(super) const fn of_side(op: BinOp, side: OperandSide) -> Self {
        match side {
            OperandSide::Left => Self::Left(op),
            OperandSide::Right => Self::Right(op),
        }
    }
}

/// Grouping lives in `Parenthesized` nodes and codegen prints the tree flat,
/// so a value that binds looser than its position would regroup on reparse.
///
/// A value is a closed literal expression, so none prints a Luau
/// if-expression last.
fn keeps_grouping(value: &Expression<'_>, position: OperandPosition) -> bool {
    match value {
        Expression::BinaryOp(run) => match position {
            OperandPosition::Left(op) => run.can_nest_bare(op, OperandSide::Left),
            OperandPosition::Right(op) => run.can_nest_bare(op, OperandSide::Right),
            OperandPosition::Unary => !binds_looser_than_unary(run),
            OperandPosition::Cast => false,
        },
        Expression::UnaryOp(_) => match position {
            // Unary ops bind looser than ^, so (-a)^b needs its parens.
            OperandPosition::Left(op) => op != BinOp::Pow,
            OperandPosition::Right(_) | OperandPosition::Unary => true,
            OperandPosition::Cast => false,
        },
        // Luau: `::` takes a simple expression, and `x :: A :: B` doesn't
        // parse.
        Expression::TypeCast(_) => !matches!(position, OperandPosition::Cast),
        Expression::Number(_)
        | Expression::Integer(_)
        | Expression::StringLiteral(_)
        | Expression::Nil(_)
        | Expression::True(_)
        | Expression::False(_)
        | Expression::VarArg(_)
        | Expression::FunctionDef(_)
        | Expression::Name(_)
        | Expression::Suffixed(_)
        | Expression::Parenthesized(_)
        | Expression::TableConstructor(_)
        | Expression::IfExpression(_)
        | Expression::InterpolatedString(_)
        | Expression::Error(_) => true,
    }
}

/// The operand `expr` prints last, which is what a following token lands on.
fn last_operand<'tree, 'ast>(expr: &'tree mut Expression<'ast>) -> &'tree mut Expression<'ast> {
    let mut current = expr;
    loop {
        current = match current {
            Expression::BinaryOp(binary_op) => &mut binary_op.links.last_mut().operand,
            Expression::UnaryOp(unop) => &mut unop.operand,
            Expression::Number(_)
            | Expression::Integer(_)
            | Expression::StringLiteral(_)
            | Expression::Nil(_)
            | Expression::True(_)
            | Expression::False(_)
            | Expression::VarArg(_)
            | Expression::FunctionDef(_)
            | Expression::Name(_)
            | Expression::Suffixed(_)
            | Expression::Parenthesized(_)
            | Expression::TableConstructor(_)
            | Expression::IfExpression(_)
            | Expression::InterpolatedString(_)
            | Expression::TypeCast(_)
            | Expression::Error(_) => return current,
        };
    }
}

/// `local x = nil` can become `local x`, since nil is the default. Const
/// declarations keep their mandatory initializer.
fn is_nil_initialized(local: &LocalAssignment<'_>) -> bool {
    !local.kind.is_const()
        && local.names.len() == 1
        && matches!(&*local.exprs, [value] if is_nil(value))
}

/// The name and value of a lone `local name = value` whose value a group may
/// evaluate beside its neighbors, the only kind of `local` an initialized
/// group merges.
fn single_local<'tree, 'ast>(
    stmt: &'tree Statement<'ast>,
    version: LuaVersion,
) -> Option<(&'ast str, &'tree Expression<'ast>)> {
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
        return Some((attributed.name.text, expr));
    }
    None
}

fn is_bare_local(stmt: &Statement<'_>, version: LuaVersion) -> bool {
    matches!(stmt, Statement::LocalAssignment(local) if local.exprs.is_empty() && !has_fixed_binding(local, version))
}

/// Finds a read or write of any of a group's names, closure bodies included.
struct ReferenceFinder<'names, 'ast> {
    names: &'names [&'ast str],
    has_reference: bool,
}

impl<'tree> Visitor<'tree> for ReferenceFinder<'_, '_> {
    fn visit_assignment_target(&mut self, target: &'tree Var<'tree>) {
        if self.has_reference {
            return;
        }
        if let Var::Name(name) = target
            && contains(self.names, name.text)
        {
            self.has_reference = true;
            return;
        }
        self.walk_assignment_target(target);
    }

    fn visit_expr(&mut self, expr: &'tree Expression<'tree>) {
        if self.has_reference {
            return;
        }
        if let Expression::Name(name) = expr
            && contains(self.names, name.text)
        {
            self.has_reference = true;
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
            && contains(self.names, root.text)
        {
            self.has_reference = true;
            return;
        }
        self.walk_stmt(stmt);
    }
}

fn contains(names: &[&str], name: &str) -> bool {
    names.contains(&name)
}

fn references_any(expr: &Expression<'_>, names: &[&str]) -> bool {
    let mut finder = ReferenceFinder { names, has_reference: false };
    finder.visit_expr(expr);
    finder.has_reference
}

/// Whether a local's value moves to its one read, `read_depth` levels deep,
/// having taken in the values of `landed_locals` locals inlined into it.
fn can_inline(value: &Expression<'_>, landed_locals: usize, read_depth: usize) -> bool {
    if landed_locals >= MAX_ACTIVE_LOCALS || literal_kind(value).is_none() {
        return false;
    }
    let depth = expr_nesting_depth(value);
    // The value can gain parentheses where it lands, and a cast inside it
    // a second pair.
    depth <= MAX_ADDED_DEPTH
        && read_depth.saturating_add(2).saturating_add(depth) <= MAX_NESTING_DEPTH
}

#[expect(
    clippy::multiple_inherent_impl,
    reason = "the compressor is one type with its rewrites split by family"
)]
impl<'ast> Compressor<'ast> {
    /// Walks a `local` and drops what of it is dead or inlines: the whole
    /// statement, or in one that gives each name its own value, a name and
    /// its value. Says whether anything of it stays.
    pub(super) fn visit_local(&mut self, stmt: &mut Statement<'ast>) -> bool {
        // An initializer can hold `local`s of its own, so each list is a
        // stack that every `local` leaves as it found it.
        let facts_start = self.local_facts.len();
        let landed_start = self.landed_counts.len();
        let is_kept = self.visit_local_from(stmt, facts_start, landed_start);
        self.local_facts.truncate(facts_start);
        self.landed_counts.truncate(landed_start);
        is_kept
    }

    fn visit_local_from(
        &mut self,
        stmt: &mut Statement<'ast>,
        facts_start: usize,
        landed_start: usize,
    ) -> bool {
        let Statement::LocalAssignment(boxed) = stmt else {
            return true;
        };
        let local = &mut **boxed;
        let should_remove_dead = self.config.remove_dead_code;
        let Self { scan, names, local_facts, .. } = self;
        local_facts.extend(
            local.names.iter().map(|attributed| scan.local(names.find(attributed.name.text))),
        );
        let is_dead = should_remove_dead
            && !self
                .local_facts
                .get(facts_start..)
                .unwrap_or_default()
                .iter()
                .any(|&(is_live, _)| is_live);
        if is_dead && unused_local_action(local, self.version) == DeadLocalAction::Remove {
            return false;
        }
        self.walk_attributed_names(&mut local.names);
        for expr in &mut local.exprs {
            let outer_landed_locals = mem::replace(&mut self.landed_locals, 0);
            self.visit_expr(expr);
            let landed_locals = mem::replace(&mut self.landed_locals, outer_landed_locals);
            self.landed_counts.push(landed_locals);
        }
        if is_dead {
            match unused_local_action(local, self.version) {
                DeadLocalAction::Keep => {}
                DeadLocalAction::Remove => return false,
                DeadLocalAction::ExtractCalls => {
                    self.extract_calls(stmt);
                    return true;
                }
            }
        }
        if should_remove_dead && is_nil_initialized(local) {
            local.exprs = List::new();
            self.has_changed = true;
            return true;
        }
        // Dropping a name and its value keeps every other name's value only
        // when each name has its own.
        if has_fixed_binding(local, self.version) || local.names.len() != local.exprs.len() {
            return true;
        }
        let mut kept = 0usize;
        for index in 0..local.names.len() {
            let (
                Some(attributed),
                Some(value),
                Some(&landed_locals),
                Some(&(is_live, inline_read_depth)),
            ) = (
                local.names.get(index),
                local.exprs.get_mut(index),
                self.landed_counts.get(landed_start.saturating_add(index)),
                self.local_facts.get(facts_start.saturating_add(index)),
            )
            else {
                break;
            };
            let name = attributed.name.text;
            let is_dropped = if should_remove_dead && !is_live && is_pure_expr(value) {
                true
            } else if self.config.inline_locals
                && let Some(read_depth) = inline_read_depth
                && can_inline(value, landed_locals, read_depth)
                && let Some(id) = self.names.find(name)
            {
                let locals = landed_locals.saturating_add(1);
                self.values.insert(id, Value { expr: value.take(), locals });
                true
            } else {
                false
            };
            if is_dropped {
                self.has_changed = true;
            } else {
                local.names.swap(kept, index);
                local.exprs.swap(kept, index);
                kept = kept.saturating_add(1);
            }
        }
        local.names.truncate(kept);
        local.exprs.truncate(kept);
        kept > 0
    }

    /// Puts the calls of an unused `local` whose other initializers are pure
    /// in its place.
    fn extract_calls(&mut self, stmt: &mut Statement<'ast>) {
        let Statement::LocalAssignment(local) = stmt else {
            return;
        };
        let mut calls = local.exprs.iter_mut().filter(|expr| is_call(expr)).filter_map(|expr| {
            if let Expression::Suffixed(chain) = expr.take() {
                Some(Statement::FunctionCall(chain))
            } else {
                None
            }
        });
        let first_call = calls.next();
        self.extracted_calls.extend(calls);
        if let Some(call) = first_call {
            *stmt = call;
        }
        self.has_changed = true;
    }

    /// An inlined local's value for its one read, which it reaches as the
    /// walk does, already rewritten.
    pub(super) fn take_value(&mut self, name: &str) -> Option<Expression<'ast>> {
        let value = self.values.remove(&self.names.find(name)?)?;
        self.landed_locals = self.landed_locals.saturating_add(value.locals);
        Some(value.expr)
    }

    /// Only in an operand position can a value regroup, so only there does
    /// it gain parens. Anywhere else it stays bare, since `return (1+2)` is
    /// longer than `return 1+2`.
    pub(super) fn visit_operand(
        &mut self,
        operand: &mut Expression<'ast>,
        position: OperandPosition,
    ) {
        if let Expression::Name(name) = operand
            && !self.values.is_empty()
            && let Some(value) = self.take_value(name.text)
        {
            if keeps_grouping(&value, position) {
                self.has_bare_operand = true;
                *operand = value;
            } else {
                *operand = parenthesize(self.allocator, value);
            }
            return;
        }
        self.visit_expr_in(operand, true);
    }

    /// Luau: a cast has no closing token and `T<` opens generic arguments, so
    /// a cast inlined just before `<` needs parens of its own. A cast that was
    /// already there parsed fine, so only inlining can cause this.
    pub(super) fn close_casts_before_less_than(&mut self, run: &mut BinaryOp<'ast>) {
        let allocator = self.allocator;
        let mut before = &mut run.first;
        for link in &mut *run.links {
            if link.op == BinOp::Lt
                && let Expression::Name(name) = last_operand(before)
                && let Some(id) = self.names.find(name.text)
                && let Some(value) = self.values.get_mut(&id)
            {
                let cast = last_operand(&mut value.expr);
                if matches!(cast, Expression::TypeCast(_)) {
                    *cast = parenthesize(allocator, cast.take());
                }
            }
            before = &mut link.operand;
        }
    }

    /// `local a = 1 local b = 2` becomes `local a, b = 1, 2`, `a = 1 b = 2`
    /// becomes `a, b = 1, 2` where both are proven locals, `local a local b`
    /// becomes `local a, b`, and `local a, b` and then `a, b = X, Y` becomes
    /// `local a, b = X, Y`.
    pub(super) fn merge_stmts(&mut self, stmts: &mut List<'ast, Statement<'ast>>) {
        let mut kept = 0usize;
        let mut index = 0usize;
        let mut next_local = 0usize;
        let mut group_names = mem::take(&mut self.group_names);
        while index < stmts.len() {
            let group_end = self.merge_group_end(stmts, index, &mut group_names);
            if group_end > index.saturating_add(1) {
                self.merge_group(stmts, index, group_end);
                self.has_changed = true;
            }
            stmts.swap(kept, index);
            if let Some(stmt) = stmts.get(kept) {
                self.redeclare_locals(stmt, &mut next_local);
            }
            kept = kept.saturating_add(1);
            index = group_end;
        }
        self.group_names = group_names;
        stmts.truncate(kept);
        self.fuse_bare_locals(stmts);
    }

    /// The end of the run of statements from `start` that merge into it.
    fn merge_group_end(
        &self,
        stmts: &[Statement<'ast>],
        start: usize,
        group_names: &mut Vec<&'ast str>,
    ) -> usize {
        let Some(first) = stmts.get(start) else {
            return start;
        };
        let followers = stmts.get(start.saturating_add(1)..).unwrap_or_default();
        group_names.clear();
        let follower_count = if let Some((name, _)) = single_local(first, self.version) {
            group_names.push(name);
            followers
                .iter()
                .take_while(|stmt| {
                    single_local(stmt, self.version).is_some_and(|(follower, value)| {
                        let can_join = !references_any(value, group_names);
                        if can_join {
                            group_names.push(follower);
                        }
                        can_join
                    })
                })
                .count()
        } else if is_bare_local(first, self.version) {
            followers.iter().take_while(|stmt| is_bare_local(stmt, self.version)).count()
        } else if let Statement::Assignment(assignment) = first
            && let Some((name, _)) = self.single_assignment(assignment)
        {
            let follower_limit = assignment_target_limit(self.live_registers).saturating_sub(1);
            group_names.push(name);
            followers
                .iter()
                .take(follower_limit)
                .take_while(|stmt| {
                    let Statement::Assignment(follower) = stmt else {
                        return false;
                    };
                    self.single_assignment(follower).is_some_and(|(target, value)| {
                        // PUC Lua stores a multiple assignment right to left,
                        // so a repeated target would keep its first value.
                        let can_join =
                            !group_names.contains(&target) && !references_any(value, group_names);
                        if can_join {
                            group_names.push(target);
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

    /// Past [`MAX_ACTIVE_LOCALS`] in scope, no name is proven a local.
    fn is_in_scope(&self, name: &str) -> bool {
        self.locals.len() <= MAX_ACTIVE_LOCALS
            && self
                .names
                .find(name)
                .is_some_and(|id| self.locals.iter().rev().any(|&local| local == id))
    }

    /// A group stores right to left, so only a proven local may join, since
    /// any other store can run `__newindex`. A group also holds one register
    /// per value until the stores run, and only a leaf value needs exactly
    /// one.
    fn single_assignment<'tree>(
        &self,
        assignment: &'tree Assignment<'ast>,
    ) -> Option<(&'ast str, &'tree Expression<'ast>)> {
        if let ([Var::Name(name)], [expr]) = (&*assignment.targets, &*assignment.values)
            && is_single_register_value(expr)
        {
            let target = name.text;
            let is_proven_local = !is_env_binding(target, self.version)
                && (self.global_names.is_empty() || !self.global_names.contains(target))
                && self.is_in_scope(target);
            return is_proven_local.then_some((target, expr));
        }
        None
    }

    /// Fuses `local a, b` plus `a, b = X, Y` into `local a, b = X, Y`.
    fn fuse_bare_locals(&mut self, stmts: &mut List<'ast, Statement<'ast>>) {
        let mut kept = 0usize;
        let mut index = 0usize;
        while index < stmts.len() {
            let can_fuse = if let (Some(stmt), Some(Statement::Assignment(assignment))) =
                (stmts.get(index), stmts.get(index.saturating_add(1)))
                && let Statement::LocalAssignment(local) = stmt
            {
                is_bare_local(stmt, self.version) && can_fuse_assignment(local, assignment)
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
                self.has_changed = true;
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
    }
}

/// Whether `assignment` assigns exactly the names a bare `local` declares, so
/// the two can fuse into one initialized declaration.
fn can_fuse_assignment(local: &LocalAssignment<'_>, assignment: &Assignment<'_>) -> bool {
    // Every target must be the matching declared name, or a field store like
    // the `t.x` in `a, t.x = 1, 2` would be lost in the fused `local`.
    let has_same_names = local.names.len() == assignment.targets.len()
        && local.names.iter().zip(assignment.targets.iter()).all(|(attributed, target)| {
            matches!(target, Var::Name(name) if name.text == attributed.name.text)
        });
    if !has_same_names {
        return false;
    }
    let declared_names: Vec<&str> =
        local.names.iter().map(|attributed| attributed.name.text).collect();
    // `local a, a; a, a = 1, 2` stores both values in the second `a`, right to
    // left, but `local a, a = 1, 2` gives it the last.
    let has_distinct_names = declared_names
        .iter()
        .enumerate()
        .all(|(i, name)| !declared_names.iter().take(i).any(|earlier| earlier == name));
    // The values must not reference the declared names: in
    // `local f; f = function() f() end` the body's `f` is the new local, but
    // fused into `local f = function() ... end` it would resolve to an outer
    // `f`.
    has_distinct_names
        && !assignment.values.iter().any(|value| references_any(value, &declared_names))
}

#[cfg(test)]
mod tests {
    use core::fmt::Write as _;

    use luck_allocator::Allocator;
    use luck_ast::{LastStatement, expr_nesting_depth};
    use luck_token::LuaVersion;

    use super::Compressor;
    use crate::fixtures::{NO_PASSES, minify_with, parse};
    use crate::transform_config::TransformConfig;

    const DEAD_CODE: TransformConfig = TransformConfig { remove_dead_code: true, ..NO_PASSES };
    const INLINING: TransformConfig = TransformConfig { inline_locals: true, ..NO_PASSES };
    const MERGING: TransformConfig = TransformConfig { merge_locals: true, ..NO_PASSES };

    fn remove_dead(source: &str) -> String {
        minify_with(source, LuaVersion::Lua54, &DEAD_CODE)
    }

    fn inline(source: &str) -> String {
        minify_with(source, LuaVersion::Lua54, &INLINING)
    }

    fn inline_luau(source: &str) -> String {
        minify_with(source, LuaVersion::Luau, &INLINING)
    }

    fn merge(source: &str) -> String {
        minify_with(source, LuaVersion::Lua54, &MERGING)
    }

    fn assert_outputs(apply: fn(&str) -> String, cases: &[(&str, &str)]) {
        for &(source, expected) in cases {
            assert_eq!(apply(source), expected, "source: {source}");
        }
    }

    /// One walk of `source`, printed.
    fn walk_once(source: &str, config: TransformConfig) -> String {
        let allocator = Allocator::new();
        let mut block = parse(&allocator, source, LuaVersion::Lua54);
        Compressor::new(&allocator, LuaVersion::Lua54, config).walk(&mut block);
        luck_codegen::compact(&block)
    }

    #[test]
    fn removes_unused_locals() {
        assert_outputs(
            remove_dead,
            &[
                ("local unused = 42 local used = 1 return used", "local used=1 return used"),
                ("local function unused() return 1 end return 2", "return 2"),
                ("local unused = 1 + 2", ""),
                ("local unused = foo()", "foo()"),
                ("local a, b = f(), 1", "f()"),
                ("local a, b = f(), g()", "f()g()"),
            ],
        );
    }

    #[test]
    fn keeps_unused_env_only_where_it_redirects_globals() {
        let source = "local _ENV = {} return 1";
        assert_eq!(remove_dead(source), "local _ENV={}return 1");
        for version in [LuaVersion::Lua51, LuaVersion::Luau] {
            assert_eq!(minify_with(source, version, &DEAD_CODE), "return 1", "{version:?}");
        }
    }

    #[test]
    fn keeps_local_rooting_a_function_declaration() {
        for source in [
            "local M = {} function M.foo() end print(\"ok\")",
            "local M = {} function M:foo() end print(\"ok\")",
            "local M = {} function M.a.b() end print(\"ok\")",
            "local M = {} do function M.foo() end end print(\"ok\")",
            "local M function M() end print(\"ok\")",
        ] {
            assert!(remove_dead(source).contains("local M"), "{source}");
        }
    }

    #[test]
    fn removes_unused_local_named_like_a_function_declaration_field() {
        assert_eq!(remove_dead("local foo = {} function M.foo() end"), "function M.foo()end");
    }

    #[test]
    fn removes_a_run_of_locals_each_used_only_by_the_next_in_one_walk() {
        let mut forward = String::from("local function f0() end\n");
        for (i, previous) in (1..300u32).zip(0u32..) {
            writeln!(forward, "local function f{i}() f{previous}() end").unwrap();
        }
        let mut backward = String::new();
        for (i, next) in (0..300u32).zip(1u32..) {
            writeln!(backward, "local f{i} = function() f{next}() end").unwrap();
        }
        for source in [forward, backward] {
            assert_eq!(walk_once(&source, DEAD_CODE), "", "source: {source}");
        }
    }

    #[test]
    fn keeps_a_local_used_by_a_kept_local() {
        let source = "local a = function() end local b = function() return a end return b";
        assert_eq!(
            walk_once(source, DEAD_CODE),
            "local a=function()end local b=function()return a end return b"
        );
    }

    #[test]
    fn removes_an_unused_local_function_that_calls_itself() {
        assert_eq!(walk_once("local function f() return f() end\n", DEAD_CODE), "");
    }

    #[test]
    fn a_kept_call_keeps_what_it_uses() {
        assert_eq!(
            walk_once("local y = 1 local x = function() return y end, g()", DEAD_CODE),
            "g()"
        );
        assert_eq!(walk_once("local y = 1 local x = g(y)", DEAD_CODE), "local y=1 g(y)");
    }

    #[test]
    fn keeps_unused_locals_whose_initializer_can_raise_or_read() {
        for initializer in [
            "g",
            "1 // 0",
            "1 % 0",
            "nil + 1",
            "#nil",
            "\"a\" .. nil",
            "1 < \"a\"",
            "-\"a\"",
            "{[nil] = 1}",
            "{[0/0] = 1}",
        ] {
            let result = remove_dead(&format!("local unused = {initializer} return 1"));
            assert!(result.contains("local unused"), "`{initializer}` must stay: {result}");
        }
    }

    #[test]
    fn inlines_single_use_literals() {
        assert_outputs(
            inline,
            &[
                ("local x = 42 return x", "return 42"),
                ("local x = 42 local y = x + 1 return y", "return 42+1"),
                (
                    "local function foo() local x = 42 return x end return foo",
                    "local function foo()return 42 end return foo",
                ),
                ("if true then local x = 42 print(x) end", "if true then print(42)end"),
                ("local x = 1 print(x)", "print(1)"),
                ("local x = 1 local function f() return x end", "local function f()return 1 end"),
                ("do local x = 1 do print(x) end end", "do do print(1)end end"),
                ("repeat local x = 1 until x == y", "repeat until 1==y"),
                ("repeat local x = 1 repeat until x until z", "repeat repeat until 1 until z"),
            ],
        );
    }

    #[test]
    fn keeps_what_inlining_would_change() {
        for source in [
            // More than one read.
            "local x=42 print(x)return x",
            // A call, a read, a table, a closure, and `...` each carry an
            // effect or an identity.
            "local x=foo()return x",
            "local x=y return x",
            "local t={1,2,3}return t",
            "local f=function()return 1 end return f",
            "local function f(...)local x=...return x end",
            // A read outside the declaration's scope is a global's.
            "print(x)local x=1",
            "do local x=1 end print(x)",
            // A write would land on a global.
            "local x=1 x=2",
            "local x=\"s\"function x.f()end print(x)",
            // The error must still fire before the print.
            "local x=1+nil print(\"before\")return x",
        ] {
            assert_eq!(inline(source), source);
        }
        assert_eq!(inline_luau("local x=1 x+=2"), "local x=1 x+=2");
        assert_eq!(inline_luau("local x=foo::Bar return x"), "local x=foo::Bar return x");
    }

    #[test]
    fn keeps_a_name_another_binding_wears() {
        assert_eq!(
            inline("local self = 5 function M:get() return self end"),
            "local self=5 function M:get()return self end"
        );
        assert_eq!(
            minify_with(
                "local arg = 5 local function f(...) return arg end return f",
                LuaVersion::Lua51,
                &INLINING
            ),
            "local arg=5 local function f(...)return arg end return f"
        );
        for source in [
            "local args=1 local function f(...args)return args end",
            "local x=1 global x print(x)",
            "local x=1 global function g()local x=2 return x end return g",
        ] {
            let result = minify_with(source, LuaVersion::Lua55, &INLINING);
            assert!(!result.contains("return 1"), "{source}: {result}");
        }
    }

    #[test]
    fn parenthesizes_a_value_that_would_regroup_where_it_lands() {
        assert_outputs(
            inline,
            &[
                ("local s = 1 / 0 return 3 % s", "return 3%(1/0)"),
                ("local s = 1 + 2 return s * 2", "return(1+2)*2"),
                ("local s = 1 + 2 return 2 * s", "return 2*(1+2)"),
                ("local s = 1 - 2 return 5 - s", "return 5-(1-2)"),
                ("local s = 2 ^ 3 return s ^ 2", "return(2^3)^2"),
                ("local s = 1 .. 2 return s .. 3", "return(1 ..2)..3"),
                ("local s = 1 + 2 return 2 ^ s", "return 2^(1+2)"),
                ("local s = 1 or 2 return s and 3", "return(1 or 2)and 3"),
                ("local s = -2 return s ^ y", "return(-2)^y"),
                ("local s = not nil return s ^ y", "return(not nil)^y"),
                ("local s = 1 + 2 return -s", "return-(1+2)"),
                ("local s = 1 == 2 return not s", "return not(1==2)"),
                ("local s = \"a\" .. \"b\" return #s", "return#(\"a\"..\"b\")"),
                ("local s = 1 + 2 return -s ^ 2", "return-(1+2)^2"),
                ("local s = -1 return -s ^ 2", "return-(-1)^2"),
                ("local s = 1 + 2 return (s * 3)", "return((1+2)*3)"),
                ("local s = 1 + 2 print(s * 3, 4)", "print((1+2)*3,4)"),
                ("local a = 1 + 2 local b = a * 3 return b ^ 2", "return((1+2)*3)^2"),
                ("local x = 37.5 return x[\"k\"]", "return(37.5)[\"k\"]"),
                ("local x = \"s\" return x.len", "return(\"s\").len"),
                ("local x = false x:resize(9)", "(false):resize(9)"),
            ],
        );
    }

    #[test]
    fn leaves_a_value_bare_where_its_grouping_holds() {
        assert_outputs(
            inline,
            &[
                ("local s = 1 + 2 return s", "return 1+2"),
                ("local s = 1 + 2 print(s)", "print(1+2)"),
                ("local s = 1 + 2 return {s}", "return{1+2}"),
                ("local s = 1 + 2 return t[s]", "return t[1+2]"),
                ("local s = 1 * 2 return s + 3", "return 1*2+3"),
                ("local s = 1 * 2 return 3 + s", "return 3+1*2"),
                ("local s = 1 - 2 return s - 5", "return 1-2-5"),
                ("local s = 2 ^ 3 return 2 ^ s", "return 2^2^3"),
                ("local s = 1 .. 2 return 3 .. s", "return 3 ..1 ..2"),
                ("local s = 1 + 2 return s < 4", "return 1+2<4"),
                ("local s = 1 < 2 return s == true", "return 1<2==true"),
                ("local s = 2 ^ 3 return -s", "return-2^3"),
                ("local s = -2 return 3 ^ s", "return 3^-2"),
                ("local s = -2 return s * 3", "return-2*3"),
                ("local s = -1 return -s", "return- -1"),
                ("local s = (1 + 2) return s * 2", "return(1+2)*2"),
            ],
        );
    }

    #[test]
    fn inlines_into_luau_casts_and_instantiations() {
        assert_outputs(
            inline_luau,
            &[
                ("local s = 1 + 2 return s :: any", "return(1+2)::any"),
                ("local s = -1 return s :: any", "return(-1)::any"),
                ("local s = 1 :: number return s :: any", "return(1::number)::any"),
                ("local s = 1 return s :: any", "return 1::any"),
                ("local s = 1 :: number return s < 2", "return(1::number)<2"),
                ("local s = 1 :: number return 2 + s < 3", "return 2+(1::number)<3"),
                ("local s = 1 :: number return -s < 2", "return-(1::number)<2"),
                ("local s = 1 :: number return 1 < s < 3", "return 1<(1::number)<3"),
                ("local s = 2 + 1 :: number return s < 3", "return 2+(1::number)<3"),
                ("local s = -1 :: number return s < 3", "return-(1::number)<3"),
                ("local s = 2 + 1 :: number return 2 * s < 3", "return 2*(2+(1::number))<3"),
                ("local s = 1 :: number return s", "return 1::number"),
                ("local s = 1 :: number return s + 1", "return 1::number+1"),
                ("local s = 1 :: number return 1 + s", "return 1+1::number"),
                ("local s = 1 :: number return s > 2", "return 1::number>2"),
                ("local s = 1 return x :: Foo<T> < s", "return x::Foo<T><1"),
                ("local s = \"x\" return s<<T>>", "return(\"x\")<<T>>"),
                ("local s = -1 return s<<T>>", "return(-1)<<T>>"),
                ("local s = \"x\" :: T return s<<U>>", "return(\"x\"::T)<<U>>"),
                ("local s = \"x\" return s<<T>><<U>>", "return(\"x\")<<T>><<U>>"),
                ("local s = (\"x\")<<T>> return s .. \"y\"", "local s=(\"x\")<<T>>return s..\"y\""),
            ],
        );
    }

    /// `local x0 = 1`, then `local xi = x(i-1) + 1` up to `x(links)`, returned.
    fn sum_chain(links: usize) -> String {
        let mut source = String::from("local x0 = 1\n");
        for (i, previous) in (1..=links).zip(0usize..) {
            writeln!(source, "local x{i} = x{previous} + 1").unwrap();
        }
        writeln!(source, "return x{links}").unwrap();
        source
    }

    #[test]
    fn inlines_a_chain_of_as_many_locals_as_one_function_holds() {
        let links = luck_token::MAX_ACTIVE_LOCALS - 1;
        let allocator = Allocator::new();
        let source = sum_chain(links);
        let mut block = parse(&allocator, &source, LuaVersion::Lua54);
        Compressor::new(&allocator, LuaVersion::Lua54, INLINING).compress(&mut block);
        let Some(LastStatement::Return(return_stmt)) = &block.last_stmt else {
            panic!("expected a return");
        };
        let run = return_stmt.exprs.first().expect("the return has its value");
        assert_eq!(expr_nesting_depth(run), 1, "the chain splices into one run");
        assert_eq!(luck_codegen::compact(&block), format!("return 1{}", "+1".repeat(links)));

        let mut concat = String::from("local x0 = \"z\"\n");
        for (i, previous) in (1..=links).zip(0usize..) {
            writeln!(concat, "local x{i} = \"a\" .. x{previous}").unwrap();
        }
        writeln!(concat, "return x{links}").unwrap();
        assert_eq!(inline(&concat), format!("return{}\"z\"", "\"a\"..".repeat(links)));
    }

    #[test]
    fn stops_a_chain_past_the_locals_one_function_holds() {
        let links = luck_token::MAX_ACTIVE_LOCALS;
        let expected = format!("local x{links}=1{} return x{links}", "+1".repeat(links));
        assert_eq!(walk_once(&sum_chain(links), INLINING), expected);
    }

    #[test]
    fn stops_inlining_a_run_of_locals_before_it_nests_past_the_depth_cap() {
        // Each value needs parentheses where the next one uses it.
        let mut source = String::from("local x0 = 1\n");
        for (i, previous) in (1..luck_token::MAX_ACTIVE_LOCALS).zip(0usize..) {
            writeln!(source, "local x{i} = x{previous} * 2 + 1").unwrap();
        }
        writeln!(source, "return x{}", luck_token::MAX_ACTIVE_LOCALS - 1).unwrap();
        assert!(inline(&source).contains("local"), "the run must stop inlining partway");
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
        assert_eq!(
            minify_with("local a=1 local b=a::number print(b)", LuaVersion::Luau, &MERGING),
            "local a=1 local b=a::number print(b)"
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
