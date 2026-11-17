//! Rewrites of control flow: dead and constant branches, loops that run once
//! or never, nested `if`s, and `if ... return` runs.

use core::{iter, mem};

use luck_allocator::{Allocator, Box, List};
use luck_ast::{
    BinaryLink, BinaryOp, Block, DoBlock, ElseClause, Expression, IfStatement, LastStatement,
    NumericFor, OneOrMore, Statement, UnaryOp, VisitorMut as _, is_multi_value,
};
use luck_token::{BinOp, UnOp};

use super::Compressor;
use crate::expr::{
    binds_looser_than_unary, constant_truthiness, ends_in_if_expr, fits_at_depth, is_always_truthy,
    is_pure_expr, is_same_pure_expr, parenthesize, truncate_multi_value,
};

pub(super) fn is_empty_block(block: &Block<'_>) -> bool {
    block.stmts.is_empty() && block.last_stmt.is_none()
}

/// A step of 1 is the default.
pub(super) fn has_default_step(numeric_for: &NumericFor<'_>) -> bool {
    matches!(&numeric_for.step, Some(Expression::Number(literal)) if literal.text == "1")
}

/// Only a bare `return` is removable. `return nil` returns one value, and
/// `select('#', f())` observes the difference.
pub(super) fn ends_in_bare_return(block: &Block<'_>) -> bool {
    matches!(&block.last_stmt, Some(LastStatement::Return(return_stmt)) if return_stmt.exprs.is_empty())
}

fn has_no_else(if_stmt: &IfStatement<'_>) -> bool {
    if_stmt.elseif_clauses.is_empty() && if_stmt.else_clause.is_none()
}

/// Nested loops and function bodies aren't searched, since their exits can't
/// reach this loop.
fn has_loop_exit(block: &Block<'_>) -> bool {
    if matches!(block.last_stmt, Some(LastStatement::Break(_) | LastStatement::Continue(_))) {
        return true;
    }
    block.stmts.iter().any(|stmt| match stmt {
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
    })
}

fn do_block<'ast>(allocator: &'ast Allocator, block: Block<'ast>) -> Statement<'ast> {
    let span = block.span;
    Statement::DoBlock(Box::new_in(DoBlock { span, block }, allocator))
}

/// Only wraps in `not`. Inverting a comparison operator would change which
/// metamethod is called, `__lt` instead of `__le` and so on.
fn negate<'ast>(allocator: &'ast Allocator, expr: Expression<'ast>) -> Expression<'ast> {
    let needs_parens =
        matches!(&expr, Expression::BinaryOp(binary_op) if binds_looser_than_unary(binary_op));
    let span = expr.span();
    let operand = if needs_parens { parenthesize(allocator, expr) } else { expr };
    Expression::UnaryOp(Box::new_in(UnaryOp { span, op: UnOp::Not, operand }, allocator))
}

/// `cond and X or X`, which a run of `or`s starts with when its first two
/// operands are that.
fn is_redundant_ternary(outer: &BinaryOp<'_>) -> bool {
    let Expression::BinaryOp(inner) = &outer.first else {
        return false;
    };
    let alternative = &outer.links.first().operand;
    let conditions = inner.links.split_last().map_or(&[][..], |(_, before_last)| before_last);
    outer.links.first().op == BinOp::Or
        && inner.links.first().op == BinOp::And
        && is_same_pure_expr(&inner.links.last().operand, alternative)
        && is_pure_expr(&inner.first)
        && conditions.iter().all(|link| is_pure_expr(&link.operand))
        // X must be pure too: a call that returns a falsy value runs twice in
        // the original but once here.
        && is_pure_expr(alternative)
}

/// `cond and X or X` folds to `X` when both branches are written alike and
/// the condition is pure. Says whether it folded.
pub(super) fn fold_redundant_ternary<'ast>(
    allocator: &'ast Allocator,
    expr: &mut Expression<'ast>,
) -> bool {
    let Expression::BinaryOp(outer) = expr else {
        return false;
    };
    if !is_redundant_ternary(outer) {
        return false;
    }
    let Some(alternative) = (*outer.links).first_mut() else {
        return false;
    };
    let kept = truncate_multi_value(allocator, alternative.operand.take());
    match OneOrMore::from_list(outer.links.split_off_rest()) {
        Some(rest) => {
            outer.first = kept;
            outer.links = rest;
        }
        None => *expr = kept,
    }
    true
}

