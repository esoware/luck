//! Control-flow analysis over a `Block`'s statement slice.
//!
//! Downstream lint rules use this to answer questions like "does this
//! function always return", "does this branch fall through", and
//! "what is the cyclomatic complexity of this body". The walker is
//! intentionally a pure function over `&[Statement]`: it builds no
//! persistent graph and allocates nothing beyond stack frames.
//!
//! Function literals (`Expression::FunctionDef`, `Statement::FunctionDecl`,
//! `Statement::LocalFunction`) are walked into for completeness but their
//! exits do NOT propagate into the enclosing block: a function value
//! defined inline does not change the surrounding control flow.

use luck_ast::expr::{Expression, FunctionArgs, FunctionCall, Var};
use luck_ast::shared::Block;
use luck_ast::stmt::{LastStatement, Statement};
use luck_token::BinOp;
use luck_token::TokenKind;

/// How a statement sequence terminates.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Exit {
    #[default]
    Normal,
    Return,
    Break,
    Continue,
    /// Unconditional `error(...)` call or provably infinite loop.
    /// Treated as non-returning for unreachable analysis.
    Error,
}

/// Per-branch information for control-flow analysis.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct BranchSummary {
    pub exit: Exit,
    pub always_returns: bool,
    pub may_return: bool,
    pub statement_count: usize,
    pub decision_points: u32,
}

/// Walk a full `Block` (statements plus optional `last_stmt`).
pub fn analyze_full_block(block: &Block) -> BranchSummary {
    analyze_sequence(&block.stmts, block.last_stmt.as_deref())
}

fn analyze_sequence(stmts: &[Statement], last: Option<&LastStatement>) -> BranchSummary {
    let mut summary = BranchSummary::default();
    let mut terminated = false;

    for stmt in stmts {
        summary.statement_count += 1;
        let stmt_summary = analyze_statement(stmt);

        summary.decision_points += stmt_summary.decision_points;
        if stmt_summary.may_return {
            summary.may_return = true;
        }

        if terminated {
            // Already non-Normal; later statements are unreachable and
            // must not change the enclosing exit, but their decision
            // points still count (a rule may want McCabe over the whole
            // body, dead code or not).
            continue;
        }

        if !matches!(stmt_summary.exit, Exit::Normal) {
            summary.exit = stmt_summary.exit;
            terminated = true;
        }
    }

    if !terminated {
        if let Some(last_stmt) = last {
            summary.statement_count += 1;
            let exit = match last_stmt {
                LastStatement::Return(_) => Exit::Return,
                LastStatement::Break(_) => Exit::Break,
                LastStatement::Continue(_) => Exit::Continue,
                LastStatement::Error(_) => Exit::Normal,
            };
            if matches!(exit, Exit::Return) {
                summary.may_return = true;
            }
            summary.exit = exit;
        }
    }

    summary.always_returns = matches!(summary.exit, Exit::Return | Exit::Error);
    summary
}

