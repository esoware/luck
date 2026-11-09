use core::iter::Peekable;
use core::mem;

use luck_ast::{
    Assignment, Block, Expression, FunctionBody, LocalAssignment, Punctuated, Statement, Transform,
    Var, Visitor,
};
use luck_token::{CompactString, LuaVersion, Span, Token};
use rustc_hash::FxHashSet;

use crate::binding::{has_fixed_binding, identifier_name, is_env_binding};
use crate::expr::{is_pure_modulo_name_reads, is_single_register_value};
use crate::limits::{
    GENERIC_FOR_STATE_LOCALS, NUMERIC_FOR_STATE_LOCALS, assignment_target_limit, parameter_locals,
};

/// Merges consecutive single-assignment locals, or assignments to proven
/// locals, into multi-assignment statements.
pub(crate) fn merge(block: Block, version: LuaVersion) -> Block {
    LocalMerger {
        version,
        live_registers: 0,
        locals: Vec::new(),
        global_names: FxHashSet::default(),
    }
    .transform_block(block)
}

struct LocalMerger {
    version: LuaVersion,
    /// Registers the locals in scope hold in the current function.
    live_registers: usize,
    /// Declared locals and parameters in scope, upvalues included.
    locals: Vec<CompactString>,
    /// Lua 5.5: every name a `global` statement has declared so far, which
    /// stops counting as a local even past that statement's block.
    global_names: FxHashSet<CompactString>,
}

/// The name and value of a lone `local name = value` whose value a group may
/// evaluate beside its neighbors, the only kind of `local` an initialized
/// group merges.
fn single_local(local: &LocalAssignment, version: LuaVersion) -> Option<(&Token, &Expression)> {
    // Fixed bindings stay unmerged. Merging a `const` with plain locals, for
    // one, would extend or drop its const-ness.
    if has_fixed_binding(local, version) {
        return None;
    }
    let exprs = local.exprs.as_ref()?;
    if let ([attributed], [expr]) = (local.names.items.as_slice(), exprs.items.as_slice())
        && is_pure_modulo_name_reads(expr)
    {
        return Some((&attributed.name, expr));
    }
    None
}

/// Whether the single assignment of `value` to `name` can join a group that
/// assigns `group_names`. One that joins adds its name to `group_names` for
/// the next one to check.
fn can_join_group(
    name: &Token,
    value: &Expression,
    group_names: &mut FxHashSet<CompactString>,
) -> bool {
    let can_join = !references_any_in_expr(value, group_names);
    if can_join {
        group_names.insert(identifier_name(name).into());
    }
    can_join
}

impl LocalMerger {
    fn declare_local(&mut self, name: &Token) {
        self.locals.push(identifier_name(name).into());
        self.live_registers = self.live_registers.saturating_add(1);
    }