/// `first` and each of `rest` joined by `op`, which is `and` or `or`.
///
/// Both are associative, so an operand that is a bare run of `op` joins the
/// run. Only an if-expression printed last can't swallow an operator, so one
/// anywhere else gets parentheses.
fn logical<'ast>(
    allocator: &'ast Allocator,
    op: BinOp,
    first: Expression<'ast>,
    rest: Vec<Expression<'ast>>,
) -> Expression<'ast> {
    let wrap = |operand: Expression<'ast>, is_last: bool| {
        let binds_looser = matches!(
            &operand,
            Expression::BinaryOp(inner) if inner.precedence().0 < op.precedence().0
        );
        if binds_looser || (!is_last && ends_in_if_expr(&operand)) {
            parenthesize(allocator, operand)
        } else {
            operand
        }
    };
    let last_index = rest.len().saturating_sub(1);
    let first_operand = wrap(first, rest.is_empty());
    let wrapped = rest
        .into_iter()
        .enumerate()
        .map(|(i, operand)| BinaryLink { op, operand: wrap(operand, i == last_index) });
    let Some(links) = OneOrMore::from_list(List::from_iter_in(wrapped, allocator)) else {
        return first_operand;
    };
    let span = first_operand.span().merge(links.last().operand.span());
    let mut run = BinaryOp { span, first: first_operand, links };
    run.splice_operands(allocator);
    Expression::BinaryOp(Box::new_in(run, allocator))
}

/// The condition and value of `if c then return v end`.
fn if_return_guard<'tree, 'ast>(
    stmt: &'tree Statement<'ast>,
) -> Option<(&'tree Expression<'ast>, &'tree Expression<'ast>)> {
    let Statement::IfStatement(if_stmt) = stmt else {
        return None;
    };
    if !has_no_else(if_stmt) || !if_stmt.block.stmts.is_empty() {
        return None;
    }
    let Some(LastStatement::Return(return_stmt)) = &if_stmt.block.last_stmt else {
        return None;
    };
    let [return_expr] = &*return_stmt.exprs else {
        return None;
    };
    Some((&if_stmt.condition, return_expr))
}

#[expect(
    clippy::multiple_inherent_impl,
    reason = "the compressor is one type with its rewrites split by family"
)]
impl<'ast> Compressor<'ast> {
    /// Walks an `if`, keeping only the arms that can run and walking only
    /// those, then trades branches, merges a nested `if`, or drops what's
    /// left. Says whether anything of it stays.
    ///
    /// An arm under a falsy literal never runs. The first under a truthy
    /// literal runs whenever it's reached, so it becomes the `else` and every
    /// arm after it goes.
    pub(super) fn visit_if(&mut self, stmt: &mut Statement<'ast>) -> bool {
        let Statement::IfStatement(if_stmt) = stmt else {
            return true;
        };
        if !self.config.remove_dead_code {
            self.walk_if(if_stmt);
            self.merge_nested_ifs(if_stmt);
            return true;
        }
        let IfStatement { condition, block, elseif_clauses, else_clause, .. } = &mut **if_stmt;
        self.visit_expr(condition);
        let mut has_head = match constant_truthiness(condition) {
            Some(true) => {
                self.visit_block(block);
                *stmt = do_block(self.allocator, block.take());
                self.has_changed = true;
                return !matches!(stmt, Statement::DoBlock(do_block) if is_empty_block(&do_block.block));
            }
            Some(false) => false,
            None => {
                self.visit_block(block);
                true
            }
        };
        let mut has_dropped_arm = !has_head;
        let mut taken_block = None;
        let mut kept = 0usize;
        for index in 0..elseif_clauses.len() {
            let Some(clause) = elseif_clauses.get_mut(index) else {
                break;
            };
            self.visit_expr(&mut clause.condition);
            let truthiness = constant_truthiness(&clause.condition);
            has_dropped_arm |= truthiness.is_some();
            match truthiness {
                Some(false) => {}
                Some(true) => {
                    taken_block = Some(clause.block.take());
                    break;
                }
                None if has_head => {
                    self.visit_block(&mut clause.block);
                    elseif_clauses.swap(kept, index);
                    kept = kept.saturating_add(1);
                }
                // The first arm left takes the dropped head's place.
                None => {
                    has_head = true;
                    *condition = clause.condition.take();
                    self.visit_block(&mut clause.block);
                    *block = clause.block.take();
                }
            }
        }
        elseif_clauses.truncate(kept);
        if let Some(taken) = taken_block {
            *else_clause = Some(ElseClause { span: taken.span, block: taken });
        }
        if let Some(clause) = else_clause {
            self.visit_block(&mut clause.block);
        }
        if has_dropped_arm {
            self.has_changed = true;
        }
        if !has_head {
            return match else_clause.take() {
                Some(clause) => {
                    *stmt = do_block(self.allocator, clause.block);
                    !matches!(stmt, Statement::DoBlock(do_block) if is_empty_block(&do_block.block))
                }
                // `if false then ... end`.
                None => false,
            };
        }
        self.trade_branches(if_stmt);
        self.merge_nested_ifs(if_stmt);
        // An empty-body `if` can only go when evaluating the condition is
        // side-effect free: `if f() then end` calls f.
        let is_dead = has_no_else(if_stmt)
            && (constant_truthiness(&if_stmt.condition) == Some(false)
                || (is_empty_block(&if_stmt.block) && is_pure_expr(&if_stmt.condition)));
        !is_dead
    }

