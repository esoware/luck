use core::iter::Peekable;

use luck_ast::{
    Assignment, Block, Expression, LocalAssignment, Punctuated, Statement, Transform, Var, Visitor,
};
use luck_token::{CompactString, LuaVersion, Span, Token};
use rustc_hash::FxHashSet;

use crate::binding::{has_fixed_binding, identifier_name, is_env_binding};
use crate::expr::is_pure_expr;

/// Statements a merged group may take on after its first.
///
/// Each RHS value of a multi-assignment takes its own register, and a
/// function only has about 250 of them, hence the cap.
const MAX_GROUP_FOLLOWERS: usize = 199;

/// Merges consecutive single-assignment locals (or globals) into
/// multi-assignment statements.
pub(crate) fn merge(block: Block, version: LuaVersion) -> Block {
    LocalMerger { version }.transform_block(block)
}

struct LocalMerger {
    version: LuaVersion,
}

/// The name and value of a lone pure `local name = value`, the only kind of
/// `local` an initialized group merges.
fn single_local(local: &LocalAssignment, version: LuaVersion) -> Option<(&Token, &Expression)> {
    // Fixed bindings stay unmerged. Merging a `const` with plain locals, for
    // one, would extend or drop its const-ness.
    if has_fixed_binding(local, version) {
        return None;
    }
    let exprs = local.exprs.as_ref()?;
    if let ([attributed], [expr]) = (local.names.items.as_slice(), exprs.items.as_slice())
        && is_pure_expr(expr)
    {
        return Some((&attributed.name, expr));
    }
    None
}

/// The name and value of a lone pure `name = value`, the only kind of
/// assignment a group merges.
fn single_assignment(
    assignment: &Assignment,
    version: LuaVersion,
) -> Option<(&Token, &Expression)> {
    if let ([Var::Name(name)], [expr]) =
        (assignment.targets.items.as_slice(), assignment.values.items.as_slice())
        && !is_env_binding(identifier_name(name), version)
        && is_pure_expr(expr)
    {
        return Some((name, expr));
    }
    None
}

/// Whether the single assignment of `value` to `name` can join a group that
/// has `follower_count` followers and assigns `group_names`. One that joins
/// adds its name to `group_names` for the next one to check.
fn can_join_group(
    name: &Token,
    value: &Expression,
    follower_count: usize,
    group_names: &mut FxHashSet<CompactString>,
) -> bool {
    let can_join =
        follower_count < MAX_GROUP_FOLLOWERS && !references_any_in_expr(value, group_names);
    if can_join {
        group_names.insert(identifier_name(name).into());
    }
    can_join
}

impl LocalMerger {
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
            single_local(local, self.version).is_some_and(|(name, value)| {
                can_join_group(name, value, followers.len(), &mut group_names)
            })
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
        let mut group_names = FxHashSet::from_iter([first_name]);
        let mut followers = Vec::new();
        while let Some(Statement::Assignment(next)) = stmts.next_if(|stmt| {
            let Statement::Assignment(assignment) = stmt else {
                return false;
            };
            single_assignment(assignment, self.version).is_some_and(|(name, value)| {
                // PUC Lua stores a multiple assignment right to left, so a
                // repeated target would keep its first value.
                !group_names.contains(identifier_name(name))
                    && can_join_group(name, value, followers.len(), &mut group_names)
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
        let mut merged_stmts: Vec<Statement> = Vec::new();
        let mut stmts = block.stmts.into_iter().peekable();

        while let Some(first_stmt) = stmts.next() {
            merged_stmts.push(match first_stmt {
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
                    if let Some((name, _)) = single_assignment(&first, self.version) {
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
            });
        }

        // Fuse `local a,b` plus `a,b=X,Y` into `local a,b=X,Y`.
        let fused_stmts = fuse_bare_locals(merged_stmts, self.version);

        let last_stmt = block.last_stmt.map(|mut last| {
            *last = self.transform_last_stmt(*last);
            last
        });

        Block { span: block.span, stmts: fused_stmts, last_stmt }
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
    fn merges_consecutive_assignments() {
        let result = apply("a = 1\nb = 2\nc = 3\n");
        let equal_count = result.matches('=').count();
        assert_eq!(equal_count, 1, "Global assignments should merge, got: {result}");
    }

    #[test]
    fn no_merge_of_repeated_assignment_target() {
        for (source, expected) in [
            ("x = 1\nx = 2\n", "x=1 x=2"),
            ("local x\nx = 1\nx = 2\n", "local x=1 x=2"),
            ("x = 1\ny = 2\nx = 3\n", "x,y=1,2 x=3"),
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
    fn bug_keeps_the_store_order_of_global_assignments() {
        let result = apply("a = 1\nb = 2\n");
        assert_ne!(
            result, "a,b=1,2",
            "PUC stores right to left, so `__newindex` would see `b` before `a`"
        );
    }

    #[test]
    fn bug_merged_assignment_stays_within_the_parser_assignment_limit() {
        let source = (0..250u32).map(|i| format!("g{i} = {i}\n")).collect::<Vec<_>>().concat();
        let target_count = largest_assignment_target_count(&apply(&source));
        assert!(
            target_count <= 195,
            "PUC parsers reject an assignment this long, but a statement has {target_count} targets"
        );
    }

    #[test]
    fn bug_merged_assignment_leaves_registers_for_live_locals() {
        let locals =
            (0..190u32).map(|i| format!("local v{i} = x{i}\n")).collect::<Vec<_>>().concat();
        let assignments = (0..70u32).map(|i| format!("h{i} = {i}\n")).collect::<Vec<_>>().concat();
        let target_count =
            largest_assignment_target_count(&apply(&format!("{locals}{assignments}")));
        assert!(
            target_count <= 60,
            "190 live locals leave room for 60 values, but a statement has {target_count} targets"
        );
    }
}