fn analyze_statement(stmt: &Statement) -> BranchSummary {
    let mut summary = BranchSummary {
        statement_count: 1,
        ..BranchSummary::default()
    };

    match stmt {
        Statement::Assignment(node) => {
            summary.decision_points += count_decision_points_punctuated_exprs(&node.values);
        }
        Statement::FunctionCall(node) => {
            let is_unconditional_error = call_is_error(&node.call);
            summary.decision_points += count_decision_points_call(&node.call);
            if is_unconditional_error {
                summary.exit = Exit::Error;
            }
        }
        Statement::DoBlock(node) => {
            let inner = analyze_full_block(&node.block);
            summary.exit = inner.exit;
            summary.may_return |= inner.may_return;
            summary.decision_points += inner.decision_points;
        }
        Statement::WhileLoop(node) => {
            summary.decision_points += 1;
            summary.decision_points += count_decision_points_expression(&node.condition);
            let body = analyze_full_block(&node.block);
            summary.may_return |= body.may_return;
            summary.decision_points += body.decision_points;
            // `while true do ... end` never falls through unless an inner
            // `break` is reachable. The walker can't see through the body
            // statically beyond the per-branch summary, but if the body
            // always returns/errors then so does the loop.
            if is_literal_true(&node.condition) {
                summary.exit = match body.exit {
                    Exit::Return => Exit::Return,
                    Exit::Break => Exit::Normal,
                    Exit::Error => Exit::Error,
                    // A body that "continues" or runs normally yields an
                    // infinite loop, unreachable from the outside.
                    Exit::Continue | Exit::Normal => Exit::Error,
                };
            }
        }
        Statement::RepeatLoop(node) => {
            summary.decision_points += 1;
            summary.decision_points += count_decision_points_expression(&node.condition);
            let body = analyze_full_block(&node.block);
            summary.may_return |= body.may_return;
            summary.decision_points += body.decision_points;
            if is_literal_false(&node.condition) {
                // `repeat ... until false` is equivalent to `while true do ... end`.
                summary.exit = match body.exit {
                    Exit::Return => Exit::Return,
                    Exit::Break => Exit::Normal,
                    Exit::Error => Exit::Error,
                    Exit::Continue | Exit::Normal => Exit::Error,
                };
            } else if is_literal_true(&node.condition) {
                // `repeat ... until true` runs the body exactly once.
                summary.exit = match body.exit {
                    Exit::Break => Exit::Normal,
                    other => other,
                };
            } else {
                // Body executes at least once; a return/error in the body
                // still terminates the loop. A break is consumed.
                summary.exit = match body.exit {
                    Exit::Return => Exit::Return,
                    Exit::Error => Exit::Error,
                    Exit::Break | Exit::Continue | Exit::Normal => Exit::Normal,
                };
            }
        }
        Statement::IfStatement(node) => {
            // +1 for the if itself, +1 per elseif. The `else` is the
            // fallthrough, not a decision point in McCabe's definition.
            summary.decision_points += 1;
            summary.decision_points += count_decision_points_expression(&node.condition);
            for clause in &node.elseif_clauses {
                summary.decision_points += 1;
                summary.decision_points += count_decision_points_expression(&clause.condition);
            }

            let then_summary = analyze_full_block(&node.block);
            summary.may_return |= then_summary.may_return;
            summary.decision_points += then_summary.decision_points;

            let mut branch_exits: Vec<Exit> = vec![then_summary.exit];
            for clause in &node.elseif_clauses {
                let clause_summary = analyze_full_block(&clause.block);
                summary.may_return |= clause_summary.may_return;
                summary.decision_points += clause_summary.decision_points;
                branch_exits.push(clause_summary.exit);
            }

            if let Some(else_clause) = &node.else_clause {
                let else_summary = analyze_full_block(&else_clause.block);
                summary.may_return |= else_summary.may_return;
                summary.decision_points += else_summary.decision_points;
                branch_exits.push(else_summary.exit);
                summary.exit = combine_branches(&branch_exits);
            } else {
                // Implicit fallthrough is Normal, so the if-without-else
                // can never terminate the enclosing sequence.
                summary.exit = Exit::Normal;
            }
        }
        Statement::NumericFor(node) => {
            summary.decision_points += 1;
            summary.decision_points += count_decision_points_expression(&node.start);
            summary.decision_points += count_decision_points_expression(&node.limit);
            if let Some((_, step)) = &node.comma2_and_step {
                summary.decision_points += count_decision_points_expression(step);
            }
            let body = analyze_full_block(&node.block);
            summary.may_return |= body.may_return;
            summary.decision_points += body.decision_points;
            // The body may execute zero times; the loop itself falls
            // through normally.
            summary.exit = Exit::Normal;
        }
        Statement::GenericFor(node) => {
            summary.decision_points += 1;
            summary.decision_points += count_decision_points_punctuated_exprs(&node.exprs);
            let body = analyze_full_block(&node.block);
            summary.may_return |= body.may_return;
            summary.decision_points += body.decision_points;
            summary.exit = Exit::Normal;
        }
        // Function declarations are isolated control-flow units: their
        // exits and decision points belong to the function, not the
        // enclosing block. Callers that need per-function complexity
        // run `analyze_full_block` on the body directly.
        Statement::FunctionDecl(_) => {}
        Statement::LocalFunction(_) => {}
        Statement::LocalAssignment(node) => {
            if let Some((_, exprs)) = &node.equal_and_exprs {
                summary.decision_points += count_decision_points_punctuated_exprs(exprs);
            }
        }
        Statement::EmptyStatement(_) => {}
        Statement::Goto(_) => {}
        Statement::Label(_) => {}
        Statement::GlobalDeclaration(node) => {
            if let Some((_, exprs)) = &node.equal_and_exprs {
                summary.decision_points += count_decision_points_punctuated_exprs(exprs);
            }
        }
        Statement::GlobalFunction(_) => {}
        Statement::GlobalStar(_) => {}
        // Lua 5.2+: bare `break` statement.
        Statement::Break(_) => {
            summary.exit = Exit::Break;
        }
        Statement::CompoundAssignment(node) => {
            summary.decision_points += count_decision_points_expression(&node.expr);
        }
        Statement::TypeDeclaration(_) => {}
        Statement::Error(_) => {}
    }

    summary
}

