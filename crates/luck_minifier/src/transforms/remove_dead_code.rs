use core::mem;

use luck_ast::{
    BinaryOp, Block, DoBlock, ElseClause, Expression, FunctionBody, IfStatement, LastStatement,
    LocalAssignment, Punctuated, Statement, Transform, UnaryOp, Var, Visitor,
};
use luck_token::{BinOp, CompactString, LuaVersion, Span, UnOp};
use rustc_hash::FxHashSet;

use crate::binding::{has_attributed_or_env_name, identifier_name, is_env_binding};
use crate::expr::{extract_boolean, is_nil, is_pure_expr};

/// Removes unused locals, dead branches, and trivial statements, looping to a
/// fixpoint.
pub(crate) fn remove(mut block: Block, version: LuaVersion) -> Block {
    loop {
        let mut remover = DeadCodeRemover {
            referenced_names: collect_all_references(&block),
            version,
            has_changed: false,
        };
        block = remover.transform_block(block);
        if !remover.has_changed {
            return block;
        }
    }
}

/// Whether a block contains a `break`/`continue` that binds to the loop
/// directly enclosing it.
///
/// Nested loops and function bodies aren't searched, since their exits can't
/// reach this loop.
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
        // Lua 5.2+
        Statement::Break(_) => true,
        Statement::DoBlock(do_block) => has_loop_exit(&do_block.block),
        Statement::IfStatement(if_stmt) => {
            has_loop_exit(&if_stmt.block)
                || if_stmt.elseif_clauses.iter().any(|clause| has_loop_exit(&clause.block))
                || if_stmt.else_clause.as_ref().is_some_and(|clause| has_loop_exit(&clause.block))
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
    let mut collector = ReferenceCollector { references: FxHashSet::default() };
    collector.visit_block(block);
    collector.references
}

struct ReferenceCollector {
    references: FxHashSet<CompactString>,
}

impl<'ast> Visitor<'ast> for ReferenceCollector {
    fn visit_var(&mut self, var: &'ast Var) {
        if let Var::Name(name) = var {
            self.references.insert(identifier_name(name).into());
        }
        self.walk_var(var);
    }

    // `function M.f()` reads `M` and `function f()` writes `f`, and neither
    // root is a `Var`.
    fn visit_stmt(&mut self, stmt: &'ast Statement) {
        if let Statement::FunctionDecl(function_decl) = stmt
            && let Some(root) = function_decl.name.names.first()
        {
            self.references.insert(identifier_name(root).into());
        }
        self.walk_stmt(stmt);
    }
}

/// Removes unused locals and dead branches in one traversal.
///
/// The two rewrites compose per statement, and both `remove()`'s loop and
/// the pipeline's outer loop run to a fixpoint, so interleaving them reaches
/// the same result as separate rebuilds in half the traversals.
struct DeadCodeRemover {
    referenced_names: FxHashSet<CompactString>,
    version: LuaVersion,
    has_changed: bool,
}

fn constant_truthiness(expr: &Expression) -> Option<bool> {
    if let Some(value) = extract_boolean(expr) {
        return Some(value);
    }
    if is_nil(expr) {
        return Some(false);
    }
    if matches!(expr, Expression::Number(_) | Expression::Integer(_) | Expression::StringLiteral(_))
    {
        return Some(true);
    }
    None
}

