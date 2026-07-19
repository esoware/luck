use luck_ast::expr::*;
use luck_ast::shared::*;
use luck_ast::stmt::*;
use luck_ast::transform::AstTransform;
use luck_token::BinOp;

use crate::expr::is_always_truthy;
use crate::tokens::default_span as sp;

/// Flatten nested if-statements and convert if-return guard chains to ternary expressions.
pub fn simplify(block: Block) -> Block {
    StatementSimplifier.transform_block(block)
}

struct StatementSimplifier;

impl AstTransform for StatementSimplifier {
    fn transform_statement(&mut self, stmt: Statement) -> Statement {
        match stmt {
            // if a then if b then X end end -> if a and b then X end
            Statement::IfStatement(ref outer_if)
                if outer_if.elseif_clauses.is_empty()
                    && outer_if.else_clause.is_none()
                    && has_single_inner_if(&outer_if.block) =>
            {
                let inner_if = extract_single_inner_if(&outer_if.block)
                    .expect("has_single_inner_if guard ensures this succeeds");
                if inner_if.elseif_clauses.is_empty() && inner_if.else_clause.is_none() {
                    let combined_condition = Expression::BinaryOp(Box::new(BinaryOp {
                        span: sp(),
                        left: self.transform_expression(outer_if.condition.clone()),
                        op: BinOp::And,
                        op_span: sp(),
                        right: self.transform_expression(inner_if.condition.clone()),
                    }));
                    let merged_block = self.transform_block(inner_if.block.clone());
                    let merged_if = IfStatement {
                        span: sp(),
                        if_token: outer_if.if_token,
                        condition: combined_condition,
                        then_token: outer_if.then_token,
                        block: merged_block,
                        elseif_clauses: Vec::new(),
                        else_clause: None,
                        end_token: outer_if.end_token,
                    };
                    return self.transform_statement(Statement::IfStatement(Box::new(merged_if)));
                }
                self.walk_statement(stmt)
            }
            other => self.walk_statement(other),
        }
    }

    fn transform_block(&mut self, block: Block) -> Block {
        let block = self.walk_block(block);
        try_convert_if_return_chain(block)
    }
}

/// Converts `if c1 then return X end; if c2 then return Y end; return Z`
/// into `return c1 and X or c2 and Y or Z` when all intermediate values are truthy.
fn try_convert_if_return_chain(block: Block) -> Block {
    let last_stmt = match &block.last_stmt {
        Some(last) => match last.as_ref() {
            LastStatement::Return(ret) => ret,
            _ => return block,
        },
        _ => return block,
    };
    let return_values: Vec<_> = last_stmt.exprs.iter().collect();
    if return_values.len() != 1 {
        return block;
    }
    // `return f()` expands all of f's returns; as the `or` fallback it
    // would truncate to one value. Parenthesizing doesn't help (`or`
    // truncates regardless) - reject multi-value fallbacks outright.
    if matches!(
        return_values[0],
        Expression::FunctionCall(_) | Expression::VarArg(_)
    ) {
        return block;
    }
    let fallback_expr = return_values[0].clone();

    if block.stmts.is_empty() {
        return block;
    }

    let mut guard_count = 0;
    for stmt in block.stmts.iter().rev() {
        if matches!(stmt, Statement::EmptyStatement(_)) {
            continue;
        }
        if let Some((_condition, return_expr)) = extract_if_return_guard(stmt) {
            if !is_always_truthy(return_expr) {
                break;
            }
            guard_count += 1;
        } else {
            break;
        }
    }

    if guard_count == 0 {
        return block;
    }

    let non_empty_before_guards = block
        .stmts
        .iter()
        .rev()
        .take_while(|stmt| {
            matches!(stmt, Statement::EmptyStatement(_))
                || extract_if_return_guard(stmt)
                    .map(|(_, ret)| is_always_truthy(ret))
                    .unwrap_or(false)
        })
        .count();
    let guard_start = block.stmts.len() - non_empty_before_guards;

    let mut result_expr = fallback_expr;

    for stmt in block.stmts[guard_start..].iter().rev() {
        if matches!(stmt, Statement::EmptyStatement(_)) {
            continue;
        }
        let (condition, return_expr) = extract_if_return_guard(stmt)
            .expect("take_while guard ensures this is a valid if-return");
        let and_expr = Expression::BinaryOp(Box::new(BinaryOp {
            span: sp(),
            left: condition.clone(),
            op: BinOp::And,
            op_span: sp(),
            right: return_expr.clone(),
        }));
        result_expr = Expression::BinaryOp(Box::new(BinaryOp {
            span: sp(),
            left: and_expr,
            op: BinOp::Or,
            op_span: sp(),
            right: result_expr,
        }));
    }

    let remaining_stmts: Vec<_> = block.stmts[..guard_start].to_vec();
    let new_return = ReturnStatement {
        span: sp(),
        return_token: last_stmt.return_token,
        exprs: Punctuated::from_item(result_expr),
        semicolon: None,
    };

    Block {
        span: block.span,
        stmts: remaining_stmts,
        last_stmt: Some(Box::new(LastStatement::Return(Box::new(new_return)))),
    }
}

