use rustc_hash::FxHashSet;

use luck_ast::expr::*;
use luck_ast::shared::*;
use luck_ast::stmt::*;
use luck_ast::transform::AstTransform;
use luck_ast::visitor::Visitor;
use luck_token::{BinOp, CompactString, UnOp};

use crate::expr::{extract_boolean, ident_name, is_nil, is_pure_expression};
use crate::tokens::default_span as sp;

/// Remove unused locals, dead branches, and trivial statements, looping until fixed-point.
pub fn remove(mut block: Block) -> Block {
    loop {
        let (new_block, changed) = remove_unused_locals(block);
        block = new_block;
        if !changed {
            break;
        }
    }
    block
}

fn remove_unused_locals(block: Block) -> (Block, bool) {
    let referenced = collect_all_references(&block);
    let mut transform = DeadCodeTransform {
        referenced,
        changed: false,
    };
    let block = transform.transform_block(block);
    (block, transform.changed)
}

/// Whether a block contains a `break`/`continue` that binds to the loop
/// directly enclosing it. Nested loops capture their own exits and
/// function bodies are separate control-flow units, so neither is
/// descended into.
fn has_loop_exit(block: &Block) -> bool {
    if matches!(
        block.last_stmt.as_deref(),
        Some(LastStatement::Break(_) | LastStatement::Continue(_))
    ) {
        return true;
    }
    block.stmts.iter().any(stmt_has_loop_exit)
}

fn stmt_has_loop_exit(stmt: &Statement) -> bool {
    match stmt {
        Statement::Break(_) => true,
        Statement::DoBlock(do_block) => has_loop_exit(&do_block.block),
        Statement::IfStatement(if_stmt) => {
            has_loop_exit(&if_stmt.block)
                || if_stmt
                    .elseif_clauses
                    .iter()
                    .any(|clause| has_loop_exit(&clause.block))
                || if_stmt
                    .else_clause
                    .as_ref()
                    .is_some_and(|clause| has_loop_exit(&clause.block))
        }
        Statement::WhileLoop(_)
        | Statement::RepeatLoop(_)
        | Statement::NumericFor(_)
        | Statement::GenericFor(_)
        | Statement::FunctionDecl(_)
        | Statement::LocalFunction(_)
        | Statement::GlobalFunction(_)
        | Statement::Assignment(_)
        | Statement::FunctionCall(_)
        | Statement::LocalAssignment(_)
        | Statement::EmptyStatement(_)
        | Statement::Goto(_)
        | Statement::Label(_)
        | Statement::GlobalDeclaration(_)
        | Statement::GlobalStar(_)
        | Statement::CompoundAssignment(_)
        | Statement::TypeDeclaration(_)
        | Statement::Error(_) => false,
    }
}

fn collect_all_references(block: &Block) -> FxHashSet<CompactString> {
    let mut collector = ReferenceCollector {
        references: FxHashSet::default(),
    };
    collector.visit_block(block);
    collector.references
}

struct ReferenceCollector {
    references: FxHashSet<CompactString>,
}

impl<'ast> Visitor<'ast> for ReferenceCollector {
    fn visit_var(&mut self, var: &'ast Var) {
        if let Var::Name(name) = var {
            self.references.insert(ident_name(name).into());
        }
        self.walk_var(var);
    }

    fn visit_expression(&mut self, expr: &'ast Expression) {
        self.walk_expression(expr);
    }
}

/// The fused DCE rebuild: one traversal applies both the unused-local
/// removal (driven by the pre-collected `referenced` set) and the dead-
/// branch elimination. The two used to be separate full rebuilds; local
/// rewrites compose per-statement, and both `remove()`'s inner loop and
/// the pipeline's outer loop run to fixpoint, so interleaving them
/// reaches the same result in half the traversals.
struct DeadCodeTransform {
    referenced: FxHashSet<CompactString>,
    changed: bool,
}

fn is_const_truthy(expr: &Expression) -> Option<bool> {
    if let Some(b) = extract_boolean(expr) {
        return Some(b);
    }
    if is_nil(expr) {
        return Some(false);
    }
    if matches!(expr, Expression::Number(_) | Expression::StringLiteral(_)) {
        return Some(true);
    }
    None
}