/// Whether a rewritten statement can never run or has nothing left to run.
fn is_dead_branch(stmt: &Statement) -> bool {
    match stmt {
        Statement::WhileLoop(while_loop) => {
            constant_truthiness(&while_loop.condition) == Some(false)
        }
        Statement::IfStatement(if_stmt) => {
            // An empty-body `if` can only go when evaluating the condition is
            // side-effect free: `if f() then end` calls f.
            let has_no_effect = if_stmt.block.stmts.is_empty()
                && if_stmt.block.last_stmt.is_none()
                && is_pure_expr(&if_stmt.condition);
            if_stmt.elseif_clauses.is_empty()
                && if_stmt.else_clause.is_none()
                && (constant_truthiness(&if_stmt.condition) == Some(false) || has_no_effect)
        }
        // `x = x` stays. Without binding resolution this pass can't prove `x`
        // is a local, and on a global under a metatabled environment it fires
        // __index and __newindex.
        Statement::Assignment(_)
        | Statement::FunctionCall(_)
        | Statement::DoBlock(_)
        | Statement::RepeatLoop(_)
        | Statement::NumericFor(_)
        | Statement::GenericFor(_)
        | Statement::FunctionDecl(_)
        | Statement::LocalFunction(_)
        | Statement::LocalAssignment(_)
        | Statement::EmptyStatement(_)
        | Statement::Goto(_)
        | Statement::Label(_)
        | Statement::GlobalDeclaration(_)
        | Statement::GlobalFunction(_)
        | Statement::GlobalStar(_)
        | Statement::Break(_)
        | Statement::CompoundAssignment(_)
        | Statement::TypeDeclaration(_)
        | Statement::Error(_) => false,
    }
}

/// Whether `outer` is `cond and X or X` with `cond` and `X` both pure.
fn is_redundant_ternary(outer: &BinaryOp) -> bool {
    matches!(
        &outer.left,
        Expression::BinaryOp(inner)
            if matches!(outer.op, BinOp::Or)
                && matches!(inner.op, BinOp::And)
                && inner.right == outer.right
                && is_pure_expr(&inner.left)
                // X must be pure too: a call that returns a falsy value runs
                // twice in the original but once here.
                && is_pure_expr(&outer.right)
    )
}

impl DeadCodeRemover {
    fn simplify_if(&mut self, mut if_stmt: IfStatement) -> Statement {
        if let Some(is_truthy) = constant_truthiness(&if_stmt.condition) {
            if is_truthy {
                self.has_changed = true;
                let new_block = self.transform_block(if_stmt.block);
                return Statement::DoBlock(Box::new(DoBlock {
                    span: Span::default(),
                    block: new_block,
                }));
            }
            // Elseif arms must be promoted before the else is considered:
            // taking the else first would drop them.
            let mut clauses = mem::take(&mut if_stmt.elseif_clauses).into_iter();
            if let Some(first_elseif) = clauses.next() {
                self.has_changed = true;
                let else_clause = if_stmt.else_clause.take();
                let new_condition = self.transform_expr(first_elseif.condition);
                let new_block = self.transform_block(first_elseif.block);
                let new_if = IfStatement {
                    span: Span::default(),
                    condition: new_condition,
                    block: new_block,
                    elseif_clauses: clauses.collect(),
                    else_clause,
                };
                return self.transform_stmt(Statement::IfStatement(Box::new(new_if)));
            }
            if let Some(else_clause) = if_stmt.else_clause.take() {
                self.has_changed = true;
                let new_block = self.transform_block(else_clause.block);
                return Statement::DoBlock(Box::new(DoBlock {
                    span: Span::default(),
                    block: new_block,
                }));
            }
        }

        if if_stmt.block.stmts.is_empty()
            && if_stmt.block.last_stmt.is_none()
            && if_stmt.elseif_clauses.is_empty()
            && let Some(else_clause) = if_stmt.else_clause.take()
        {
            self.has_changed = true;
            let negated = negate_expr(if_stmt.condition);
            let new_block = self.transform_block(else_clause.block);
            let new_if = IfStatement {
                span: Span::default(),
                condition: negated,
                block: new_block,
                elseif_clauses: Vec::new(),
                else_clause: None,
            };
            return Statement::IfStatement(Box::new(new_if));
        }

        match if_stmt {
            IfStatement {
                condition: Expression::UnaryOp(unop),
                block,
                elseif_clauses,
                else_clause: Some(else_clause),
                ..
            } if matches!(unop.op, UnOp::Not) && elseif_clauses.is_empty() => {
                self.has_changed = true;
                let new_if = IfStatement {
                    span: Span::default(),
                    condition: unop.operand,
                    block: self.transform_block(else_clause.block),
                    elseif_clauses: Vec::new(),
                    else_clause: Some(ElseClause {
                        span: Span::default(),
                        block: self.transform_block(block),
                    }),
                };
                Statement::IfStatement(Box::new(new_if))
            }
            other => self.walk_stmt(Statement::IfStatement(Box::new(other))),
        }
    }
}

