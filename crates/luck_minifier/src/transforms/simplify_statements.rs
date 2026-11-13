use luck_ast::{
    BinaryOp, Block, Expression, IfStatement, LastStatement, Punctuated, ReturnStatement,
    Statement, Transform, is_multi_value,
};
use luck_token::{BinOp, Span};

use crate::expr::{
    MAX_ADDED_DEPTH, ends_in_if_expr, fits_at_depth, is_always_truthy, parenthesize,
};

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
            Statement::IfStatement(mut outer_if) => {
                // `if a then if b then X end end` becomes `if a and b then X end`.
                // The check guarantees the pop yields the inner `if`.
                if has_no_else(&outer_if)
                    && outer_if.block.last_stmt.is_none()
                    && matches!(
                        outer_if.block.stmts.as_slice(),
                        [Statement::IfStatement(inner_if)]
                            if has_no_else(inner_if)
                                && conjunction_fits(
                                    self.depth,
                                    &outer_if.condition,
                                    &inner_if.condition
                                )
                    )
                    && let Some(Statement::IfStatement(inner_if)) = outer_if.block.stmts.pop()
                {
                    // Left for the merged `if`'s own walk, so the body is
                    // walked once and a rewrite inside either condition sees
                    // the depth it ends up at.
                    let merged_if = IfStatement {
                        span: Span::default(),
                        condition: logical(outer_if.condition, BinOp::And, inner_if.condition),
                        block: inner_if.block,
                        elseif_clauses: Vec::new(),
                        else_clause: None,
                    };
                    return self.transform_stmt(Statement::IfStatement(Box::new(merged_if)));
                }
                self.walk_stmt(Statement::IfStatement(outer_if))
            }
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

    // Each guard nests the chain one level deeper, and the fallback may hold
    // the chain an earlier round built, so a long run converts only its tail.
    let built_links = spine_links(fallback_expr, BinOp::Or);
    // A guard sits under the fallback's `or` links, the one that joins it,
    // and one for each guard after it.
    let mut guard_level = level.saturating_add(built_links).saturating_add(1);
    let mut link_count = 0usize;
    for stmt in block.stmts.iter().rev().take(MAX_ADDED_DEPTH.saturating_sub(built_links)) {
        let is_link = match extract_if_return_guard(stmt) {
            Some((condition, return_expr)) => {
                guard_level = guard_level.saturating_add(1);
                is_always_truthy(return_expr)
                    && conjunction_fits(guard_level, condition, return_expr)
            }
            None => matches!(stmt, Statement::EmptyStatement(_)),
        };
        if !is_link {
            break;
        }
        link_count = link_count.saturating_add(1);
    }
    let chain_start = block.stmts.len().saturating_sub(link_count);
    let has_guard = block
        .stmts
        .get(chain_start..)
        .is_some_and(|chain| chain.iter().any(|stmt| extract_if_return_guard(stmt).is_some()));
    if !has_guard || !fits_at_depth(level, 1, fallback_expr) {
        return block;
    }

    let fallback = fallback_expr.take();
    // Left-nested, the shape the parser builds for the printed chain.
    let guards = block
        .stmts
        .drain(chain_start..)
        .filter_map(into_guard_parts)
        .map(|(condition, return_expr)| logical(condition, BinOp::And, return_expr))
        .reduce(|earlier, guard| logical(earlier, BinOp::Or, guard));
    let result_expr = match guards {
        Some(chain) => logical(chain, BinOp::Or, fallback),
        None => fallback,
    };
    **return_stmt =
        ReturnStatement { span: Span::default(), exprs: Punctuated::from_item(result_expr) };
    block
}

/// Whether `lhs and rhs` still nests within the parser's cap at `level`.
///
/// Each side can gain the operator and a pair of parentheses, and `lhs`
/// sinks under every `and` that `rhs` starts with.
fn conjunction_fits(level: usize, lhs: &Expression, rhs: &Expression) -> bool {
    let lhs_levels = spine_links(rhs, BinOp::And).saturating_add(2);
    fits_at_depth(level, lhs_levels, lhs) && fits_at_depth(level, 2, rhs)
}

/// How many `op` links `expr` starts with: the ones a left operand joined on
/// `op` ends up under.
fn spine_links(expr: &Expression, op: BinOp) -> usize {
    let mut links = 0usize;
    let mut spine = expr;
    while let Expression::BinaryOp(binary_op) = spine
        && binary_op.op == op
    {
        links = links.saturating_add(1);
        spine = &binary_op.lhs;
    }
    links
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

/// A nested operand of the same operator stays bare on either side, because
/// `and` and `or` are associative. An if-expression as `rhs` prints last, so
/// only one in `lhs` can swallow an operator.
///
/// A bare `rhs` on the same operator reads back as the tail of one chain,
/// with `lhs` under every link of it, which is how the walkers count it.
fn logical(left_operand: Expression, op: BinOp, right_operand: Expression) -> Expression {
    let left = if binds_looser(&left_operand, op) || ends_in_if_expr(&left_operand) {
        parenthesize(left_operand)
    } else {
        left_operand
    };
    let right =
        if binds_looser(&right_operand, op) { parenthesize(right_operand) } else { right_operand };
    Expression::BinaryOp(Box::new(BinaryOp { span: Span::default(), lhs: left, op, rhs: right }))
}

fn binds_looser(operand: &Expression, op: BinOp) -> bool {
    matches!(operand, Expression::BinaryOp(inner) if inner.op.precedence().0 < op.precedence().0)
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
    fn converts_only_the_tail_of_a_long_if_return_run() {
        let result = apply(&format!("{}return 0\n", "if a then return 1 end\n".repeat(300)));
        assert_reparses(&result, LuaVersion::Lua54);
        assert_eq!(result.matches("if").count(), 300 - MAX_ADDED_DEPTH, "{result}");
    }
}
