//! Reshaping statements that all run: a `do` block that scopes nothing
//! unwrapped, nested `if`s merged into one condition, and `if ... return`
//! runs turned into `and`/`or` chains.

use core::{iter, mem};

use luck_allocator::{Allocator, List};
use luck_ast::{
    BinaryLink, BinaryOp, Block, Expression, GetSpan as _, IfStatement, LastStatement, OneOrMore,
    Statement, is_multi_value,
};
use luck_token::BinOp;

use super::Compressor;
use crate::expr::{ends_in_if_expr, is_always_truthy, parenthesize};

/// Whether a statement declares something its block scopes: a local, a
/// label, a Luau type, or a Lua 5.5 global.
const fn is_scoped_declaration(stmt: &Statement<'_>) -> bool {
    match stmt {
        Statement::LocalAssignment(_)
        | Statement::LocalFunction(_)
        | Statement::Label(_)
        | Statement::TypeDeclaration(_)
        | Statement::GlobalDeclaration(_)
        | Statement::GlobalFunction(_)
        | Statement::GlobalStar(_) => true,
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
        | Statement::Break(_)
        | Statement::CompoundAssignment(_)
        | Statement::Error(_) => false,
    }
}

/// Whether a `do` block's statements can take its place in the block around
/// it: it scopes nothing, and its last statement, if any, can end that
/// block, which `can_end_outer` says.
fn can_unwrap(stmt: &Statement<'_>, can_end_outer: bool) -> bool {
    let Statement::DoBlock(do_block) = stmt else {
        return false;
    };
    (do_block.block.last_stmt.is_none() || can_end_outer)
        && !do_block.block.stmts.iter().any(is_scoped_declaration)
}

pub(super) fn has_no_else(if_stmt: &IfStatement<'_>) -> bool {
    if_stmt.elseif_clauses.is_empty() && if_stmt.else_clause.is_none()
}

/// Whether `if_stmt`'s one statement is an `if` that
/// [`Compressor::merge_nested_ifs`] can take into it.
pub(super) fn holds_nested_if(if_stmt: &IfStatement<'_>) -> bool {
    has_no_else(if_stmt)
        && if_stmt.block.last_stmt.is_none()
        && matches!(&*if_stmt.block.stmts, [Statement::IfStatement(inner)] if has_no_else(inner))
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
    let mut run = BinaryOp::boxed(span, first_operand, links, allocator);
    run.splice_operands(allocator);
    Expression::BinaryOp(run)
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

impl<'ast> Compressor<'_, 'ast> {
    /// `if a then if b then X end end` becomes `if a and b then X end`, for
    /// every level of the run at once, so an `if` inside the run leaves its
    /// own merge to this one, which builds the run of `and`s once. The run is
    /// one level, so each condition is measured once, for the `and` and a pair
    /// of parentheses it can gain. An `if` whose condition doesn't fit `depth`
    /// levels down, where this one is, starts a run of its own.
    pub(super) fn merge_nested_ifs(&mut self, outer_if: &mut IfStatement<'ast>, depth: usize) {
        if !self.config.simplify_statements {
            return;
        }
        let mut head = outer_if;
        let mut head_depth = depth;
        while has_no_else(head) {
            let mut inner_conditions = Vec::new();
            while head.block.last_stmt.is_none()
                && matches!(
                    &*head.block.stmts,
                    [Statement::IfStatement(inner_if)]
                        if has_no_else(inner_if) && self.fits(head_depth, 2, &inner_if.condition)
                )
                && (!inner_conditions.is_empty() || self.fits(head_depth, 2, &head.condition))
                && let Some(Statement::IfStatement(inner_if)) =
                    head.block.stmts.first_mut().map(Statement::take)
            {
                let IfStatement { condition, block, .. } = inner_if.unbox();
                inner_conditions.push(condition);
                head.block = block;
            }
            if !inner_conditions.is_empty() {
                head.condition =
                    logical(self.allocator, BinOp::And, head.condition.take(), inner_conditions);
                self.leave_expr(&mut head.condition, false);
                self.has_changed = true;
            }
            if !holds_nested_if(head) {
                break;
            }
            let Some(Statement::IfStatement(next)) = head.block.stmts.first_mut() else {
                break;
            };
            head = &mut **next;
            // Its block's level.
            head_depth = head_depth.saturating_add(1);
        }
    }

    /// Puts the statements of each `do` block that scopes nothing in its
    /// place, as in `if true then f() end`, which dead-code removal leaves as
    /// `do f() end`.
    pub(super) fn unwrap_do_blocks(&mut self, block: &mut Block<'ast>) {
        let last_index = block.stmts.len().saturating_sub(1);
        let can_end_block = block.last_stmt.is_none();
        let mut stmts = block.stmts.iter().enumerate();
        if !stmts.any(|(i, stmt)| can_unwrap(stmt, can_end_block && i == last_index)) {
            return;
        }
        self.unwrapped_stmts.clear();
        for (i, stmt) in mem::take(&mut block.stmts).into_iter().enumerate() {
            if !can_unwrap(&stmt, can_end_block && i == last_index) {
                self.unwrapped_stmts.push(stmt);
                continue;
            }
            if let Statement::DoBlock(do_block) = stmt {
                let inner = do_block.unbox().block;
                self.unwrapped_stmts.extend(inner.stmts);
                if inner.last_stmt.is_some() {
                    block.last_stmt = inner.last_stmt;
                }
            }
        }
        block.stmts = List::from_iter_in(self.unwrapped_stmts.drain(..), self.allocator);
        self.has_changed = true;
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
                        && self.fits(level, 3, condition)
                        && self.fits(level, 3, return_expr)
                }
                None => matches!(stmt, Statement::EmptyStatement(_)),
            })
            .count();
        let chain_start = stmts.len().saturating_sub(link_count);
        let chain = stmts.get(chain_start..).unwrap_or_default();
        let has_guard = chain.iter().any(|stmt| if_return_guard(stmt).is_some());
        if !has_guard || !self.fits(level, 1, fallback) {
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
            let mut guard = logical(allocator, BinOp::And, if_stmt.condition.take(), vec![value]);
            self.leave_expr(&mut guard, true);
            guards.push(guard);
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
        self.leave_expr(fallback, false);
        self.has_changed = true;
    }
}