    fn walk_if(&mut self, if_stmt: &mut IfStatement<'ast>) {
        self.visit_expr(&mut if_stmt.condition);
        self.visit_block(&mut if_stmt.block);
        for clause in &mut if_stmt.elseif_clauses {
            self.visit_expr(&mut clause.condition);
            self.visit_block(&mut clause.block);
        }
        if let Some(else_clause) = &mut if_stmt.else_clause {
            self.visit_block(&mut else_clause.block);
        }
    }

    /// An `if` whose one `else` trades places with an empty `then` under the
    /// negated condition, or with the `then` of a negated condition.
    fn trade_branches(&mut self, if_stmt: &mut IfStatement<'ast>) {
        if !if_stmt.elseif_clauses.is_empty() {
            return;
        }
        let Some(else_clause) = &mut if_stmt.else_clause else {
            return;
        };
        // The `not` and its parentheses nest the condition two levels
        // deeper.
        if is_empty_block(&if_stmt.block) && fits_at_depth(self.depth, 2, &if_stmt.condition) {
            if_stmt.condition = negate(self.allocator, if_stmt.condition.take());
            if_stmt.block = else_clause.block.take();
            if_stmt.else_clause = None;
            self.has_changed = true;
            return;
        }
        if let Expression::UnaryOp(negation) = &mut if_stmt.condition
            && negation.op == UnOp::Not
        {
            // `if not c then A else B end` becomes `if c then B else A end`.
            if_stmt.condition = negation.operand.take();
            mem::swap(&mut if_stmt.block, &mut else_clause.block);
            self.has_changed = true;
        }
    }

    /// `if a then if b then X end end` becomes `if a and b then X end`, for
    /// every level of the run at once. The run of `and`s is one level, so each
    /// condition is measured once, for the `and` and a pair of parentheses it
    /// can gain.
    fn merge_nested_ifs(&mut self, outer_if: &mut IfStatement<'ast>) {
        if !self.config.simplify_statements || !has_no_else(outer_if) {
            return;
        }
        let mut inner_conditions = Vec::new();
        while outer_if.block.last_stmt.is_none()
            && matches!(
                &*outer_if.block.stmts,
                [Statement::IfStatement(inner_if)]
                    if has_no_else(inner_if) && fits_at_depth(self.depth, 2, &inner_if.condition)
            )
            && (!inner_conditions.is_empty() || fits_at_depth(self.depth, 2, &outer_if.condition))
            && let Some(Statement::IfStatement(inner_if)) =
                outer_if.block.stmts.first_mut().map(Statement::take)
        {
            let IfStatement { condition, block, .. } = inner_if.unbox();
            inner_conditions.push(condition);
            outer_if.block = block;
        }
        if !inner_conditions.is_empty() {
            outer_if.condition =
                logical(self.allocator, BinOp::And, outer_if.condition.take(), inner_conditions);
            self.has_changed = true;
        }
    }

