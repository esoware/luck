use core::mem;

use luck_ast::{
    BinaryOp, Block, Expression, IfStatement, LastStatement, Punctuated, ReturnStatement,
    Statement, Transform, is_multi_value,
};
use luck_token::{BinOp, Span};

use crate::expr::{ends_in_if_expr, is_always_truthy, parenthesize};

/// Flattens nested if-statements and converts if-return guard chains into
/// `and`/`or` expressions.
pub(crate) fn simplify(block: Block) -> Block {
    StatementSimplifier.transform_block(block)
}

struct StatementSimplifier;

impl Transform for StatementSimplifier {
    fn transform_stmt(&mut self, stmt: Statement) -> Statement {
        match stmt {
            Statement::IfStatement(mut outer_if) => {
                // `if a then if b then X end end` becomes `if a and b then X end`.
                // The check guarantees the pop yields the inner `if`.
                if has_no_else(&outer_if)
                    && outer_if.block.last_stmt.is_none()
                    && matches!(
                        outer_if.block.stmts.as_slice(),
                        [Statement::IfStatement(inner_if)] if has_no_else(inner_if)
                    )
                    && let Some(Statement::IfStatement(inner_if)) = outer_if.block.stmts.pop()
                {
                    let combined_condition = logical(
                        self.transform_expr(outer_if.condition),
                        BinOp::And,
                        self.transform_expr(inner_if.condition),
                    );
                    let merged_block = self.transform_block(inner_if.block);
                    let merged_if = IfStatement {
                        span: Span::default(),
                        condition: combined_condition,
                        block: merged_block,
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
        try_convert_if_return_chain(self.walk_block(block))
    }
}

/// Converts `if c1 then return X end; if c2 then return Y end; return Z`
/// into `return c1 and X or c2 and Y or Z` when `X` and `Y` are always
/// truthy.
fn try_convert_if_return_chain(mut block: Block) -> Block {
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

    let link_count = block.stmts.iter().rev().take_while(|stmt| is_chain_link(stmt)).count();
    let chain_start = block.stmts.len().saturating_sub(link_count);
    let has_guard = block
        .stmts
        .get(chain_start..)
        .is_some_and(|chain| chain.iter().any(|stmt| extract_if_return_guard(stmt).is_some()));
    if !has_guard {
        return block;
    }

    let fallback = mem::replace(fallback_expr, Expression::Nil(Span::default()));
    let result_expr = block.stmts.drain(chain_start..).rev().filter_map(into_guard_parts).fold(
        fallback,
        |otherwise, (condition, return_expr)| {
            logical(logical(condition, BinOp::And, return_expr), BinOp::Or, otherwise)
        },
    );
    **return_stmt =
        ReturnStatement { span: Span::default(), exprs: Punctuated::from_item(result_expr) };
    block
}

/// Whether a statement can sit in a trailing if-return chain: a truthy guard,
/// or an empty statement between guards.
fn is_chain_link(stmt: &Statement) -> bool {
    matches!(stmt, Statement::EmptyStatement(_))
        || extract_if_return_guard(stmt)
            .is_some_and(|(_, return_expr)| is_always_truthy(return_expr))
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

/// Takes a guard that `extract_if_return_guard` accepts apart into its
/// condition and returned value.
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

/// `left op right` for `and` and `or`, parenthesizing an operand that would
/// regroup when codegen prints the tree flat.
///
/// A nested operand of the same operator stays bare on either side, because
/// `and` and `or` are associative. An if-expression as `right` prints last, so
/// only one in `left` can swallow an operator.
fn logical(left_operand: Expression, op: BinOp, right_operand: Expression) -> Expression {
    let left = if binds_looser(&left_operand, op) || ends_in_if_expr(&left_operand) {
        parenthesize(left_operand)
    } else {
        left_operand
    };
    let right =
        if binds_looser(&right_operand, op) { parenthesize(right_operand) } else { right_operand };
    Expression::BinaryOp(Box::new(BinaryOp { span: Span::default(), left, op, right }))
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
}