    /// The name and value of a lone `name = value` a group can take.
    ///
    /// A group stores right to left, so only a proven local may join, since
    /// any other store can run `__newindex`. A group also holds one register
    /// per value until the stores run, and only a leaf value needs exactly
    /// one.
    fn single_assignment<'stmt>(
        &self,
        assignment: &'stmt Assignment,
    ) -> Option<(&'stmt Token, &'stmt Expression)> {
        if let ([Var::Name(name)], [expr]) =
            (assignment.targets.items.as_slice(), assignment.values.items.as_slice())
            && is_single_register_value(expr)
        {
            let target = identifier_name(name);
            let is_proven_local = !is_env_binding(target, self.version)
                && !self.global_names.contains(target)
                && self.locals.iter().rev().any(|local| local == target);
            return is_proven_local.then_some((name, expr));
        }
        None
    }

    fn merge_locals(
        &mut self,
        first: Box<LocalAssignment>,
        first_name: CompactString,
        stmts: &mut Peekable<impl Iterator<Item = Statement>>,
    ) -> Statement {
        let mut group_names = FxHashSet::from_iter([first_name]);
        let mut followers = Vec::new();
        while let Some(Statement::LocalAssignment(next)) = stmts.next_if(|stmt| {
            let Statement::LocalAssignment(local) = stmt else {
                return false;
            };
            single_local(local, self.version)
                .is_some_and(|(name, value)| can_join_group(name, value, &mut group_names))
        }) {
            followers.push(next);
        }
        if followers.is_empty() {
            return self.transform_stmt(Statement::LocalAssignment(first));
        }

        let mut names = first.names.into_items();
        let mut exprs: Vec<Expression> =
            first.exprs.into_iter().flat_map(Punctuated::into_items).collect();
        for next in followers {
            names.extend(next.names.into_items());
            exprs.extend(next.exprs.into_iter().flat_map(Punctuated::into_items));
        }
        self.transform_stmt(Statement::LocalAssignment(Box::new(LocalAssignment {
            span: Span::default(),
            names: Punctuated::from_items(names),
            exprs: Some(Punctuated::from_items(exprs)),
            is_const: false,
            is_exported: false,
        })))
    }

    fn merge_assignments(
        &mut self,
        first: Box<Assignment>,
        first_name: CompactString,
        stmts: &mut Peekable<impl Iterator<Item = Statement>>,
    ) -> Statement {
        let follower_limit = assignment_target_limit(self.live_registers).saturating_sub(1);
        let mut group_names = FxHashSet::from_iter([first_name]);
        let mut followers = Vec::new();
        while let Some(Statement::Assignment(next)) = stmts.next_if(|stmt| {
            let Statement::Assignment(assignment) = stmt else {
                return false;
            };
            followers.len() < follower_limit
                && self.single_assignment(assignment).is_some_and(|(name, value)| {
                    // PUC Lua stores a multiple assignment right to left, so a
                    // repeated target would keep its first value.
                    !group_names.contains(identifier_name(name))
                        && can_join_group(name, value, &mut group_names)
                })
        }) {
            followers.push(next);
        }
        if followers.is_empty() {
            return self.transform_stmt(Statement::Assignment(first));
        }

        let mut targets = first.targets.into_items();
        let mut values = first.values.into_items();
        for next in followers {
            targets.extend(next.targets.into_items());
            values.extend(next.values.into_items());
        }
        self.transform_stmt(Statement::Assignment(Box::new(Assignment {
            span: Span::default(),
            targets: Punctuated::from_items(targets),
            values: Punctuated::from_items(values),
        })))
    }

    /// Merges consecutive bare locals: `local a\nlocal b` becomes `local a,b`.
    fn merge_bare_locals(
        &mut self,
        first: Box<LocalAssignment>,
        stmts: &mut Peekable<impl Iterator<Item = Statement>>,
    ) -> Statement {
        let mut followers = Vec::new();
        while let Some(Statement::LocalAssignment(next)) =
            stmts.next_if(|stmt| is_bare_local_stmt(stmt, self.version))
        {
            followers.push(next);
        }
        if followers.is_empty() {
            return self.transform_stmt(Statement::LocalAssignment(first));
        }

        let mut names = first.names.into_items();
        for next in followers {
            names.extend(next.names.into_items());
        }
        Statement::LocalAssignment(Box::new(LocalAssignment {
            span: Span::default(),
            names: Punctuated::from_items(names),
            exprs: None,
            is_const: false,
            is_exported: false,
        }))
    }
}