    /// `repeat ... until true` runs once, so it becomes a `do` block unless
    /// its body has its own `break` or `continue`, which a `do` block would
    /// rebind to an outer loop (or make invalid). Says whether anything of it
    /// stays.
    pub(super) fn visit_repeat(&mut self, stmt: &mut Statement<'ast>) -> bool {
        let Statement::RepeatLoop(repeat_loop) = stmt else {
            return true;
        };
        self.visit_block(&mut repeat_loop.block);
        self.visit_expr(&mut repeat_loop.condition);
        if !self.config.remove_dead_code
            || constant_truthiness(&repeat_loop.condition) != Some(true)
            || has_loop_exit(&repeat_loop.block)
        {
            return true;
        }
        let block = repeat_loop.block.take();
        self.has_changed = true;
        let is_empty = is_empty_block(&block);
        *stmt = do_block(self.allocator, block);
        !is_empty
    }

    /// Converts `if c1 then return X end; if c2 then return Y end; return Z`
    /// into `return c1 and X or c2 and Y or Z` when `X` and `Y` are always
    /// truthy.
    pub(super) fn convert_if_return_chain(&mut self, block: &mut Block<'ast>) {
        // The walk is inside the block, at its own level.
        let level = self.depth;
        let Block { stmts, last_stmt, .. } = block;
        let Some(LastStatement::Return(return_stmt)) = last_stmt else {
            return;
        };
        let [fallback] = &mut *return_stmt.exprs else {
            return;
        };
        // `return f()` returns every value f does, but as the `or` fallback it
        // would be truncated to one, so leave the chain alone.
        if is_multi_value(fallback) {
            return;
        }
        // The guards and the fallback make one run of `or`s whatever its
        // length, and each guard's condition and value sit in a run of `and`s
        // under it, each maybe in parentheses.
        let link_count = stmts
            .iter()
            .rev()
            .take_while(|stmt| match if_return_guard(stmt) {
                Some((condition, return_expr)) => {
                    is_always_truthy(return_expr)
                        && fits_at_depth(level, 3, condition)
                        && fits_at_depth(level, 3, return_expr)
                }
                None => matches!(stmt, Statement::EmptyStatement(_)),
            })
            .count();
        let chain_start = stmts.len().saturating_sub(link_count);
        let chain = stmts.get(chain_start..).unwrap_or_default();
        let has_guard = chain.iter().any(|stmt| if_return_guard(stmt).is_some());
        if !has_guard || !fits_at_depth(level, 1, fallback) {
            return;
        }
        let allocator = self.allocator;
        let mut guards = Vec::with_capacity(link_count);
        for stmt in stmts.iter_mut().skip(chain_start) {
            let Statement::IfStatement(if_stmt) = stmt else {
                continue;
            };
            let Some(LastStatement::Return(guard_return)) = &mut if_stmt.block.last_stmt else {
                continue;
            };
            let Some(value) = (*guard_return.exprs).first_mut().map(Expression::take) else {
                continue;
            };
            guards.push(logical(allocator, BinOp::And, if_stmt.condition.take(), vec![value]));
        }
        stmts.truncate(chain_start);
        let mut chain_values = guards.into_iter();
        let fallback_expr = fallback.take();
        *fallback = match chain_values.next() {
            Some(first_guard) => logical(
                allocator,
                BinOp::Or,
                first_guard,
                chain_values.chain(iter::once(fallback_expr)).collect(),
            ),
            None => fallback_expr,
        };
        self.has_changed = true;
    }
}

#[cfg(test)]
mod tests {
    use luck_token::LuaVersion;

    use crate::fixtures::{NO_PASSES, minify_with};
    use crate::transform_config::TransformConfig;

    const DEAD_CODE: TransformConfig = TransformConfig { remove_dead_code: true, ..NO_PASSES };
    const STATEMENTS: TransformConfig = TransformConfig { simplify_statements: true, ..NO_PASSES };

    fn remove_dead(source: &str) -> String {
        minify_with(source, LuaVersion::Lua54, &DEAD_CODE)
    }

    fn simplify(source: &str) -> String {
        minify_with(source, LuaVersion::Lua54, &STATEMENTS)
    }

    #[test]
    fn makes_each_rewrite_when_it_is_the_only_one() {
        for (source, expected) in [
            ("local function f() return end return f", "local function f()end return f"),
            ("for i = 1, 2, 1 do f(i) end", "for i=1,2 do f(i)end"),
            ("local x = nil return x", "local x return x"),
            ("repeat f() until true", "do f()end"),
            ("if not a then f() else g() end", "if a then g()else f()end"),
            ("if a then else f() end", "if not a then f()end"),
            ("if a then f() elseif 1 then g() end", "if a then f()else g()end"),
            ("if {} then end", ""),
            ("while nil do f() end", ""),
            ("do end", ""),
            ("local x = f()", "f()"),
            ("f(); g()", "f()g()"),
            ("return 1 < 2 and \"x\" or \"x\"", "return\"x\""),
        ] {
            assert_eq!(remove_dead(source), expected, "source: {source}");
        }
    }