impl AstTransform for DeadCodeTransform {
    fn transform_block(&mut self, block: Block) -> Block {
        let mut new_stmts: Vec<Statement> = Vec::new();
        for stmt in block.stmts {
            let stmt = self.transform_statement(stmt);
            // Unused-local removal first: a dropped/extracted statement
            // never reaches the branch checks below.
            let replacements = match simplify_dead_local(stmt, &self.referenced) {
                None => {
                    self.changed = true;
                    continue;
                }
                Some(replacements) => replacements,
            };
            for stmt in replacements {
                match &stmt {
                    Statement::WhileLoop(while_loop)
                        if is_const_truthy(&while_loop.condition) == Some(false) =>
                    {
                        self.changed = true;
                        continue;
                    }
                    Statement::IfStatement(if_stmt)
                        if is_const_truthy(&if_stmt.condition) == Some(false)
                            && if_stmt.elseif_clauses.is_empty()
                            && if_stmt.else_clause.is_none() =>
                    {
                        self.changed = true;
                        continue;
                    }
                    // Empty-body `if` can only go when evaluating the condition
                    // is side-effect free - `if f() then end` calls f.
                    Statement::IfStatement(if_stmt)
                        if if_stmt.block.stmts.is_empty()
                            && if_stmt.block.last_stmt.is_none()
                            && if_stmt.elseif_clauses.is_empty()
                            && if_stmt.else_clause.is_none()
                            && is_pure_expression(&if_stmt.condition, true) =>
                    {
                        self.changed = true;
                        continue;
                    }
                    // NOTE: `x = x` self-assignment is NOT removed. Without
                    // binding resolution we can't prove `x` is a local; for a
                    // global under a metatabled environment the statement fires
                    // __index + __newindex.
                    _ => new_stmts.push(stmt),
                }
            }
        }

        let last_stmt = block
            .last_stmt
            .map(|last| Box::new(self.transform_last_statement(*last)));

        Block {
            span: block.span,
            stmts: new_stmts,
            last_stmt,
        }
    }

    fn walk_function_body(&mut self, mut body: FunctionBody) -> FunctionBody {
        let new_block = self.transform_block(body.block);
        let new_block = if let Some(last) = &new_block.last_stmt {
            match last.as_ref() {
                LastStatement::Return(ret) => {
                    // Only a bare `return` is removable. `return nil`
                    // returns ONE value - `select('#', f())` observes the
                    // difference.
                    let returns: Vec<_> = ret.exprs.iter().collect();
                    if returns.is_empty() {
                        self.changed = true;
                        Block {
                            span: new_block.span,
                            stmts: new_block.stmts,
                            last_stmt: None,
                        }
                    } else {
                        new_block
                    }
                }
                _ => new_block,
            }
        } else {
            new_block
        };
        body.block = new_block;
        body
    }

    fn transform_statement(&mut self, stmt: Statement) -> Statement {
        match stmt {
            Statement::IfStatement(mut if_stmt) => {
                if let Some(truthy) = is_const_truthy(&if_stmt.condition) {
                    if truthy {
                        self.changed = true;
                        let new_block = self.transform_block(if_stmt.block);
                        return Statement::DoBlock(Box::new(DoBlock {
                            span: sp(),
                            block: new_block,
                        }));
                    } else if !if_stmt.elseif_clauses.is_empty() {
                        // Elseif arms must be promoted before the else is
                        // considered: taking the else first would drop them.
                        self.changed = true;
                        let else_clause = if_stmt.else_clause.take();
                        let mut clauses = if_stmt.elseif_clauses.into_iter();
                        let first_elseif = clauses.next().expect("checked non-empty above");
                        let new_cond = self.transform_expression(first_elseif.condition);
                        let new_block = self.transform_block(first_elseif.block);
                        let new_if = IfStatement {
                            span: sp(),
                            condition: new_cond,
                            block: new_block,
                            elseif_clauses: clauses.collect(),
                            else_clause,
                        };
                        return self.transform_statement(Statement::IfStatement(Box::new(new_if)));
                    } else if let Some(else_clause) = if_stmt.else_clause.take() {
                        self.changed = true;
                        let new_block = self.transform_block(else_clause.block);
                        return Statement::DoBlock(Box::new(DoBlock {
                            span: sp(),
                            block: new_block,
                        }));
                    }
                }

                if if_stmt.block.stmts.is_empty()
                    && if_stmt.block.last_stmt.is_none()
                    && if_stmt.elseif_clauses.is_empty()
                    && let Some(else_clause) = if_stmt.else_clause.take()
                {
                    self.changed = true;
                    let negated = negate_expression(if_stmt.condition);
                    let new_block = self.transform_block(else_clause.block);
                    let new_if = IfStatement {
                        span: sp(),
                        condition: negated,
                        block: new_block,
                        elseif_clauses: Vec::new(),
                        else_clause: None,
                    };
                    return Statement::IfStatement(Box::new(new_if));
                }

                if matches!(&if_stmt.condition, Expression::UnaryOp(unop) if matches!(unop.op, UnOp::Not))
                    && if_stmt.elseif_clauses.is_empty()
                    && let Some(else_clause) = if_stmt.else_clause.take()
                {
                    self.changed = true;
                    let Expression::UnaryOp(unop) = if_stmt.condition else {
                        unreachable!("condition matched UnaryOp above")
                    };
                    let new_if = IfStatement {
                        span: sp(),
                        condition: unop.operand,
                        block: self.transform_block(else_clause.block),
                        elseif_clauses: Vec::new(),
                        else_clause: Some(ElseClause {
                            span: sp(),
                            block: self.transform_block(if_stmt.block),
                        }),
                    };
                    return Statement::IfStatement(Box::new(new_if));
                }

                self.walk_statement(Statement::IfStatement(if_stmt))
            }
            // A body-level break/continue binds to THIS repeat; in a
            // do-block it would rebind to an outer loop (or be invalid),
            // so the rewrite only fires when none exists.
            Statement::RepeatLoop(repeat_loop)
                if is_const_truthy(&repeat_loop.condition) == Some(true)
                    && !has_loop_exit(&repeat_loop.block) =>
            {
                self.changed = true;
                let new_block = self.transform_block(repeat_loop.block);
                Statement::DoBlock(Box::new(DoBlock {
                    span: sp(),
                    block: new_block,
                }))
            }
            // step=1 is the default; stripping it saves bytes
            Statement::NumericFor(mut numeric_for) => {
                if let Some(step) = &numeric_for.step
                    && let Expression::Number(literal) = step
                    && literal.text == "1"
                {
                    self.changed = true;
                    numeric_for.step = None;
                }
                self.walk_statement(Statement::NumericFor(numeric_for))
            }
            // `local x = nil` -> `local x` (nil is the default)
            Statement::LocalAssignment(mut local) => {
                // Const declarations keep their mandatory initializer.
                let single_nil = !local.is_const
                    && local.names.len() == 1
                    && local.exprs.as_ref().is_some_and(|exprs| {
                        exprs.len() == 1 && is_nil(exprs.first().expect("len checked above"))
                    });
                if single_nil {
                    self.changed = true;
                    local.span = sp();
                    local.exprs = None;
                    return Statement::LocalAssignment(local);
                }
                self.walk_statement(Statement::LocalAssignment(local))
            }
            other => self.walk_statement(other),
        }
    }