fn extract_if_return_guard(stmt: &Statement) -> Option<(&Expression, &Expression)> {
    let Statement::IfStatement(if_stmt) = stmt else {
        return None;
    };
    if !if_stmt.elseif_clauses.is_empty() || if_stmt.else_clause.is_some() {
        return None;
    }
    if !if_stmt.block.stmts.is_empty() {
        return None;
    }
    let last = if_stmt.block.last_stmt.as_ref()?;
    let LastStatement::Return(ret) = last.as_ref() else {
        return None;
    };
    let returns: Vec<_> = ret.exprs.iter().collect();
    if returns.len() != 1 {
        return None;
    }
    Some((&if_stmt.condition, returns[0]))
}

fn has_single_inner_if(block: &Block) -> bool {
    block.stmts.len() == 1
        && block.last_stmt.is_none()
        && matches!(&block.stmts[0], Statement::IfStatement(_))
}

fn extract_single_inner_if(block: &Block) -> Option<&IfStatement> {
    if block.stmts.len() == 1
        && block.last_stmt.is_none()
        && let Statement::IfStatement(inner) = &block.stmts[0]
    {
        return Some(inner);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn apply(source: &str) -> String {
        let result = luck_parser::parse(source, luck_token::LuaVersion::Lua54);
        assert!(result.errors.is_empty(), "parse failed");
        let block = simplify(result.block);
        luck_codegen::compact(&block, source)
    }

    #[test]
    fn flattens_nested_if() {
        let r = apply("if a then\n  if b then\n    print(1)\n  end\nend\n");
        assert!(
            r.contains("and"),
            "Expected flattened if with and, got: {r}"
        );
        let if_count = r.matches("if").count();
        assert_eq!(if_count, 1, "Expected single if, got: {r}");
    }

    #[test]
    fn if_return_to_ternary() {
        let r = apply("if x then return 1 end\nreturn 0\n");
        assert!(
            r.contains("and") && r.contains("or"),
            "Expected ternary conversion, got: {r}"
        );
    }

    #[test]
    fn no_ternary_when_falsy_return() {
        let r = apply("if x then return false end\nreturn true\n");
        assert!(
            r.contains("if"),
            "Should not convert when return value could be falsy, got: {r}"
        );
    }

    #[test]
    fn if_return_ternary_with_semicolons() {
        // Lua 5.2+ parses semicolons as EmptyStatement - must not break the chain
        let r = apply("if x then return 1 end;\nif y then return 2 end;\nreturn 0\n");
        assert!(
            r.contains("and") && r.contains("or"),
            "Semicolons between if-return guards should not prevent ternary conversion, got: {r}"
        );
    }

    #[test]
    fn if_return_ternary_variable_return() {
        // Return value is a variable - is_always_truthy returns false, so no conversion
        let r = apply("if x then return y end\nreturn 0\n");
        assert!(
            r.contains("if"),
            "Variable return values are not always truthy, got: {r}"
        );
    }

    #[test]
    fn if_return_ternary_string_literal() {
        let r = apply("if cond then return \"hello\" end\nreturn \"world\"\n");
        assert!(
            r.contains("and") && r.contains("or"),
            "String literal returns should convert to ternary, got: {r}"
        );
    }

    #[test]
    fn if_return_ternary_chained() {
        let r = apply(
            "if a then return 1 end\nif b then return 2 end\nif c then return 3 end\nreturn 0\n",
        );
        assert!(
            !r.contains("if"),
            "All chained if-return guards with truthy values should be converted, got: {r}"
        );
    }
}