    #[test]
    fn keeps_the_semicolon_a_leading_paren_needs() {
        assert_eq!(remove_dead("f(); (g or h)()"), "f();(g or h)()");
    }

    /// Every Lua skips a `;` to see whether a label ends its block, which
    /// lets a `goto` jump past a `local` to it.
    #[test]
    fn removes_a_semicolon_after_a_label_that_ends_its_block() {
        assert_eq!(
            remove_dead("do goto done local x = f() print(x) ::done:: ; end"),
            "do goto done local x=f()print(x)::done::end"
        );
    }

    #[test]
    fn removes_dead_branches() {
        for (source, expected) in [
            ("if false then print(1) end", ""),
            ("if true then print(1) end", "do print(1)end"),
            ("while false do print(1) end", ""),
            (
                "if false then print(1) elseif x then print(2) else print(3) end",
                "if x then print(2)else print(3)end",
            ),
            (
                "if false then print(1) elseif false then print(2) else print(3) end",
                "do print(3)end",
            ),
        ] {
            assert_eq!(remove_dead(source), expected, "source: {source}");
        }
    }

    #[test]
    fn drops_falsy_arms_anywhere_in_the_chain() {
        for (source, expected) in [
            (
                "if a then f(1) elseif false then f(2) elseif b then f(3) elseif nil then f(4) else f(5) end",
                "if a then f(1)elseif b then f(3)else f(5)end",
            ),
            ("if false then f(1) elseif nil then f(2) elseif a then f(3) end", "if a then f(3)end"),
            ("if false then f(1) elseif nil then f(2) else f(3) end", "do f(3)end"),
            ("if false then f(1) elseif nil then f(2) end", ""),
        ] {
            assert_eq!(remove_dead(source), expected, "source: {source}");
        }
    }

    #[test]
    fn turns_the_first_truthy_arm_into_the_else() {
        for (source, expected) in [
            (
                "if a then f(1) elseif 1 then f(2) elseif b then f(3) else f(4) end",
                "if a then f(1)else f(2)end",
            ),
            (
                "if false then f(1) elseif a then f(2) elseif 'x' then f(3) elseif true then f(4) end",
                "if a then f(2)else f(3)end",
            ),
            ("if false then f(1) elseif true then f(2) elseif a then f(3) end", "do f(2)end"),
            ("if true then f(1) elseif a then f(2) else f(3) end", "do f(1)end"),
        ] {
            assert_eq!(remove_dead(source), expected, "source: {source}");
        }
    }

    #[test]
    fn keeps_every_arm_whose_condition_is_not_a_literal() {
        let source = "if a then f(1) elseif b() then f(2) elseif c.d then f(3) end";
        assert_eq!(remove_dead(source), "if a then f(1)elseif b()then f(2)elseif c.d then f(3)end");
    }

    #[test]
    fn collapses_a_long_run_of_falsy_arms() {
        let source = format!(
            "if false then f(){} elseif a then g() end",
            " elseif false then f()".repeat(5_000)
        );
        assert_eq!(remove_dead(&source), "if a then g()end");
    }

    #[test]
    fn keeps_global_self_assignment() {
        // `x = x` on a global fires __index and __newindex under a metatabled
        // environment, and this pass has no binding info.
        assert_eq!(remove_dead("x = x return 1"), "x=x return 1");
    }

    #[test]
    fn unrolls_repeat_until_true_only_without_its_own_exit() {
        // `break` binds to the repeat; a do-block would rebind it to an
        // outer loop (or make it invalid).
        assert_eq!(remove_dead("repeat print(1) until true"), "do print(1)end");
        assert_eq!(
            remove_dead("while f() do repeat if g() then break end until true end"),
            "while f()do repeat if g()then break end until true end"
        );
        assert_eq!(remove_dead("repeat break until true"), "repeat break until true");
    }

    #[test]
    fn negated_empty_then_condition_keeps_its_grouping() {
        for (condition, expected) in [
            ("a == b", "if not(a==b)then print(1)end"),
            ("a and b", "if not(a and b)then print(1)end"),
            ("a or b", "if not(a or b)then print(1)end"),
        ] {
            assert_eq!(remove_dead(&format!("if {condition} then else print(1) end")), expected);
        }
    }

