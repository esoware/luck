use core::iter;

use luck_ast::{
    BinaryLink, BinaryOp, Block, Expression, IfStatement, LastStatement, OneOrMore, Punctuated,
    ReturnStatement, Statement, Transform, is_multi_value,
};
use luck_token::{BinOp, Span};

use crate::expr::{ends_in_if_expr, fits_at_depth, is_always_truthy, parenthesize};

pub(crate) fn simplify(block: Block) -> Block {
    StatementSimplifier { depth: 0 }.transform_block(block)
}

struct StatementSimplifier {
    /// Levels of nesting open where the walk is.
    depth: usize,
}

impl Transform for StatementSimplifier {
    fn descend(&mut self) {
        self.depth = self.depth.saturating_add(1);
    }

    fn ascend(&mut self) {
        self.depth = self.depth.saturating_sub(1);
    }

    fn transform_stmt(&mut self, stmt: Statement) -> Statement {
        match stmt {
            Statement::IfStatement(outer_if) => self.merge_nested_ifs(outer_if),
            other @ (Statement::Assignment(_)
            | Statement::FunctionCall(_)
            | Statement::DoBlock(_)
            | Statement::WhileLoop(_)
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
            | Statement::Error(_)) => self.walk_stmt(other),
        }
    }

    fn transform_block(&mut self, block: Block) -> Block {
        let walked = self.walk_block(block);
        // The walk has stepped back out of the block, which is one level in.
        try_convert_if_return_chain(walked, self.depth.saturating_add(1))
    }
}

impl StatementSimplifier {
    /// `if a then if b then X end end` becomes `if a and b then X end`, for
    /// every level of the run at once. The run of `and`s is one level, so each
    /// condition is measured once, for the `and` and a pair of parentheses it
    /// can gain.
    fn merge_nested_ifs(&mut self, mut outer_if: Box<IfStatement>) -> Statement {
        let mut inner_conditions = Vec::new();
        if has_no_else(&outer_if) {
            // The check guarantees the pop yields the inner `if`. The outer
            // condition is measured only once there's an `if` to merge.
            while outer_if.block.last_stmt.is_none()
                && matches!(
                    outer_if.block.stmts.as_slice(),
                    [Statement::IfStatement(inner_if)]
                        if has_no_else(inner_if)
                            && fits_at_depth(self.depth, 2, &inner_if.condition)
                )
                && (!inner_conditions.is_empty()
                    || fits_at_depth(self.depth, 2, &outer_if.condition))
                && let Some(Statement::IfStatement(inner_if)) = outer_if.block.stmts.pop()
            {
                let IfStatement { condition, block, .. } = *inner_if;
                inner_conditions.push(condition);
                outer_if.block = block;
            }
        }
        if !inner_conditions.is_empty() {
            outer_if.span = Span::default();
            outer_if.condition = logical(BinOp::And, outer_if.condition.take(), inner_conditions);
        }
        // Left for the merged `if`'s own walk, so the body is walked once and
        // a rewrite inside a condition sees the depth it ends up at.
        self.walk_stmt(Statement::IfStatement(outer_if))
    }
}

