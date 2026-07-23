use rustc_hash::FxHashSet;

use luck_ast::expr::*;
use luck_ast::shared::*;
use luck_ast::stmt::*;
use luck_ast::transform::AstTransform;
use luck_ast::visitor::Visitor;
use luck_token::{CompactString, Token};

use crate::expr::{ident_name, is_pure_expression};
use crate::tokens::default_span as sp;

/// Merge consecutive single-assignment locals (or globals) into multi-assignment statements.
pub fn merge(block: Block) -> Block {
    LocalMerger.transform_block(block)
}

struct LocalMerger;

fn extract_single_assignment_parts(stmt: &Statement) -> Option<(&Token, &Expression, bool)> {
    match stmt {
        Statement::LocalAssignment(local) => {
            // Const declarations stay unmerged: mixing them with plain
            // locals in one statement would extend or drop const-ness.
            if local.is_const || local.is_exported {
                return None;
            }
            if let Some(exprs) = &local.exprs {
                let names: Vec<_> = local.names.iter().collect();
                let expr_list: Vec<_> = exprs.iter().collect();
                if names.len() == 1
                    && expr_list.len() == 1
                    && is_pure_expression(expr_list[0], true)
                {
                    return Some((&names[0].name, expr_list[0], true));
                }
            }
            None
        }
        Statement::Assignment(assign) => {
            let vars: Vec<_> = assign.targets.iter().collect();
            let exprs: Vec<_> = assign.values.iter().collect();
            if vars.len() == 1
                && exprs.len() == 1
                && let Var::Name(name) = &vars[0]
                && is_pure_expression(exprs[0], true)
            {
                return Some((name, exprs[0], false));
            }
            None
        }
        _ => None,
    }
}