impl Transform for LocalMerger {
    fn transform_block(&mut self, block: Block) -> Block {
        let outer_live_registers = self.live_registers;
        let outer_local_count = self.locals.len();
        let mut merged_stmts: Vec<Statement> = Vec::new();
        let mut stmts = block.stmts.into_iter().peekable();

        while let Some(first_stmt) = stmts.next() {
            let merged_stmt = match first_stmt {
                Statement::LocalAssignment(first) => {
                    if let Some((name, _)) = single_local(&first, self.version) {
                        let first_name = CompactString::from(identifier_name(name));
                        self.merge_locals(first, first_name, &mut stmts)
                    } else if is_bare_local(&first, self.version) {
                        self.merge_bare_locals(first, &mut stmts)
                    } else {
                        self.transform_stmt(Statement::LocalAssignment(first))
                    }
                }
                Statement::Assignment(first) => {
                    if let Some((name, _)) = self.single_assignment(&first) {
                        let first_name = CompactString::from(identifier_name(name));
                        self.merge_assignments(first, first_name, &mut stmts)
                    } else {
                        self.transform_stmt(Statement::Assignment(first))
                    }
                }
                other @ (Statement::FunctionCall(_)
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
                | Statement::Error(_)) => self.transform_stmt(other),
            };
            if let Statement::LocalAssignment(local) = &merged_stmt {
                for attributed in local.names.iter() {
                    self.declare_local(&attributed.name);
                }
            }
            if let Statement::LocalFunction(function) = &merged_stmt {
                self.declare_local(&function.name);
            }
            merged_stmts.push(merged_stmt);
        }

        // Fuse `local a,b` plus `a,b=X,Y` into `local a,b=X,Y`.
        let fused_stmts = fuse_bare_locals(merged_stmts, self.version);

        let last_stmt = block.last_stmt.map(|mut last| {
            *last = self.transform_last_stmt(*last);
            last
        });

        self.live_registers = outer_live_registers;
        self.locals.truncate(outer_local_count);
        Block { span: block.span, stmts: fused_stmts, last_stmt }
    }

    fn transform_stmt(&mut self, stmt: Statement) -> Statement {
        // Lua 5.5
        if let Statement::GlobalDeclaration(global) = &stmt {
            for attributed in global.names.iter() {
                self.global_names.insert(identifier_name(&attributed.name).into());
            }
        }
        // Lua 5.5
        if let Statement::GlobalFunction(function) = &stmt {
            self.global_names.insert(identifier_name(&function.name).into());
        }
        let outer_live_registers = self.live_registers;
        let loop_registers = if let Statement::NumericFor(_) = &stmt {
            NUMERIC_FOR_STATE_LOCALS.saturating_add(1)
        } else if let Statement::GenericFor(generic_for) = &stmt {
            GENERIC_FOR_STATE_LOCALS.saturating_add(generic_for.names.len())
        } else {
            0
        };
        self.live_registers = self.live_registers.saturating_add(loop_registers);
        let walked = self.walk_stmt(stmt);
        self.live_registers = outer_live_registers;
        walked
    }

    fn transform_function_body(&mut self, body: FunctionBody) -> FunctionBody {
        let outer_live_registers = mem::replace(&mut self.live_registers, parameter_locals(&body));
        let outer_local_count = self.locals.len();
        self.locals.extend(body.params.iter().map(|param| identifier_name(&param.name).into()));
        let walked = self.walk_function_body(body);
        self.locals.truncate(outer_local_count);
        self.live_registers = outer_live_registers;
        walked
    }
}

fn is_bare_local(local: &LocalAssignment, version: LuaVersion) -> bool {
    local.exprs.is_none() && !has_fixed_binding(local, version)
}

fn is_bare_local_stmt(stmt: &Statement, version: LuaVersion) -> bool {
    matches!(stmt, Statement::LocalAssignment(local) if is_bare_local(local, version))
}

/// Whether `stmt` assigns exactly the names a bare `local` declares, so the
/// two can fuse into one initialized declaration.
fn can_fuse_assignment(local: &LocalAssignment, stmt: &Statement) -> bool {
    let Statement::Assignment(assignment) = stmt else {
        return false;
    };
    // Every target must be the matching declared name, or a field store like
    // the `t.x` in `a, t.x = 1, 2` would be lost in the fused `local`.
    let has_same_names = local.names.len() == assignment.targets.len()
        && local.names.iter().zip(assignment.targets.iter()).all(|(attributed, target)| {
            matches!(
                target,
                Var::Name(name) if identifier_name(name) == identifier_name(&attributed.name)
            )
        });
    if !has_same_names {
        return false;
    }
    // The RHS must not reference the declared names: in
    // `local f; f = function() f() end` the body's `f` is the new local, but
    // fused into `local f = function() ... end` it would resolve to an outer
    // `f`.
    let declared_names: FxHashSet<CompactString> =
        local.names.iter().map(|attributed| identifier_name(&attributed.name).into()).collect();
    // `local a, a; a, a = 1, 2` stores both values in the second `a`, right to
    // left, but `local a, a = 1, 2` gives it the last.
    declared_names.len() == local.names.len()
        && !assignment.values.iter().any(|value| references_any_in_expr(value, &declared_names))
}