    #[test]
    fn folds_a_ternary_whose_branches_are_written_alike() {
        for (source, expected) in [
            ("return ... and 1 or 1\n", "return 1"),
            ("return ... and -2 ^ 3 or -2 ^ 3\n", "return-2^3"),
            ("return ... and {1, x = \"a\"} or {1, x = \"a\"}\n", "return{1,x=\"a\"}"),
            ("return a and ... or ...\n", "return a and...or..."),
            ("return true and ... or ...\n", "return(...)"),
        ] {
            assert_eq!(remove_dead(source), expected, "source: {source}");
        }
    }

    #[test]
    fn keeps_a_ternary_whose_branches_differ() {
        for source in [
            "return ... and 1 or 2\n",
            "return ... and 1 or 1.0\n",
            "return ... and {1} or {2}\n",
            "return ... and function() end or function() end\n",
        ] {
            assert!(remove_dead(source).contains("or"), "source: {source}");
        }
    }

    #[test]
    fn flattens_nested_ifs_into_one_run_of_ands() {
        for (source, expected) in [
            ("if a then if b then print(1) end end", "if a and b then print(1)end"),
            ("if a or b then if c then x() end end", "if(a or b)and c then x()end"),
            ("if a then if b or c then x() end end", "if a and(b or c)then x()end"),
            ("if a and b then if c then x() end end", "if a and b and c then x()end"),
            ("if a then if b and c then x() end end", "if a and b and c then x()end"),
            ("if not a then if b == c then x() end end", "if not a and b==c then x()end"),
        ] {
            assert_eq!(simplify(source), expected, "source: {source}");
        }
        let source = format!("{}x(){}", "if a or b then ".repeat(200), " end".repeat(200));
        assert_eq!(
            simplify(&source),
            format!("if(a or b){}then x()end", "and(a or b)".repeat(199))
        );
    }

    #[test]
    fn turns_an_if_return_run_into_a_ternary() {
        for (source, expected) in [
            ("if x then return 1 end return 0", "return x and 1 or 0"),
            (
                "if cond then return \"hello\" end return \"world\"",
                "return cond and\"hello\"or\"world\"",
            ),
            (
                "if a then return 1 end if b then return 2 end return 0",
                "return a and 1 or b and 2 or 0",
            ),
            ("if a or b then return 1 end return 0", "return(a or b)and 1 or 0"),
            ("if a and b then return 1 end return c or d", "return a and b and 1 or c or d"),
            // Lua 5.2+ parses semicolons as statements, which must not break
            // the chain.
            (
                "if x then return 1 end; if y then return 2 end; return 0",
                "return x and 1 or y and 2 or 0",
            ),
        ] {
            assert_eq!(simplify(source), expected, "source: {source}");
        }
    }

    #[test]
    fn keeps_an_if_return_whose_value_may_be_falsy() {
        for source in [
            "if x then return false end return true",
            "if x then return y end return 0",
            "if x then return 1 end return f()",
        ] {
            assert!(simplify(source).starts_with("if"), "source: {source}");
        }
    }

    #[test]
    fn converts_a_long_if_return_run_into_one_run_of_ors() {
        let result = simplify(&format!("{}return 0\n", "if a then return 1 end\n".repeat(300)));
        assert_eq!(result, format!("return {}0", "a and 1 or ".repeat(300)));
    }

    #[test]
    fn merged_if_exprs_end_before_the_next_operator() {
        for (source, expected) in [
            (
                "if if a then b else c then return 1 end return 0",
                "return(if a then b else c)and 1 or 0",
            ),
            (
                "if a then if if b then c else d then x() end end",
                "if a and if b then c else d then x()end",
            ),
            (
                "if a then if if b then c else d then if e then x() end end end",
                "if a and(if b then c else d)and e then x()end",
            ),
            (
                "if a then if if b then c else d then return 1 end end return 0",
                "return(a and if b then c else d)and 1 or 0",
            ),
            (
                "if not if a then b else c then return 1 end return 0",
                "return(not if a then b else c)and 1 or 0",
            ),
            (
                "if a then return 1 end return if b then c else d",
                "return a and 1 or if b then c else d",
            ),
        ] {
            assert_eq!(minify_with(source, LuaVersion::Luau, &STATEMENTS), expected, "{source}");
        }
    }
}