impl AstTransform for LocalMerger {
    fn transform_block(&mut self, block: Block) -> Block {
        let mut merged: Vec<Statement> = Vec::new();
        let mut iter = block.stmts.into_iter().peekable();

        // Statements are consumed, never cloned: grouping decisions need only
        // one statement of lookahead, so `peek` tests membership by reference
        // and `next` moves the statement into the group.
        while let Some(first_stmt) = iter.next() {
            let first_parts = extract_single_assignment_parts(&first_stmt)
                .map(|(name, _, is_local)| (CompactString::from(ident_name(name)), is_local));
            if let Some((first_name, is_local)) = first_parts {
                let mut declared: FxHashSet<CompactString> = FxHashSet::default();
                declared.insert(first_name);
                let mut group: Vec<Statement> = vec![first_stmt];

                loop {
                    let joins = match iter.peek() {
                        Some(next_stmt) => match extract_single_assignment_parts(next_stmt) {
                            Some((name, expr, next_is_local)) => {
                                // Luau allocates a register per RHS value in
                                // multi-assignments, hence the group cap.
                                if next_is_local != is_local
                                    || group.len() >= 200
                                    || references_any_in_expr(expr, &declared)
                                {
                                    false
                                } else {
                                    declared.insert(ident_name(name).into());
                                    true
                                }
                            }
                            None => false,
                        },
                        None => false,
                    };
                    if !joins {
                        break;
                    }
                    group.push(iter.next().expect("peeked statement exists"));
                }

                if group.len() >= 2 {
                    let total = group.len();
                    let mut group_names: Vec<AttributedName> = Vec::with_capacity(total);
                    let mut group_exprs: Vec<Expression> = Vec::with_capacity(total);
                    for stmt in group {
                        match stmt {
                            Statement::LocalAssignment(local) => {
                                let local = *local;
                                let exprs =
                                    local.exprs.expect("group members are single assignments");
                                group_names.extend(local.names.into_items());
                                group_exprs.extend(exprs.into_items());
                            }
                            Statement::Assignment(assign) => {
                                for var in assign.targets.into_items() {
                                    match var {
                                        Var::Name(name) => group_names.push(AttributedName {
                                            name,
                                            type_annotation: None,
                                            attrib: None,
                                        }),
                                        Var::FieldAccess(_) | Var::Index(_) => {
                                            unreachable!("group targets are bare names")
                                        }
                                    }
                                }
                                group_exprs.extend(assign.values.into_items());
                            }
                            _ => unreachable!("group members matched single-assignment shape"),
                        }
                    }
                    if is_local {
                        let merged_local = LocalAssignment {
                            span: sp(),
                            names: Punctuated::from_items(group_names),
                            exprs: Some(Punctuated::from_items(group_exprs)),
                            is_const: false,
                            is_exported: false,
                        };
                        merged.push(self.transform_statement(Statement::LocalAssignment(
                            Box::new(merged_local),
                        )));
                    } else {
                        let var_punct = Punctuated::from_items(
                            group_names
                                .into_iter()
                                .map(|attributed| Var::Name(attributed.name))
                                .collect(),
                        );
                        let merged_assign = Assignment {
                            span: sp(),
                            targets: var_punct,
                            values: Punctuated::from_items(group_exprs),
                        };
                        merged.push(
                            self.transform_statement(Statement::Assignment(Box::new(
                                merged_assign,
                            ))),
                        );
                    }
                } else {
                    let stmt = group.pop().expect("group holds the first statement");
                    merged.push(self.transform_statement(stmt));
                }
            } else if is_bare_local(&first_stmt) {
                // merge consecutive bare locals: `local a\nlocal b` -> `local a,b`
                let mut group: Vec<Statement> = vec![first_stmt];
                while iter.peek().is_some_and(is_bare_local) {
                    group.push(iter.next().expect("peeked statement exists"));
                }
                if group.len() >= 2 {
                    let mut names: Vec<AttributedName> = Vec::new();
                    for stmt in group {
                        let Statement::LocalAssignment(local) = stmt else {
                            unreachable!("bare-local group members are local assignments")
                        };
                        names.extend(local.names.into_items());
                    }
                    merged.push(Statement::LocalAssignment(Box::new(LocalAssignment {
                        span: sp(),
                        names: Punctuated::from_items(names),
                        exprs: None,
                        is_const: false,
                        is_exported: false,
                    })));
                } else {
                    let stmt = group.pop().expect("group holds the first statement");
                    merged.push(self.transform_statement(stmt));
                }
            } else {
                merged.push(self.transform_statement(first_stmt));
            }
        }

        // fuse `local a,b` + `a,b=X,Y` -> `local a,b=X,Y`
        let merged = fuse_bare_locals(merged);

        let last_stmt = block
            .last_stmt
            .map(|last| Box::new(self.transform_last_statement(*last)));

        Block {
            span: block.span,
            stmts: merged,
            last_stmt,
        }
    }
}

fn is_bare_local(stmt: &Statement) -> bool {
    matches!(
        stmt,
        Statement::LocalAssignment(local)
            if local.exprs.is_none() && !local.is_const && !local.is_exported
    )
}

fn fuse_bare_locals(stmts: Vec<Statement>) -> Vec<Statement> {
    let mut result: Vec<Statement> = Vec::with_capacity(stmts.len());
    let mut iter = stmts.into_iter().peekable();

    while let Some(stmt) = iter.next() {
        if let Statement::LocalAssignment(ref local) = stmt
            && local.exprs.is_none()
            && !local.is_exported
        {
            let local_names: Vec<CompactString> = local
                .names
                .iter()
                .map(|attributed| ident_name(&attributed.name).into())
                .collect();
            if let Some(Statement::Assignment(assign)) = iter.peek() {
                // EVERY target must be a bare name - filtering out field
                // targets and then comparing silently deleted `t.x = ...`
                // from `a, t.x = 1, 2`.
                let target_count = assign.targets.iter().count();
                let assign_names: Vec<CompactString> = assign
                    .targets
                    .iter()
                    .filter_map(|v| {
                        if let Var::Name(n) = v {
                            Some(ident_name(n).into())
                        } else {
                            None
                        }
                    })
                    .collect();
                // The RHS must not reference the declared names: in
                // `local f; f = function() f() end` the body's `f` is the
                // NEW binding; fused into `local f = function() ... end`
                // it would resolve to the outer scope.
                let name_set: FxHashSet<CompactString> = local_names.iter().cloned().collect();
                let rhs_uses_names = assign
                    .values
                    .iter()
                    .any(|value| references_any_in_expr(value, &name_set));
                if assign_names.len() == target_count
                    && assign_names == local_names
                    && !rhs_uses_names
                    && let Some(Statement::Assignment(assign)) = iter.next()
                {
                    let fused = Statement::LocalAssignment(Box::new(LocalAssignment {
                        span: local.span,
                        names: local.names.clone(),
                        exprs: Some(assign.values),
                        is_const: false,
                        is_exported: false,
                    }));
                    result.push(fused);
                    continue;
                }
            }
        }
        result.push(stmt);
    }

    result
}