impl Transform for DeadCodeRemover {
    fn transform_block(&mut self, block: Block) -> Block {
        let mut new_stmts: Vec<Statement> = Vec::new();
        for stmt in block.stmts {
            let transformed = self.transform_stmt(stmt);
            // Unused-local removal first: a dropped/extracted statement
            // never reaches the branch checks below.
            match classify_dead_local(&transformed, &self.referenced_names, self.version) {
                DeadLocalAction::Keep => {
                    if is_dead_branch(&transformed) {
                        self.has_changed = true;
                    } else {
                        new_stmts.push(transformed);
                    }
                }
                DeadLocalAction::Remove => self.has_changed = true,
                DeadLocalAction::ExtractCalls => {
                    if let Statement::LocalAssignment(local) = transformed {
                        new_stmts.extend(call_stmts(*local));
                    }
                }
            }
        }

        let last_stmt = block.last_stmt.map(|mut last| {
            *last = self.transform_last_stmt(*last);
            last
        });

        Block { span: block.span, stmts: new_stmts, last_stmt }
    }

    fn transform_function_body(&mut self, mut body: FunctionBody) -> FunctionBody {
        body.block = self.transform_block(body.block);
        // Only a bare `return` is removable. `return nil` returns one value,
        // and `select('#', f())` observes the difference.
        if let Some(LastStatement::Return(return_stmt)) = body.block.last_stmt.as_deref()
            && return_stmt.exprs.is_empty()
        {
            self.has_changed = true;
            body.block.last_stmt = None;
        }
        body
    }

    fn transform_stmt(&mut self, stmt: Statement) -> Statement {
        match stmt {
            Statement::IfStatement(if_stmt) => self.simplify_if(*if_stmt),
            // `repeat ... until true` runs once, so it becomes a `do` block
            // unless its body has its own `break` or `continue`, which a `do`
            // block would rebind to an outer loop (or make invalid).
            Statement::RepeatLoop(repeat_loop)
                if constant_truthiness(&repeat_loop.condition) == Some(true)
                    && !has_loop_exit(&repeat_loop.block) =>
            {
                self.has_changed = true;
                let new_block = self.transform_block(repeat_loop.block);
                Statement::DoBlock(Box::new(DoBlock { span: Span::default(), block: new_block }))
            }
            // A step of 1 is the default.
            Statement::NumericFor(mut numeric_for) => {
                if let Some(step) = &numeric_for.step
                    && let Expression::Number(literal) = step
                    && literal.text == "1"
                {
                    self.has_changed = true;
                    numeric_for.step = None;
                }
                self.walk_stmt(Statement::NumericFor(numeric_for))
            }
            // `local x = nil` becomes `local x`, since nil is the default.
            Statement::LocalAssignment(mut local) => {
                // Const declarations keep their mandatory initializer.
                let is_single_nil = !local.is_const
                    && local.names.len() == 1
                    && local.exprs.as_ref().is_some_and(
                        |exprs| matches!(exprs.items.as_slice(), [value] if is_nil(value)),
                    );
                if is_single_nil {
                    self.has_changed = true;
                    local.span = Span::default();
                    local.exprs = None;
                    return Statement::LocalAssignment(local);
                }
                self.walk_stmt(Statement::LocalAssignment(local))
            }
            other @ (Statement::Assignment(_)
            | Statement::FunctionCall(_)
            | Statement::DoBlock(_)
            | Statement::WhileLoop(_)
            | Statement::RepeatLoop(_)
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
            | Statement::Error(_)) => self.walk_stmt(other),
        }
    }

