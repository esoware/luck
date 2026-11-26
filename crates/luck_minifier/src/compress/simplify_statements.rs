//! Reshaping statements that all run: a `do` block that scopes nothing
//! unwrapped, nested `if`s merged into one condition, and `if ... return`
//! runs turned into `and`/`or` chains.

use std::{iter, mem};

use luck_allocator::{Allocator, Box, List};
use luck_ast::{
    BinaryLink, BinaryOp, Block, Expression, GetSpan as _, IfStatement, OneOrMore, Statement,
};
use luck_token::BinOp;
use luck_value::ToBoolean as _;

use super::Compressor;
use super::simplify_parens::Place;
use crate::expr::fits_at_depth;

/// Whether a statement declares something its block scopes: a local or a
/// type.
const fn is_scoped_declaration(stmt: &Statement<'_>) -> bool {
    match stmt {
        Statement::LocalAssignment(_)
        | Statement::LocalFunction(_)
        | Statement::TypeDeclaration(_)
        | Statement::Declaration(_) => true,
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
        | Statement::Error(_) => false,
    }
}

/// Whether `block` ends in a statement that has to end the block it's in,
/// so its statements can't go where others follow them.
pub(super) fn ends_in_block_end(block: &Block<'_>) -> bool {
    block.stmts.last().is_some_and(Statement::has_to_end_block)
}

/// Whether a `do` block's statements can take its place in the block around
/// it: it scopes nothing, and it ends that block, which `is_last` says, or
/// its own statements can go on where others follow.
fn can_unwrap(stmt: &Statement<'_>, is_last: bool) -> bool {
    let Statement::DoBlock(do_block) = stmt else {
        return false;
    };
    (is_last || !ends_in_block_end(&do_block.block))
        && !do_block.block.stmts.iter().any(is_scoped_declaration)
}

pub(super) fn has_no_else(if_stmt: &IfStatement<'_>) -> bool {
    if_stmt.elseif_clauses.is_empty() && if_stmt.else_clause.is_none()
}

/// Whether `if_stmt`'s one statement is an `if` that
/// [`Compressor::merge_nested_ifs`] can take into it.
pub(super) fn holds_nested_if(if_stmt: &IfStatement<'_>) -> bool {
    is_mergeable(if_stmt)
        && matches!(&*if_stmt.block.stmts, [Statement::IfStatement(inner)] if is_mergeable(inner))
}

/// Whether `if_stmt` is an `if c then ... end` whose condition joins a run
/// of `and`s, which a binding's can't, since the binding would then hold
/// the whole run.
fn is_mergeable(if_stmt: &IfStatement<'_>) -> bool {
    has_no_else(if_stmt) && if_stmt.binding.is_none()
}

/// `first` and each of `rest` joined by `op`, which is `and` or `or`.
///
/// Both are associative and no metamethod intercepts either, so an operand
/// that is a run of `op` joins the run wherever it stands.
fn logical<'ast>(
    allocator: &'ast Allocator,
    op: BinOp,
    first: Expression<'ast>,
    rest: impl IntoIterator<Item = Expression<'ast>>,
) -> Expression<'ast> {
    let first_span = first.span();
    // An operand's own first operand and the links after it.
    let split = |operand: Expression<'ast>| {
        let is_joined = matches!(&operand, Expression::BinaryOp(run) if run.links.first().op == op);
        if is_joined && let Expression::BinaryOp(run) = operand {
            let BinaryOp { first: head, links: tail, .. } = Box::unbox(run);
            return (head, Some(tail));
        }
        (operand, None)
    };
    let (first_operand, first_tail) = split(first);
    let mut joined: Vec<BinaryLink<'ast>> = first_tail.into_iter().flatten().collect();
    let mut last_span = None;
    for operand in rest {
        last_span = Some(operand.span());
        let (head, tail) = split(operand);
        joined.push(BinaryLink { op, operand: head });
        joined.extend(tail.into_iter().flatten());
    }
    let span = last_span.map_or(first_span, |last| first_span.merge(last));
    let Some(links) = OneOrMore::from_list(List::from_iter_in(joined, allocator)) else {
        return first_operand;
    };
    Expression::new_binary_op(span, first_operand, links, allocator)
}