/// Combine branch exits using the "weakest matching exit" rule.
///
/// Returns the common exit kind iff every branch shares it; otherwise
/// `Exit::Normal` (the path that takes the disagreeing branch falls
/// through). Error is treated as a stronger non-return than Return only
/// when paired with itself; mixed Return/Error collapses to Return
/// because both prevent fallthrough.
fn combine_branches(exits: &[Exit]) -> Exit {
    if exits.is_empty() {
        return Exit::Normal;
    }
    let first = exits[0];
    let all_non_normal = exits.iter().all(|exit| !matches!(exit, Exit::Normal));
    if !all_non_normal {
        return Exit::Normal;
    }
    let all_returnish = exits
        .iter()
        .all(|exit| matches!(exit, Exit::Return | Exit::Error));
    if all_returnish {
        // Prefer Return as the dominant label - Error is rare and merging
        // it into Return preserves "always_returns" without losing info.
        return if exits.iter().all(|exit| matches!(exit, Exit::Error)) {
            Exit::Error
        } else {
            Exit::Return
        };
    }
    if exits.iter().all(|exit| *exit == first) {
        return first;
    }
    // Mixed non-Normal exits (e.g. one branch breaks, another returns):
    // the caller can't pick a single label, so report the weakest
    // observable property. Treat as Normal since the enclosing sequence
    // may continue along the break path once consumed by a loop.
    Exit::Normal
}

fn is_literal_true(expr: &Expression) -> bool {
    matches!(expr, Expression::True(_))
}

fn is_literal_false(expr: &Expression) -> bool {
    matches!(expr, Expression::False(_))
}

/// Direct call to the `error` global with no receiver method.
fn call_is_error(call: &FunctionCall) -> bool {
    if call.method.is_some() {
        return false;
    }
    let Expression::Var(var) = &call.callee else {
        return false;
    };
    let Var::Name(token) = var.as_ref() else {
        return false;
    };
    matches!(&token.kind, TokenKind::Identifier(name) if name.as_str() == "error")
}

fn count_decision_points_punctuated_exprs(exprs: &luck_ast::shared::Punctuated<Expression>) -> u32 {
    exprs.iter().map(count_decision_points_expression).sum()
}

fn count_decision_points_call(call: &FunctionCall) -> u32 {
    let mut total = count_decision_points_expression(&call.callee);
    match &call.args {
        FunctionArgs::Parenthesized { args, .. } => {
            total += count_decision_points_punctuated_exprs(args);
        }
        FunctionArgs::TableConstructor(_) => {}
        FunctionArgs::StringLiteral(_) => {}
    }
    total
}