/// Converts `if c1 then return X end; if c2 then return Y end; return Z`
/// into `return c1 and X or c2 and Y or Z` when `X` and `Y` are always
/// truthy. `level` is the block's own level of nesting.
fn try_convert_if_return_chain(mut block: Block, level: usize) -> Block {
    let Some(LastStatement::Return(return_stmt)) = block.last_stmt.as_deref_mut() else {
        return block;
    };
    let [fallback_expr] = return_stmt.exprs.items.as_mut_slice() else {
        return block;
    };
    // `return f()` returns every value f does, but as the `or` fallback it
    // would be truncated to one, so leave the chain alone.
    if is_multi_value(fallback_expr) {
        return block;
    }

    // The guards and the fallback make one run of `or`s whatever its length,
    // and each guard's condition and value sit in a run of `and`s under it,
    // each maybe in parentheses.
    let link_count = block
        .stmts
        .iter()
        .rev()
        .take_while(|stmt| match extract_if_return_guard(stmt) {
            Some((condition, return_expr)) => {
                is_always_truthy(return_expr)
                    && fits_at_depth(level, 3, condition)
                    && fits_at_depth(level, 3, return_expr)
            }
            None => matches!(stmt, Statement::EmptyStatement(_)),
        })
        .count();
    let chain_start = block.stmts.len().saturating_sub(link_count);
    let has_guard = block
        .stmts
        .get(chain_start..)
        .is_some_and(|chain| chain.iter().any(|stmt| extract_if_return_guard(stmt).is_some()));
    if !has_guard || !fits_at_depth(level, 1, fallback_expr) {
        return block;
    }

    let fallback = fallback_expr.take();
    let result_expr = {
        let mut guards = block.stmts.drain(chain_start..).filter_map(into_guard_parts).map(
            |(condition, return_expr)| logical(BinOp::And, condition, iter::once(return_expr)),
        );
        match guards.next() {
            Some(first_guard) => {
                logical(BinOp::Or, first_guard, guards.chain(iter::once(fallback)))
            }
            None => fallback,
        }
    };
    **return_stmt =
        ReturnStatement { span: Span::default(), exprs: Punctuated::from_item(result_expr) };
    block
}

fn extract_if_return_guard(stmt: &Statement) -> Option<(&Expression, &Expression)> {
    let Statement::IfStatement(if_stmt) = stmt else {
        return None;
    };
    if !has_no_else(if_stmt) || !if_stmt.block.stmts.is_empty() {
        return None;
    }
    let last = if_stmt.block.last_stmt.as_ref()?;
    let LastStatement::Return(return_stmt) = last.as_ref() else {
        return None;
    };
    let [return_expr] = return_stmt.exprs.items.as_slice() else {
        return None;
    };
    Some((&if_stmt.condition, return_expr))
}

fn into_guard_parts(stmt: Statement) -> Option<(Expression, Expression)> {
    let Statement::IfStatement(if_stmt) = stmt else {
        return None;
    };
    let IfStatement { condition, block, .. } = *if_stmt;
    let LastStatement::Return(return_stmt) = *block.last_stmt? else {
        return None;
    };
    let [return_expr] = <[Expression; 1]>::try_from(return_stmt.exprs.into_items()).ok()?;
    Some((condition, return_expr))
}

const fn has_no_else(if_stmt: &IfStatement) -> bool {
    if_stmt.elseif_clauses.is_empty() && if_stmt.else_clause.is_none()
}

/// `first` and each of `rest` joined by `op`, which is `and` or `or`.
///
/// Both are associative, so an operand that is a bare run of `op` joins the
/// run. Only an if-expression printed last can't swallow an operator, so one
/// anywhere else gets parentheses.
fn logical<Rest>(op: BinOp, first: Expression, rest: Rest) -> Expression
where
    Rest: IntoIterator<Item = Expression>,
{
    let wrap = |operand: Expression, is_last: bool| {
        if binds_looser(&operand, op) || (!is_last && ends_in_if_expr(&operand)) {
            parenthesize(operand)
        } else {
            operand
        }
    };
    let mut others = rest.into_iter().peekable();
    let first_operand = wrap(first, others.peek().is_none());
    let mut wrapped = Vec::new();
    while let Some(operand) = others.next() {
        let is_last = others.peek().is_none();
        wrapped.push(BinaryLink { op, operand: wrap(operand, is_last) });
    }
    let Some(links) = OneOrMore::from_vec(wrapped) else {
        return first_operand;
    };
    let mut run = BinaryOp { span: Span::default(), first: first_operand, links };
    run.splice_operands();
    Expression::BinaryOp(Box::new(run))
}

fn binds_looser(operand: &Expression, op: BinOp) -> bool {
    matches!(operand, Expression::BinaryOp(inner) if inner.precedence().0 < op.precedence().0)
}

#[cfg(test)]
mod tests {
    use luck_token::LuaVersion;

    use super::*;
    use crate::fixtures::{assert_reparses, run_pass};

    fn apply(source: &str) -> String {
        run_pass(source, LuaVersion::Lua54, simplify)
    }