    fn transform_expression(&mut self, expr: Expression) -> Expression {
        let expr = self.walk_expression(expr);
        // `cond and X or X` -> `X` when both branches identical and cond is pure
        if let Expression::BinaryOp(ref outer) = expr
            && matches!(outer.op, BinOp::Or)
            && let Expression::BinaryOp(ref inner) = outer.left
            && matches!(inner.op, BinOp::And)
            && inner.right == outer.right
            && is_pure_expression(&inner.left, true)
            // X must be pure too: a falsy-returning CALL evaluates twice
            // (truncated) in the original but once here.
            && is_pure_expression(&outer.right, true)
        {
            self.changed = true;
            let Expression::BinaryOp(outer) = expr else {
                unreachable!("matched BinaryOp above")
            };
            return outer.right;
        }
        expr
    }
}

fn negate_expression(expr: Expression) -> Expression {
    // Only wrap in `not` - never invert comparison operators, as that changes
    // which metamethod is called (__lt vs __le, etc.)
    Expression::UnaryOp(Box::new(UnaryOp {
        span: sp(),
        op: UnOp::Not,
        operand: expr,
    }))
}

enum DeadLocalAction {
    Keep,
    Remove,
    ExtractCalls,
}

fn classify_dead_local(stmt: &Statement, referenced: &FxHashSet<CompactString>) -> DeadLocalAction {
    match stmt {
        Statement::LocalAssignment(local) => {
            // `<close>` runs __close at scope exit and `<const>` affects
            // validity - an attributed local is never dead.
            if local
                .names
                .iter()
                .any(|attributed| attributed.attrib.is_some())
            {
                return DeadLocalAction::Keep;
            }
            let all_unused = local
                .names
                .iter()
                .all(|n| !referenced.contains(ident_name(&n.name)));
            if !all_unused {
                return DeadLocalAction::Keep;
            }
            match &local.exprs {
                None => DeadLocalAction::Remove,
                Some(exprs) => {
                    let expr_list: Vec<_> = exprs.iter().collect();
                    if expr_list.is_empty() || expr_list.iter().all(|e| is_pure_expression(e, true))
                    {
                        DeadLocalAction::Remove
                    } else if expr_list.iter().all(|e| {
                        is_pure_expression(e, true) || matches!(e, Expression::FunctionCall(_))
                    }) {
                        DeadLocalAction::ExtractCalls
                    } else {
                        DeadLocalAction::Keep
                    }
                }
            }
        }
        Statement::LocalFunction(local_func) => {
            let name = ident_name(&local_func.name);
            if referenced.contains(name) {
                DeadLocalAction::Keep
            } else {
                DeadLocalAction::Remove
            }
        }
        Statement::DoBlock(do_block) => {
            if do_block.block.stmts.is_empty() && do_block.block.last_stmt.is_none() {
                DeadLocalAction::Remove
            } else {
                DeadLocalAction::Keep
            }
        }
        _ => DeadLocalAction::Keep,
    }
}