/// McCabe-style decision-point count for an expression. Every short-circuit
/// `and`/`or` adds 1 (each adds an independent path); a Luau `if/elseif/else`
/// expression adds 1 per `if`/`elseif`.
fn count_decision_points_expression(expr: &Expression) -> u32 {
    match expr {
        Expression::Nil(_) => 0,
        Expression::False(_) => 0,
        Expression::True(_) => 0,
        Expression::Number(_) => 0,
        Expression::StringLiteral(_) => 0,
        Expression::VarArg(_) => 0,
        Expression::FunctionDef(_) => 0,
        Expression::Var(var) => count_decision_points_var(var),
        Expression::FunctionCall(call) => count_decision_points_call(call),
        Expression::Parenthesized(node) => count_decision_points_expression(&node.expr),
        Expression::TableConstructor(node) => {
            let mut total = 0;
            for (field, _) in &node.fields {
                match field {
                    luck_ast::shared::Field::Bracketed { key, value, .. } => {
                        total += count_decision_points_expression(key);
                        total += count_decision_points_expression(value);
                    }
                    luck_ast::shared::Field::Named { value, .. } => {
                        total += count_decision_points_expression(value);
                    }
                    luck_ast::shared::Field::Positional { value, .. } => {
                        total += count_decision_points_expression(value);
                    }
                }
            }
            total
        }
        Expression::BinaryOp(node) => {
            let self_points = match node.op {
                BinOp::And | BinOp::Or => 1,
                _ => 0,
            };
            self_points
                + count_decision_points_expression(&node.left)
                + count_decision_points_expression(&node.right)
        }
        Expression::UnaryOp(node) => count_decision_points_expression(&node.operand),
        // Luau: each `if`/`elseif` in an if-expression is a branch point.
        Expression::IfExpression(node) => {
            let mut total = 1;
            total += count_decision_points_expression(&node.condition);
            total += count_decision_points_expression(&node.then_expr);
            for clause in &node.elseif_clauses {
                total += 1;
                total += count_decision_points_expression(&clause.condition);
                total += count_decision_points_expression(&clause.expr);
            }
            total += count_decision_points_expression(&node.else_expr);
            total
        }
        Expression::InterpolatedString(node) => {
            let mut total = 0;
            for segment in &node.segments {
                if let Some(expr) = &segment.expr {
                    total += count_decision_points_expression(expr);
                }
            }
            total
        }
        Expression::TypeCast(node) => count_decision_points_expression(&node.expr),
        Expression::Error(_) => 0,
    }
}