#[cfg(test)]
mod tests {
    use luck_allocator::Allocator;
    use luck_ast::MAX_NESTING_DEPTH;
    use luck_testgen::with_the_smallest_stack;
    use luck_token::LuaVersion;

    use crate::fixtures::{NO_PASSES, minify_with};
    use crate::transform_config::TransformConfig;

    const STATEMENTS: TransformConfig = TransformConfig { simplify_statements: true, ..NO_PASSES };

    fn simplify(source: &str) -> String {
        minify_with(source, LuaVersion::Lua54, &STATEMENTS)
    }

    #[test]
    fn unwraps_a_do_block_that_scopes_nothing() {
        for (source, expected) in [
            ("do f() g() end h()", "f()g()h()"),
            ("while a do do break end end", "while a do break end"),
            ("f() do return 1 end", "f()return 1"),
            ("do do f() end end", "f()"),
        ] {
            assert_eq!(simplify(source), expected, "source: {source}");
        }
        let both = TransformConfig { remove_dead_code: true, ..STATEMENTS };
        assert_eq!(minify_with("if true then f() end", LuaVersion::Lua54, &both), "f()");
    }

    #[test]
    fn keeps_a_do_block_that_scopes_something() {
        for source in [
            "do local x=1 end",
            "do local function f()end end",
            "do goto l::l::end",
            "do return 1 end f()",
        ] {
            assert_eq!(simplify(source), source);
        }
        assert_eq!(
            minify_with("do type T=number end", LuaVersion::Luau, &STATEMENTS),
            "do type T=number end"
        );
        assert_eq!(
            minify_with("do global x end", LuaVersion::Lua55, &STATEMENTS),
            "do global x end"
        );
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
        let both = TransformConfig { remove_dead_code: true, ..STATEMENTS };
        for (dropped_if, expected) in [
            ("if true then if a then if b then x() end end end", "if a and b then x()end"),
            ("if a then if true then if b then x() end end end", "if a and b then x()end"),
        ] {
            assert_eq!(minify_with(dropped_if, LuaVersion::Lua54, &both), expected, "{dropped_if}");
        }
    }

    #[test]
    fn folds_the_runs_it_builds() {
        let folding = TransformConfig { fold_constants: true, ..STATEMENTS };
        for (source, expected) in [
            ("if true then if x then f() end end", "if x then f()end"),
            ("if (false) then if (true) then f() end end", "if false then f()end"),
            ("if true then return 1 end return 0", "return 1 or 0"),
        ] {
            assert_eq!(minify_with(source, LuaVersion::Lua54, &folding), expected, "{source}");
        }
    }

    #[test]
    fn merges_a_run_of_nested_ifs_once_however_deep() {
        with_the_smallest_stack(|| {
            // Merged an `if` at a time, the run of `and`s was copied at every level.
            // The chunk's block and the call take the levels the `if`s leave.
            let ifs = MAX_NESTING_DEPTH - 2;
            let source = format!("{}x(){}", "if a then ".repeat(ifs), " end".repeat(ifs));
            let allocator = Allocator::new();
            let output = crate::minify(&allocator, &source, LuaVersion::Lua54, &STATEMENTS);
            let expected = format!("if a{} then x()end", " and a".repeat(ifs - 1));
            assert!(output.as_ref() == Ok(&expected), "{:?}", output.map(|text| text.len()));
            assert!(allocator.used_bytes() < 64 * source.len(), "{}", allocator.used_bytes());
        });
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