fn fuse_bare_locals(stmts: Vec<Statement>, version: LuaVersion) -> Vec<Statement> {
    let mut fused_stmts: Vec<Statement> = Vec::with_capacity(stmts.len());
    let mut remaining = stmts.into_iter().peekable();

    while let Some(mut stmt) = remaining.next() {
        // A bare local is never const or exported, so fusing only adds the
        // values.
        if let Statement::LocalAssignment(local) = &mut stmt
            && is_bare_local(local, version)
            && let Some(Statement::Assignment(assignment)) =
                remaining.next_if(|next_stmt| can_fuse_assignment(local, next_stmt))
        {
            local.exprs = Some(assignment.values);
        }
        fused_stmts.push(stmt);
    }

    fused_stmts
}

/// Reports whether any `Var::Name` or function declaration root in `names`
/// appears in the visited subtree.
///
/// `Visitor` reaches every node, closure bodies included. Assignment targets
/// go through `visit_var` too, so writes count as well as reads.
struct ReferenceFinder<'names> {
    names: &'names FxHashSet<CompactString>,
    has_reference: bool,
}

impl<'ast> Visitor<'ast> for ReferenceFinder<'_> {
    fn visit_var(&mut self, var: &'ast Var) {
        if self.has_reference {
            return;
        }
        if let Var::Name(name) = var
            && self.names.contains(identifier_name(name))
        {
            self.has_reference = true;
            return;
        }
        self.walk_var(var);
    }

    fn visit_expr(&mut self, expr: &'ast Expression) {
        if self.has_reference {
            return;
        }
        self.walk_expr(expr);
    }

    fn visit_stmt(&mut self, stmt: &'ast Statement) {
        if self.has_reference {
            return;
        }
        if let Statement::FunctionDecl(function_decl) = stmt
            && let Some(root) = function_decl.name.names.first()
            && self.names.contains(identifier_name(root))
        {
            self.has_reference = true;
            return;
        }
        self.walk_stmt(stmt);
    }
}