/// Returns None to remove the statement, Some(stmts) to replace it.
fn simplify_dead_local(
    stmt: Statement,
    referenced: &FxHashSet<CompactString>,
) -> Option<Vec<Statement>> {
    match classify_dead_local(&stmt, referenced) {
        DeadLocalAction::Keep => Some(vec![stmt]),
        DeadLocalAction::Remove => None,
        DeadLocalAction::ExtractCalls => {
            if let Statement::LocalAssignment(local) = stmt {
                if let Some(exprs) = local.exprs {
                    let kept: Vec<_> = exprs
                        .into_items()
                        .into_iter()
                        .filter_map(|expr| {
                            if let Expression::FunctionCall(call) = expr {
                                Some(Statement::FunctionCall(Box::new(FunctionCallStmt {
                                    span: call.span,
                                    call: *call,
                                })))
                            } else {
                                None
                            }
                        })
                        .collect();
                    if kept.is_empty() { None } else { Some(kept) }
                } else {
                    None
                }
            } else {
                None
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn apply(source: &str) -> String {
        let result = luck_parser::parse(source, luck_token::LuaVersion::Lua54);
        assert!(result.errors.is_empty(), "parse failed");
        let block = remove(result.block);
        luck_codegen::compact(&block, source)
    }

    #[test]
    fn removes_unused_local_literal() {
        let r = apply("local unused = 42\nlocal used = 1\nreturn used\n");
        assert!(!r.contains("unused"), "Unused local not removed: {r}");
        assert!(r.contains("used"), "Used local was removed: {r}");
    }

    #[test]
    fn removes_unused_local_function() {
        let r = apply("local function unused() return 1 end\nreturn 2\n");
        assert!(!r.contains("unused"), "Unused function not removed: {r}");
    }

    #[test]
    fn removes_if_false() {
        let r = apply("if false then print(1) end\n");
        assert!(!r.contains("print"), "if false branch not removed: {r}");
    }

    #[test]
    fn if_false_promotes_elseif_and_keeps_else() {
        let r = apply("if false then print(1) elseif x then print(2) else print(3) end\n");
        assert!(!r.contains("print(1)"), "dead branch must go: {r}");
        assert!(r.contains("print(2)"), "elseif branch must survive: {r}");
        assert!(r.contains("print(3)"), "else branch must survive: {r}");
        assert!(r.contains("if x"), "elseif must be promoted to if: {r}");
    }

    #[test]
    fn if_false_chained_elseifs_collapse_to_else() {
        let r = apply("if false then print(1) elseif false then print(2) else print(3) end\n");
        assert!(!r.contains("print(1)"), "dead branch must go: {r}");
        assert!(!r.contains("print(2)"), "dead elseif must go: {r}");
        assert!(r.contains("print(3)"), "else branch must survive: {r}");
    }

    #[test]
    fn unwraps_if_true() {
        let r = apply("if true then print(1) end\n");
        assert!(r.contains("print"), "Body should remain: {r}");
    }

    #[test]
    fn removes_while_false() {
        let r = apply("while false do print(1) end\n");
        assert!(!r.contains("print"), "while false body not removed: {r}");
    }

    #[test]
    fn local_nil_simplifies() {
        let r = apply("local x = nil\nreturn x\n");
        assert!(
            !r.contains("= nil") && !r.contains("=nil"),
            "Should remove = nil: {r}"
        );
        assert!(r.contains("local"), "Should keep declaration: {r}");
    }

    #[test]
    fn global_self_assignment_kept() {
        // `x = x` on a global fires __index + __newindex under a
        // metatabled environment - removal is only sound for proven
        // locals, which this pass can't prove without binding info.
        let r = apply("x = x\nreturn 1\n");
        assert!(
            r.contains("x=x") || r.contains("x = x"),
            "global self-assignment must be preserved: {r}"
        );
    }

    #[test]
    fn repeat_until_true_without_break_becomes_do_block() {
        let r = apply("repeat print(1) until true\n");
        assert!(r.contains("do"), "repeat->do rewrite expected: {r}");
        assert!(!r.contains("repeat"), "repeat should be gone: {r}");
    }

    #[test]
    fn repeat_until_true_with_break_is_kept() {
        // `break` binds to the repeat; a do-block would rebind it to an
        // outer loop (or make it invalid).
        let r = apply("while f() do repeat if g() then break end until true end\n");
        assert!(r.contains("repeat"), "repeat with break must be kept: {r}");
        let r = apply("repeat break until true\n");
        assert!(r.contains("repeat"), "repeat with break must be kept: {r}");
    }
}