    fn transform_expr(&mut self, expr: Expression) -> Expression {
        match self.walk_expr(expr) {
            // `cond and X or X` folds to `X` when both branches are identical
            // and the condition is pure.
            Expression::BinaryOp(outer) if is_redundant_ternary(&outer) => {
                self.has_changed = true;
                outer.right
            }
            other @ (Expression::Nil(_)
            | Expression::False(_)
            | Expression::True(_)
            | Expression::Number(_)
            | Expression::Integer(_)
            | Expression::StringLiteral(_)
            | Expression::VarArg(_)
            | Expression::FunctionDef(_)
            | Expression::Var(_)
            | Expression::FunctionCall(_)
            | Expression::Parenthesized(_)
            | Expression::TableConstructor(_)
            | Expression::BinaryOp(_)
            | Expression::UnaryOp(_)
            | Expression::IfExpression(_)
            | Expression::InterpolatedString(_)
            | Expression::TypeCast(_)
            | Expression::TypeInstantiation(_)
            | Expression::Error(_)) => other,
        }
    }
}

fn negate_expr(expr: Expression) -> Expression {
    // Only wrap in `not`. Inverting a comparison operator would change which
    // metamethod is called, __lt instead of __le and so on.
    Expression::UnaryOp(Box::new(UnaryOp { span: Span::default(), op: UnOp::Not, operand: expr }))
}

enum DeadLocalAction {
    Keep,
    Remove,
    /// Every initializer is pure or a call, so only the calls stay.
    ExtractCalls,
}