fn count_decision_points_var(var: &Var) -> u32 {
    match var {
        Var::Name(_) => 0,
        Var::Index(node) => {
            count_decision_points_expression(&node.prefix)
                + count_decision_points_expression(&node.index)
        }
        Var::FieldAccess(node) => count_decision_points_expression(&node.prefix),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use luck_token::LuaVersion;

    fn parse_block(source: &str) -> Block {
        let result = luck_parser::parse(source, LuaVersion::Lua54);
        assert!(
            result.errors.is_empty(),
            "parser errors for {source:?}: {:?}",
            result.errors
        );
        result.block
    }

    fn summarize(source: &str) -> BranchSummary {
        let block = parse_block(source);
        analyze_full_block(&block)
    }

    #[test]
    fn empty_block_is_normal() {
        let summary = summarize("");
        assert_eq!(summary.exit, Exit::Normal);
        assert_eq!(summary.statement_count, 0);
        assert!(!summary.always_returns);
        assert!(!summary.may_return);
    }

    #[test]
    fn bare_return_terminates() {
        let summary = summarize("return 1");
        assert_eq!(summary.exit, Exit::Return);
        assert!(summary.always_returns);
        assert!(summary.may_return);
    }

    #[test]
    fn if_without_else_does_not_always_return() {
        let summary = summarize("local c = 1\nif c then return 1 end");
        assert_eq!(summary.exit, Exit::Normal);
        assert!(!summary.always_returns);
        assert!(summary.may_return);
    }

    #[test]
    fn if_else_both_return() {
        let summary = summarize("local c = 1\nif c then return 1 else return 2 end");
        assert_eq!(summary.exit, Exit::Return);
        assert!(summary.always_returns);
        assert!(summary.may_return);
    }

    #[test]
    fn if_elseif_else_all_return() {
        let summary = summarize(
            "local c = 1\nlocal d = 1\nif c then return 1 elseif d then return 2 else return 3 end",
        );
        assert_eq!(summary.exit, Exit::Return);
        assert!(summary.always_returns);
    }

    #[test]
    fn while_true_no_break_is_error_exit() {
        let summary = summarize("while true do print(1) end");
        // Infinite loop: never falls through, treated like Error for
        // unreachable analysis. `always_returns` is true since Error
        // counts as non-falling-through.
        assert_eq!(summary.exit, Exit::Error);
        assert!(summary.always_returns);
        assert!(!summary.may_return);
    }

    #[test]
    fn while_true_with_return_returns() {
        let summary = summarize("while true do return 1 end");
        assert_eq!(summary.exit, Exit::Return);
        assert!(summary.always_returns);
        assert!(summary.may_return);
    }

    #[test]
    fn repeat_until_true_executes_once() {
        let summary = summarize("repeat return 1 until true");
        assert_eq!(summary.exit, Exit::Return);
        assert!(summary.may_return);
    }

    #[test]
    fn numeric_for_may_not_execute() {
        let summary = summarize("for i = 1, 10 do return 1 end");
        // Loop body may not execute (start > limit at runtime), so the
        // loop itself falls through normally, even though it MAY return.
        assert_eq!(summary.exit, Exit::Normal);
        assert!(!summary.always_returns);
        assert!(summary.may_return);
    }

    #[test]
    fn do_block_return_propagates() {
        let summary = summarize("do return end");
        assert_eq!(summary.exit, Exit::Return);
        assert!(summary.always_returns);
    }

    #[test]
    fn unconditional_error_is_error_exit() {
        let summary = summarize("error(\"x\")");
        assert_eq!(summary.exit, Exit::Error);
        assert!(summary.always_returns);
    }

    #[test]
    fn break_in_inner_loop_is_consumed() {
        let summary = summarize("local c = 1\nwhile c do if c then break end end");
        // The break belongs to the while; the enclosing block falls
        // through normally.
        assert_eq!(summary.exit, Exit::Normal);
        assert!(!summary.always_returns);
    }

    #[test]
    fn decision_points_empty_block() {
        let summary = summarize("");
        // McCabe baseline of 1 is added by the caller (the rule); the
        // walker reports 0 decision points for an empty body.
        assert_eq!(summary.decision_points, 0);
    }

    #[test]
    fn decision_points_single_if() {
        let summary = summarize("local c = 1\nif c then end");
        // +1 for the if statement.
        assert_eq!(summary.decision_points, 1);
    }

    #[test]
    fn decision_points_if_with_and() {
        let summary = summarize("local a = 1\nlocal b = 1\nif a and b then end");
        // +1 for the if, +1 for the `and`.
        assert_eq!(summary.decision_points, 2);
    }

    #[test]
    fn decision_points_if_elseif() {
        let summary = summarize("local a = 1\nlocal b = 1\nif a then elseif b then end");
        // +1 for the if, +1 for the elseif.
        assert_eq!(summary.decision_points, 2);
    }

    #[test]
    fn decision_points_nested_elseif_chain() {
        let summary = summarize(
            "local a = 1\nlocal b = 1\nlocal c = 1\nif a then elseif b then elseif c then end",
        );
        // +1 for the if, +1 per elseif (two of them) = 3.
        assert_eq!(summary.decision_points, 3);
    }
}