/// Detects whether any `Var::Name` whose identifier is in `names` appears
/// anywhere in the visited subtree. Because it rides the `Visitor` framework,
/// the walk is exhaustive over every statement and expression variant -
/// nested blocks, loops, closure bodies, type casts, compound assignments,
/// and call arguments included. Both reads and assignment-target writes count,
/// since assignment targets and compound-assignment vars are `Var::Name` nodes
/// that `walk_statement` routes through `visit_var`.
struct ReferenceFinder<'a> {
    names: &'a FxHashSet<CompactString>,
    found: bool,
}

impl<'ast> Visitor<'ast> for ReferenceFinder<'_> {
    fn visit_var(&mut self, var: &'ast Var) {
        if self.found {
            return;
        }
        if let Var::Name(name) = var
            && self.names.contains(ident_name(name))
        {
            self.found = true;
            return;
        }
        self.walk_var(var);
    }

    fn visit_expression(&mut self, expr: &'ast Expression) {
        if self.found {
            return;
        }
        self.walk_expression(expr);
    }

    fn visit_statement(&mut self, stmt: &'ast Statement) {
        if self.found {
            return;
        }
        self.walk_statement(stmt);
    }
}

fn references_any_in_expr(expr: &Expression, names: &FxHashSet<CompactString>) -> bool {
    let mut finder = ReferenceFinder {
        names,
        found: false,
    };
    finder.visit_expression(expr);
    finder.found
}

#[cfg(test)]
fn references_any_in_block(block: &Block, names: &FxHashSet<CompactString>) -> bool {
    let mut finder = ReferenceFinder {
        names,
        found: false,
    };
    finder.visit_block(block);
    finder.found
}

#[cfg(test)]
mod tests {
    use super::*;

    fn apply(source: &str) -> String {
        apply_version(source, luck_token::LuaVersion::Lua54)
    }

    fn apply_version(source: &str, version: luck_token::LuaVersion) -> String {
        let result = luck_parser::parse(source, version);
        assert!(result.errors.is_empty(), "parse failed");
        let block = merge(result.block);
        luck_codegen::compact(&block, source)
    }

    #[test]
    fn merges_consecutive_locals() {
        let r = apply("local a = 1\nlocal b = 2\nlocal c = 3\nprint(a, b, c)\n");
        let local_count = r.matches("local").count();
        assert_eq!(local_count, 1, "Expected single local statement, got: {r}");
    }

    #[test]
    fn no_merge_across_non_local() {
        let r = apply("local a = 1\nprint(a)\nlocal b = 2\n");
        let local_count = r.matches("local").count();
        assert_eq!(local_count, 2, "Should have 2 separate locals, got: {r}");
    }

    #[test]
    fn no_merge_function_calls() {
        let r = apply("local x = f()\nlocal y = g()\nprint(x, y)\n");
        let local_count = r.matches("local").count();
        assert_eq!(local_count, 2, "Function calls should not merge, got: {r}");
    }

    #[test]
    fn no_merge_when_dependency_in_binary() {
        let r = apply("local a = 1\nlocal b = a + 1\nprint(b)\n");
        let local_count = r.matches("local").count();
        assert_eq!(
            local_count, 2,
            "Should not merge when rhs references prior local, got: {r}"
        );
    }

    #[test]
    fn merge_global_assignments() {
        let r = apply("a = 1\nb = 2\nc = 3\n");
        let equal_count = r.matches('=').count();
        assert_eq!(equal_count, 1, "Global assignments should merge, got: {r}");
    }