/// The condition and value of `if c then return v end`.
fn if_return_guard<'tree, 'ast>(
    stmt: &'tree Statement<'ast>,
) -> Option<(&'tree Expression<'ast>, &'tree Expression<'ast>)> {
    let Statement::IfStatement(if_stmt) = stmt else {
        return None;
    };
    if !is_mergeable(if_stmt) {
        return None;
    }
    let [Statement::Return(return_stmt)] = &*if_stmt.block.stmts else {
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
    /// one level, so each condition is measured once, for the `and`. An `if`
    /// whose condition doesn't fit `depth` levels down, where this one is,
    /// starts a run of its own.
    pub(super) fn merge_nested_ifs(&mut self, outer_if: &mut IfStatement<'ast>, depth: usize) {
        if !self.config.simplify_statements {
            return;
        }
        let mut head = outer_if;
        let mut head_depth = depth;
        while is_mergeable(head) {
            let mut inner_conditions = Vec::new();
            while matches!(
                &*head.block.stmts,
                [Statement::IfStatement(inner_if)]
                    if is_mergeable(inner_if) && fits_at_depth(head_depth, 1, &inner_if.condition)
            ) && (!inner_conditions.is_empty()
                || fits_at_depth(head_depth, 1, &head.condition))
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
                self.leave_expr(&mut head.condition, Place::OneValue);
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
            head_depth += 1;
        }
    }

    /// Puts the statements of each `do` block that scopes nothing in its
    /// place, as in `if true then f() end`, which dead-code removal leaves as
    /// `do f() end`.
    pub(super) fn unwrap_do_blocks(&mut self, block: &mut Block<'ast>) {
        let last_index = block.stmts.len().saturating_sub(1);
        let mut stmts = block.stmts.iter().enumerate();
        if !stmts.any(|(i, stmt)| can_unwrap(stmt, i == last_index)) {
            return;
        }
        self.unwrapped_stmts.clear();
        for (i, stmt) in mem::take(&mut block.stmts).into_iter().enumerate() {
            if !can_unwrap(&stmt, i == last_index) {
                self.unwrapped_stmts.push(stmt);
                continue;
            }
            if let Statement::DoBlock(do_block) = stmt {
                self.unwrapped_stmts.extend(do_block.unbox().block.stmts);
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
        let Some((Statement::Return(return_stmt), stmts)) = block.stmts.split_last_mut() else {
            return;
        };
        let [fallback] = &mut *return_stmt.exprs else {
            return;
        };
        // `return f()` returns every value f does, but as the `or` fallback it
        // would be truncated to one, so leave the chain alone.
        if fallback.is_multi_value() {
            return;
        }
        // The guards and the fallback make one run of `or`s whatever its
        // length, and each guard's condition and value sit in a run of `and`s
        // under it.
        let link_count = stmts
            .iter()
            .rev()
            .take_while(|stmt| {
                if_return_guard(stmt).is_some_and(|(condition, return_expr)| {
                    return_expr.to_boolean(&self.names()) == Some(true)
                        && fits_at_depth(level, 2, condition)
                        && fits_at_depth(level, 2, return_expr)
                })
            })
            .count();
        if link_count == 0 || !fits_at_depth(level, 1, fallback) {
            return;
        }
        let return_index = stmts.len();
        let chain_start = return_index - link_count;
        let allocator = self.allocator;
        let mut guards = Vec::with_capacity(link_count);
        for stmt in stmts.iter_mut().skip(chain_start) {
            let Statement::IfStatement(if_stmt) = stmt else {
                continue;
            };
            let [Statement::Return(guard_return)] = &mut *if_stmt.block.stmts else {
                continue;
            };
            let value = guard_return.exprs[0].take();
            let mut guard = logical(allocator, BinOp::And, if_stmt.condition.take(), [value]);
            self.leave_expr(&mut guard, Place::OneValue);
            guards.push(guard);
        }
        let mut chain_values = guards.into_iter();
        let fallback_expr = fallback.take();
        *fallback = match chain_values.next() {
            Some(first_guard) => logical(
                allocator,
                BinOp::Or,
                first_guard,
                chain_values.chain(iter::once(fallback_expr)),
            ),
            None => fallback_expr,
        };
        self.leave_expr(fallback, Place::AllValues);
        // The return takes the place of the first guard, and the guards go.
        block.stmts.swap(chain_start, return_index);
        block.stmts.truncate(chain_start + 1);
        self.has_changed = true;
    }
}

#[cfg(test)]
mod tests {
    use luck_allocator::Allocator;
    use luck_ast::MAX_NESTING_DEPTH;
    use luck_testgen::with_the_smallest_stack;

    use crate::fixtures::{NO_PASSES, minify_with};
    use crate::transform_config::TransformConfig;

    const STATEMENTS: TransformConfig = TransformConfig { simplify_statements: true, ..NO_PASSES };

    fn simplify(source: &str) -> String {
        minify_with(source, &STATEMENTS)
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
        assert_eq!(minify_with("if true then f() end", &both), "f()");
    }

    #[test]
    fn keeps_a_do_block_that_scopes_something() {
        for source in [
            "do local x=1 end",
            "do local function f()end end",
            "do type T=number end",
            "do return 1 end f()",
        ] {
            assert_eq!(simplify(source), source);
        }
    }

    #[test]
    fn keeps_a_do_break_that_other_statements_follow() {
        assert_eq!(simplify("while a do do break end f() end"), "while a do do break end f()end");
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
            assert_eq!(minify_with(dropped_if, &both), expected, "{dropped_if}");
        }
    }

    #[test]
    fn folds_the_runs_it_builds() {
        let folding = TransformConfig { fold_constants: true, ..STATEMENTS };
        for (source, expected) in [
            ("if true then if x then f() end end", "if x then f()end"),
            ("if (false) then if (true) then f() end end", "if false then f()end"),
            ("if true then return 1 end return 0", "return 1"),
        ] {
            assert_eq!(minify_with(source, &folding), expected, "{source}");
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
            let output = crate::minify(&allocator, source.as_bytes(), &STATEMENTS);
            let expected = format!("if a{} then x()end", " and a".repeat(ifs - 1)).into_bytes();
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
            ("f() if x then return 1 end return 0", "f()return x and 1 or 0"),
            // A `;` between them doesn't break the chain.
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
    fn drops_a_truncation_that_lands_in_an_operator_run_in_the_same_pass() {
        let both = TransformConfig { simplify_parens: true, ..STATEMENTS };
        for (source, expected) in [
            ("if not x then return 3 end return (f())", "return not x and 3 or f()"),
            ("if a then if (f()) then g() end end", "if a and f()then g()end"),
        ] {
            assert_eq!(minify_with(source, &both), expected, "{source}");
        }
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
                "return a and(if b then c else d)and 1 or 0",
            ),
            (
                "if not if a then b else c then return 1 end return 0",
                "return not(if a then b else c)and 1 or 0",
            ),
            (
                "if a then return 1 end return if b then c else d",
                "return a and 1 or if b then c else d",
            ),
        ] {
            assert_eq!(minify_with(source, &STATEMENTS), expected, "{source}");
        }
    }

    #[test]
    fn a_bound_if_keeps_its_condition_to_itself() {
        for (source, expected) in [
            (
                "if a then if local x = b then f(x) end end",
                "if a then if local x=b then f(x)end end",
            ),
            (
                "if local x = a then if b then f(x) end end",
                "if local x=a then if b then f(x)end end",
            ),
            (
                "if a then if b then if local x = c then f(x) end end end",
                "if a and b then if local x=c then f(x)end end",
            ),
            (
                "if local x = a then if b then if c then f(x) end end end",
                "if local x=a then if b and c then f(x)end end",
            ),
            (
                "if local x = a then return 1 end return b",
                "if local x=a then return 1 end return b",
            ),
        ] {
            assert_eq!(simplify(source), expected, "source: {source}");
        }
    }
}