fn classify_dead_local(
    stmt: &Statement,
    referenced_names: &FxHashSet<CompactString>,
    version: LuaVersion,
) -> DeadLocalAction {
    match stmt {
        Statement::LocalAssignment(local) => {
            // Exports, attributed names, and a 5.2+ `_ENV` are never dead.
            if local.is_exported || has_attributed_or_env_name(local, version) {
                return DeadLocalAction::Keep;
            }
            let is_unused = local
                .names
                .iter()
                .all(|attributed| !referenced_names.contains(identifier_name(&attributed.name)));
            if !is_unused {
                return DeadLocalAction::Keep;
            }
            let Some(exprs) = &local.exprs else {
                return DeadLocalAction::Remove;
            };
            if exprs.iter().all(is_pure_expr) {
                DeadLocalAction::Remove
            } else if exprs
                .iter()
                .all(|expr| is_pure_expr(expr) || matches!(expr, Expression::FunctionCall(_)))
            {
                DeadLocalAction::ExtractCalls
            } else {
                DeadLocalAction::Keep
            }
        }
        Statement::LocalFunction(local_function) => {
            if local_function.is_exported
                || is_env_binding(identifier_name(&local_function.name), version)
            {
                return DeadLocalAction::Keep;
            }
            let name = identifier_name(&local_function.name);
            if referenced_names.contains(name) {
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
        Statement::Assignment(_)
        | Statement::FunctionCall(_)
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
        | Statement::Error(_) => DeadLocalAction::Keep,
    }
}

/// The calls among an unused local's initializers, as call statements.
fn call_stmts(local: LocalAssignment) -> impl Iterator<Item = Statement> {
    local.exprs.into_iter().flat_map(Punctuated::into_items).filter_map(|expr| {
        if let Expression::FunctionCall(call) = expr {
            Some(Statement::FunctionCall(call))
        } else {
            None
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixtures::run_pass;

    fn apply(source: &str) -> String {
        run_pass(source, LuaVersion::Lua54, |block| remove(block, LuaVersion::Lua54))
    }

    #[test]
    fn removes_unused_local_literal() {
        let result = apply("local unused = 42\nlocal used = 1\nreturn used\n");
        assert!(!result.contains("unused"), "Unused local not removed: {result}");
        assert!(result.contains("used"), "Used local was removed: {result}");
    }

    #[test]
    fn keeps_unused_env_only_where_it_redirects_globals() {
        let source = "local _ENV = {}\nreturn 1\n";
        let kept = apply(source);
        assert!(kept.contains("_ENV"), "5.4 must pin _ENV: {kept}");

        for version in [LuaVersion::Lua51, LuaVersion::Luau] {
            let removed = run_pass(source, version, |block| remove(block, version));
            assert!(
                !removed.contains("_ENV"),
                "{version:?} binds _ENV like any other local: {removed}"
            );
        }
    }

    #[test]
    fn keeps_local_rooting_a_function_declaration() {
        for source in [
            "local M = {}\nfunction M.foo() end\nprint(\"ok\")\n",
            "local M = {}\nfunction M:foo() end\nprint(\"ok\")\n",
            "local M = {}\nfunction M.a.b() end\nprint(\"ok\")\n",
            "local M = {}\ndo function M.foo() end end\nprint(\"ok\")\n",
            "local M\nfunction M() end\nprint(\"ok\")\n",
        ] {
            let result = apply(source);
            assert!(result.contains("local M"), "{source:?} must keep `local M`: {result}");
        }
    }

    #[test]
    fn removes_unused_local_named_like_a_function_declaration_field() {
        let result = apply("local foo = {}\nfunction M.foo() end\n");
        assert!(!result.contains("local foo"), "only the root is a reference: {result}");
    }

    #[test]
    fn removes_unused_local_function() {
        let result = apply("local function unused() return 1 end\nreturn 2\n");
        assert!(!result.contains("unused"), "Unused function not removed: {result}");
    }

    #[test]
    fn removes_if_false() {
        let result = apply("if false then print(1) end\n");
        assert!(!result.contains("print"), "if false branch not removed: {result}");
    }

    #[test]
    fn if_false_promotes_elseif_and_keeps_else() {
        let result = apply("if false then print(1) elseif x then print(2) else print(3) end\n");
        assert!(!result.contains("print(1)"), "dead branch must go: {result}");
        assert!(result.contains("print(2)"), "elseif branch must survive: {result}");
        assert!(result.contains("print(3)"), "else branch must survive: {result}");
        assert!(result.contains("if x"), "elseif must be promoted to if: {result}");
    }

    #[test]
    fn if_false_chained_elseifs_collapse_to_else() {
        let result = apply("if false then print(1) elseif false then print(2) else print(3) end\n");
        assert!(!result.contains("print(1)"), "dead branch must go: {result}");
        assert!(!result.contains("print(2)"), "dead elseif must go: {result}");
        assert!(result.contains("print(3)"), "else branch must survive: {result}");
    }

    #[test]
    fn unwraps_if_true() {
        let result = apply("if true then print(1) end\n");
        assert!(result.contains("print"), "Body should remain: {result}");
    }

    #[test]
    fn removes_while_false() {
        let result = apply("while false do print(1) end\n");
        assert!(!result.contains("print"), "while false body not removed: {result}");
    }

    #[test]
    fn drops_nil_initializer() {
        let result = apply("local x = nil\nreturn x\n");
        assert!(
            !result.contains("= nil") && !result.contains("=nil"),
            "Should remove = nil: {result}"
        );
        assert!(result.contains("local"), "Should keep declaration: {result}");
    }

    #[test]
    fn keeps_global_self_assignment() {
        // `x = x` on a global fires __index and __newindex under a metatabled
        // environment. Removal is only sound for proven locals, and this pass
        // has no binding info.
        let result = apply("x = x\nreturn 1\n");
        assert!(
            result.contains("x=x") || result.contains("x = x"),
            "global self-assignment must be preserved: {result}"
        );
    }

    #[test]
    fn repeat_until_true_without_break_becomes_do_block() {
        let result = apply("repeat print(1) until true\n");
        assert!(result.contains("do"), "repeat->do rewrite expected: {result}");
        assert!(!result.contains("repeat"), "repeat should be gone: {result}");
    }

    #[test]
    fn repeat_until_true_with_break_is_kept() {
        // `break` binds to the repeat; a do-block would rebind it to an
        // outer loop (or make it invalid).
        let nested = apply("while f() do repeat if g() then break end until true end\n");
        assert!(nested.contains("repeat"), "repeat with break must be kept: {nested}");
        let bare = apply("repeat break until true\n");
        assert!(bare.contains("repeat"), "repeat with break must be kept: {bare}");
    }
}