    fn names_of(items: &[&str]) -> FxHashSet<CompactString> {
        items.iter().map(|s| CompactString::from(*s)).collect()
    }

    fn parse_block(source: &str) -> Block {
        let result = luck_parser::parse(source, luck_token::LuaVersion::Luau);
        assert!(result.errors.is_empty(), "parse failed: {source}");
        result.block
    }

    fn first_expr_value(block: &Block) -> Expression {
        match &block.stmts[0] {
            Statement::LocalAssignment(local) => {
                local.exprs.as_ref().unwrap().iter().next().unwrap().clone()
            }
            other => panic!("expected local assignment, got {other:?}"),
        }
    }

    // Bug 1: a Luau type cast `... :: T` referencing a name must be detected.
    #[test]
    fn detects_reference_in_type_cast() {
        let block = parse_block("local y = x :: number\n");
        let expr = first_expr_value(&block);
        assert!(references_any_in_expr(&expr, &names_of(&["x"])));
    }

    // The type-cast reference must actually block a real merge.
    #[test]
    fn type_cast_dependency_blocks_merge() {
        let r = apply_version(
            "local a = 1\nlocal b = a :: number\nprint(b)\n",
            luck_token::LuaVersion::Luau,
        );
        let local_count = r.matches("local").count();
        assert_eq!(
            local_count, 2,
            "type cast referencing prior local must block merge, got: {r}"
        );
    }

    // Bug 2: a reference buried inside a nested block / if / loop must be detected.
    #[test]
    fn detects_reference_in_nested_block() {
        let block =
            parse_block("local y = function()\n  if true then\n    while x do end\n  end\nend\n");
        let expr = first_expr_value(&block);
        assert!(references_any_in_expr(&expr, &names_of(&["x"])));
    }

    // Bug 3: a reference appearing as a statement-level call ARGUMENT must be detected.
    #[test]
    fn detects_reference_in_call_argument() {
        // Statement-level call inside a closure: `foo(x)` references x as an arg.
        let block = parse_block("local y = function()\n  foo(x)\nend\n");
        let expr = first_expr_value(&block);
        assert!(
            references_any_in_expr(&expr, &names_of(&["x"])),
            "call argument reference must be detected"
        );
        // And the callee-only case still works (regression guard).
        let callee = parse_block("local y = function()\n  x()\nend\n");
        assert!(references_any_in_expr(
            &first_expr_value(&callee),
            &names_of(&["x"])
        ));
    }

    // Block-level statement call argument, exercised through references_any_in_block.
    #[test]
    fn detects_call_argument_at_block_level() {
        let block = parse_block("foo(x)\n");
        assert!(
            references_any_in_block(&block, &names_of(&["x"])),
            "statement-level call argument must be detected at block level"
        );
    }

    // Bug 4: a reference inside a closure body must be detected.
    #[test]
    fn detects_reference_in_closure_body() {
        let block = parse_block("local y = function() return x + 1 end\n");
        let expr = first_expr_value(&block);
        assert!(references_any_in_expr(&expr, &names_of(&["x"])));
    }

    // A new local declared inside a nested scope shadowing the name is NOT a
    // reference - declaration tokens are not Var::Name nodes.
    #[test]
    fn declaration_of_same_name_is_not_a_reference() {
        let block = parse_block("local y = function()\n  local x = 1\n  return 0\nend\n");
        let expr = first_expr_value(&block);
        assert!(
            !references_any_in_expr(&expr, &names_of(&["x"])),
            "a fresh declaration of the same name must not count as a reference"
        );
    }

    // Compound-assignment target and value are both references (Luau `+=`).
    #[test]
    fn detects_reference_in_compound_assignment() {
        let target = parse_block("local y = function()\n  x += 1\nend\n");
        assert!(references_any_in_expr(
            &first_expr_value(&target),
            &names_of(&["x"])
        ));
        let value = parse_block("local y = function()\n  z += x\nend\n");
        assert!(references_any_in_expr(
            &first_expr_value(&value),
            &names_of(&["x"])
        ));
    }
}