fn references_any_in_expr(expr: &Expression, names: &FxHashSet<CompactString>) -> bool {
    let mut finder = ReferenceFinder { names, has_reference: false };
    finder.visit_expr(expr);
    finder.has_reference
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixtures::{parse, run_pass};

    fn apply(source: &str) -> String {
        apply_version(source, LuaVersion::Lua54)
    }

    fn apply_version(source: &str, version: LuaVersion) -> String {
        run_pass(source, version, |block| merge(block, version))
    }

    #[test]
    fn merges_consecutive_locals() {
        let result = apply("local a = 1\nlocal b = 2\nlocal c = 3\nprint(a, b, c)\n");
        let local_count = result.matches("local").count();
        assert_eq!(local_count, 1, "Expected single local statement, got: {result}");
    }

    #[test]
    fn no_merge_across_non_local() {
        let result = apply("local a = 1\nprint(a)\nlocal b = 2\n");
        let local_count = result.matches("local").count();
        assert_eq!(local_count, 2, "Should have 2 separate locals, got: {result}");
    }

    #[test]
    fn no_merge_of_call_initializers() {
        let result = apply("local x = f()\nlocal y = g()\nprint(x, y)\n");
        let local_count = result.matches("local").count();
        assert_eq!(local_count, 2, "Function calls should not merge, got: {result}");
    }

    #[test]
    fn no_merge_when_dependency_in_binary() {
        let result = apply("local a = 1\nlocal b = a + 1\nprint(b)\n");
        let local_count = result.matches("local").count();
        assert_eq!(
            local_count, 2,
            "Should not merge when rhs references prior local, got: {result}"
        );
    }

    #[test]
    fn no_merge_when_a_closure_declares_a_function_on_the_earlier_name() {
        let source = "local a = {}\nlocal b = function() function a.x() end end\nb()\nprint(a.x)\n";
        let result = apply(source);
        assert!(result.contains("local a = {}") || result.contains("local a={}"), "{result}");
        assert!(!result.contains("local a, b") && !result.contains("local a,b"), "{result}");
    }

    #[test]
    fn merges_assignments_to_proven_locals() {
        for (source, expected) in [
            ("local a, b = f()\na = 1\nb = 2\n", "local a,b=f()a,b=1,2"),
            ("local function g(a, b)\na = 1\nb = 2\nend\n", "local function g(a,b)a,b=1,2 end"),
            (
                "local a, b = f()\nlocal function g()\na = 1\nb = 2\nend\n",
                "local a,b=f()local function g()a,b=1,2 end",
            ),
        ] {
            assert_eq!(apply(source), expected, "source: {source}");
        }
    }

    #[test]
    fn no_merge_of_global_assignments() {
        assert_eq!(apply("a = 1\nb = 2\n"), "a=1 b=2");
    }

    #[test]
    fn no_merge_of_a_local_with_a_global() {
        assert_eq!(apply("local a = f()\na = 1\nb = 2\n"), "local a=f()a=1 b=2");
    }

    #[test]
    fn no_merge_of_a_name_past_its_local_scope() {
        assert_eq!(apply("do local a = f() end\na = 1\nb = 2\n"), "do local a=f()end a=1 b=2");
    }

    #[test]
    fn no_merge_of_index_targets() {
        for source in ["a.b = 1\nc.d = 2\n", "local a, c = f()\na.b = 1\nc[1] = 2\n"] {
            let result = apply(source);
            assert!(result.ends_with("=1 c.d=2") || result.ends_with("=1 c[1]=2"), "{result}");
        }
    }

    #[test]
    fn no_merge_of_fields_of_a_local_env() {
        assert_eq!(apply("local _ENV = f()\nx = 1\ny = 2\n"), "local _ENV=f()x=1 y=2");
    }

    #[test]
    fn no_merge_of_an_assignment_to_env() {
        assert_eq!(apply("local _ENV, y = f()\n_ENV = 1\ny = 2\n"), "local _ENV,y=f()_ENV=1 y=2");
    }

    #[test]
    fn no_merge_of_a_local_a_global_statement_shadows() {
        assert_eq!(
            apply_version("local x, y = f()\nglobal x\nx = 1\ny = 2\n", LuaVersion::Lua55),
            "local x,y=f()global x x=1 y=2"
        );
    }

    #[test]
    fn no_merge_of_repeated_assignment_target() {
        for (source, expected) in [
            ("x = 1\nx = 2\n", "x=1 x=2"),
            ("local x\nx = 1\nx = 2\n", "local x=1 x=2"),
            ("local x, y = f()\nx = 1\ny = 2\nx = 3\n", "local x,y=f()x,y=1,2 x=3"),
        ] {
            assert_eq!(apply(source), expected, "source: {source}");
        }
    }

    #[test]
    fn no_fuse_of_repeated_declared_name() {
        let result = apply("local a, a\na, a = 1, 2\n");
        assert_eq!(result, "local a,a a,a=1,2");
    }

    #[test]
    fn merges_redeclared_local() {
        assert_eq!(apply("local x = 1\nlocal x = 2\n"), "local x,x=1,2");
    }

    fn parse_luau(source: &str) -> Block {
        parse(source, LuaVersion::Luau)
    }

    fn names_of(items: &[&str]) -> FxHashSet<CompactString> {
        items.iter().copied().map(CompactString::from).collect()
    }

    fn first_expr_value(block: &Block) -> Expression {
        let Statement::LocalAssignment(local) = &block.stmts[0] else {
            panic!("expected local assignment, got {:?}", block.stmts[0]);
        };
        local.exprs.as_ref().unwrap().iter().next().unwrap().clone()
    }

    #[test]
    fn detects_reference_in_type_cast() {
        let block = parse_luau("local y = x :: number\n");
        let expr = first_expr_value(&block);
        assert!(references_any_in_expr(&expr, &names_of(&["x"])));
    }

    #[test]
    fn type_cast_dependency_blocks_merge() {
        let result =
            apply_version("local a = 1\nlocal b = a :: number\nprint(b)\n", LuaVersion::Luau);
        let local_count = result.matches("local").count();
        assert_eq!(
            local_count, 2,
            "type cast referencing prior local must block merge, got: {result}"
        );
    }

    #[test]
    fn detects_reference_in_nested_block() {
        let block =
            parse_luau("local y = function()\n  if true then\n    while x do end\n  end\nend\n");
        let expr = first_expr_value(&block);
        assert!(references_any_in_expr(&expr, &names_of(&["x"])));
    }

    #[test]
    fn detects_reference_in_call_argument() {
        let block = parse_luau("local y = function()\n  foo(x)\nend\n");
        let expr = first_expr_value(&block);
        assert!(
            references_any_in_expr(&expr, &names_of(&["x"])),
            "call argument reference must be detected"
        );
        let callee = parse_luau("local y = function()\n  x()\nend\n");
        assert!(references_any_in_expr(&first_expr_value(&callee), &names_of(&["x"])));
    }

    #[test]
    fn detects_reference_in_closure_body() {
        let block = parse_luau("local y = function() return x + 1 end\n");
        let expr = first_expr_value(&block);
        assert!(references_any_in_expr(&expr, &names_of(&["x"])));
    }

    // A nested declaration of the name isn't a reference, since declaration
    // tokens aren't `Var::Name` nodes.
    #[test]
    fn declaration_of_same_name_is_not_a_reference() {
        let block = parse_luau("local y = function()\n  local x = 1\n  return 0\nend\n");
        let expr = first_expr_value(&block);
        assert!(
            !references_any_in_expr(&expr, &names_of(&["x"])),
            "a fresh declaration of the same name must not count as a reference"
        );
    }

    // Both the target and the value of a Luau `+=` count as references.
    #[test]
    fn detects_reference_in_compound_assignment() {
        let target = parse_luau("local y = function()\n  x += 1\nend\n");
        assert!(references_any_in_expr(&first_expr_value(&target), &names_of(&["x"])));
        let value = parse_luau("local y = function()\n  z += x\nend\n");
        assert!(references_any_in_expr(&first_expr_value(&value), &names_of(&["x"])));
    }

    fn largest_assignment_target_count(output: &str) -> usize {
        parse(output, LuaVersion::Lua54)
            .stmts
            .iter()
            .filter_map(|stmt| {
                if let Statement::Assignment(assignment) = stmt {
                    Some(assignment.targets.len())
                } else {
                    None
                }
            })
            .max()
            .unwrap_or_default()
    }

    #[test]
    fn merged_assignment_stays_within_the_parser_assignment_limit() {
        let names = (0..100u32).map(|i| format!("g{i}")).collect::<Vec<_>>().join(",");
        let assignments = (0..100u32).map(|i| format!("g{i} = {i}\n")).collect::<Vec<_>>().concat();
        let target_count =
            largest_assignment_target_count(&apply(&format!("local {names} = f()\n{assignments}")));
        assert!(
            (2..=195).contains(&target_count),
            "PUC parsers reject an assignment this long, but a statement has {target_count} targets"
        );
    }

    #[test]
    fn merged_assignment_leaves_registers_for_live_locals() {
        let locals =
            (0..190u32).map(|i| format!("local v{i} = x{i}\n")).collect::<Vec<_>>().concat();
        let assignments = (0..70u32).map(|i| format!("v{i} = {i}\n")).collect::<Vec<_>>().concat();
        let target_count =
            largest_assignment_target_count(&apply(&format!("{locals}{assignments}")));
        assert!(
            (2..=60).contains(&target_count),
            "190 live locals leave room for 60 values, but a statement has {target_count} targets"
        );
    }
}