    #[test]
    fn flattens_nested_if() {
        let result = apply("if a then\n  if b then\n    print(1)\n  end\nend\n");
        assert!(result.contains("and"), "Expected flattened if with and, got: {result}");
        let if_count = result.matches("if").count();
        assert_eq!(if_count, 1, "Expected single if, got: {result}");
    }

    #[test]
    fn flattens_a_run_of_nested_ifs_into_one_run_of_ands() {
        let source = format!("{}x(){}", "if a or b then ".repeat(200), " end".repeat(200));
        let result = apply(&source);
        assert_reparses(&result, LuaVersion::Lua54);
        assert_eq!(result, format!("if(a or b){}then x()end", "and(a or b)".repeat(199)));
    }

    #[test]
    fn if_return_to_ternary() {
        let result = apply("if x then return 1 end\nreturn 0\n");
        assert!(
            result.contains("and") && result.contains("or"),
            "Expected ternary conversion, got: {result}"
        );
    }

    #[test]
    fn no_ternary_when_falsy_return() {
        let result = apply("if x then return false end\nreturn true\n");
        assert!(
            result.contains("if"),
            "Should not convert when return value could be falsy, got: {result}"
        );
    }

    #[test]
    fn if_return_chain_spans_semicolons() {
        // Lua 5.2+ parses semicolons as EmptyStatement, which must not break
        // the chain.
        let result = apply("if x then return 1 end;\nif y then return 2 end;\nreturn 0\n");
        assert!(
            result.contains("and") && result.contains("or"),
            "Semicolons between if-return guards should not prevent ternary conversion, got: {result}"
        );
    }

    #[test]
    fn no_ternary_when_variable_return() {
        // The return value is a variable, so is_always_truthy says no and the
        // chain stays an if.
        let result = apply("if x then return y end\nreturn 0\n");
        assert!(
            result.contains("if"),
            "Variable return values are not always truthy, got: {result}"
        );
    }

    #[test]
    fn if_return_to_ternary_with_string_returns() {
        let result = apply("if cond then return \"hello\" end\nreturn \"world\"\n");
        assert!(
            result.contains("and") && result.contains("or"),
            "String literal returns should convert to ternary, got: {result}"
        );
    }

    #[test]
    fn merged_conditions_keep_their_grouping() {
        for (source, expected) in [
            ("if a or b then if c then x() end end", "if(a or b)and c then x()end"),
            ("if a then if b or c then x() end end", "if a and(b or c)then x()end"),
            ("if a or b then return 1 end return 0", "return(a or b)and 1 or 0"),
        ] {
            assert_eq!(apply(source), expected, "source: {source}");
        }
    }

    #[test]
    fn merged_conditions_stay_bare_where_grouping_holds() {
        for (source, expected) in [
            ("if a then if b then x() end end", "if a and b then x()end"),
            ("if a and b then if c then x() end end", "if a and b and c then x()end"),
            ("if a then if b and c then x() end end", "if a and b and c then x()end"),
            ("if not a then if b == c then x() end end", "if not a and b==c then x()end"),
            (
                "if a then return 1 end if b then return 2 end return 0",
                "return a and 1 or b and 2 or 0",
            ),
            ("if a and b then return 1 end return c or d", "return a and b and 1 or c or d"),
        ] {
            assert_eq!(apply(source), expected, "source: {source}");
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
            let result = run_pass(source, LuaVersion::Luau, simplify);
            assert_eq!(result, expected, "source: {source}");
            assert_reparses(&result, LuaVersion::Luau);
        }
    }

    #[test]
    fn chained_if_returns_to_one_ternary() {
        let result = apply(
            "if a then return 1 end\nif b then return 2 end\nif c then return 3 end\nreturn 0\n",
        );
        assert!(
            !result.contains("if"),
            "All chained if-return guards with truthy values should be converted, got: {result}"
        );
    }

    #[test]
    fn converts_a_long_if_return_run_into_one_run_of_ors() {
        let result = apply(&format!("{}return 0\n", "if a then return 1 end\n".repeat(300)));
        assert_reparses(&result, LuaVersion::Lua54);
        assert_eq!(result, format!("return {}0", "a and 1 or ".repeat(300)));
    }
}
