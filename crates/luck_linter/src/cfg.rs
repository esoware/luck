//! Control-flow analysis over a `Block`'s statement slice.
//!
//! Rules use this to ask whether a function always returns, whether a loop
//! body always exits, and how many decision points a body has. The walker is
//! a pure function over `&[Statement]` and builds no graph.
//!
//! It doesn't descend into nested function bodies. Their exits and decision
//! points belong to that function, not to the enclosing block.

use core::num::Saturating;

use luck_ast::{
    Block, Expression, Field, FunctionArgs, FunctionCall, IfStatement, LastStatement, Punctuated,
    Statement, Var,
};
use luck_token::BinOp;

/// How a statement sequence terminates.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) enum Exit {
    #[default]
    Normal,
    Return,
    Break,
    Continue,
    /// An unconditional `error(...)` call or an infinite loop. Neither falls
    /// through, so both count toward `always_returns()`.
    Error,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct BranchSummary {
    pub exit: Exit,
    pub can_return: bool,
    pub decision_points: u32,
}

impl BranchSummary {
    /// Whether the sequence never falls through: it returns, errors, or
    /// loops forever.
    pub(crate) const fn always_returns(self) -> bool {
        match self.exit {
            Exit::Return | Exit::Error => true,
            Exit::Normal | Exit::Break | Exit::Continue => false,
        }
    }

    const fn add_decision_points(&mut self, points: u32) {
        self.decision_points = self.decision_points.saturating_add(points);
    }

    /// Count a branch point on `condition`, plus any inside the condition.
    fn add_branch(&mut self, condition: &Expression) {
        self.add_decision_points(1);
        self.add_decision_points(count_decision_points_expr(condition));
    }

    /// Fold in a nested block. Its exit stays with the caller, because what
    /// that exit means depends on the construct holding the block.
    const fn absorb(&mut self, nested: Self) {
        self.can_return |= nested.can_return;
        self.add_decision_points(nested.decision_points);
    }
}

pub(crate) fn analyze_full_block(block: &Block) -> BranchSummary {
    analyze_sequence(&block.stmts, block.last_stmt.as_deref())
}

fn analyze_sequence(stmts: &[Statement], last: Option<&LastStatement>) -> BranchSummary {
    let mut summary = BranchSummary::default();
    let mut is_terminated = false;

    for stmt in stmts {
        let stmt_summary = analyze_stmt(stmt);

        summary.add_decision_points(stmt_summary.decision_points);
        summary.can_return |= stmt_summary.can_return;

        if is_terminated {
            // Unreachable statements can't change the exit, but their
            // decision points still count toward the body's complexity.
            continue;
        }

        if !matches!(stmt_summary.exit, Exit::Normal) {
            summary.exit = stmt_summary.exit;
            is_terminated = true;
        }
    }

    if !is_terminated && let Some(last_stmt) = last {
        let exit = match last_stmt {
            LastStatement::Return(_) => Exit::Return,
            LastStatement::Break(_) => Exit::Break,
            // Luau
            LastStatement::Continue(_) => Exit::Continue,
            LastStatement::Error(_) => Exit::Normal,
        };
        if matches!(exit, Exit::Return) {
            summary.can_return = true;
        }
        summary.exit = exit;
    }

    summary
}

fn analyze_stmt(stmt: &Statement) -> BranchSummary {
    let mut summary = BranchSummary::default();

    match stmt {
        Statement::Assignment(node) => {
            summary.add_decision_points(count_decision_points_punctuated_exprs(&node.values));
        }
        Statement::FunctionCall(node) => {
            let is_unconditional_error = call_is_error(node);
            summary.add_decision_points(count_decision_points_call(node));
            if is_unconditional_error {
                summary.exit = Exit::Error;
            }
        }
        Statement::DoBlock(node) => {
            let inner = analyze_full_block(&node.block);
            summary.exit = inner.exit;
            summary.absorb(inner);
        }
        Statement::WhileLoop(node) => {
            summary.add_branch(&node.condition);
            let body = analyze_full_block(&node.block);
            summary.absorb(body);
            // `while true` only ends through `break`, but the body summary
            // shows a break only when every path takes it. A conditional
            // break goes unseen, and the loop counts as infinite.
            if is_literal_true(&node.condition) {
                summary.exit = infinite_loop_exit(body.exit);
            }
        }
        Statement::RepeatLoop(node) => {
            summary.add_branch(&node.condition);
            let body = analyze_full_block(&node.block);
            summary.absorb(body);
            summary.exit = if is_literal_false(&node.condition) {
                // `repeat ... until false` is equivalent to `while true do ... end`.
                infinite_loop_exit(body.exit)
            } else if is_literal_true(&node.condition) {
                // `repeat ... until true` runs the body exactly once.
                match body.exit {
                    Exit::Break => Exit::Normal,
                    exit @ (Exit::Normal | Exit::Return | Exit::Continue | Exit::Error) => exit,
                }
            } else {
                // The body runs at least once, so its return or error still
                // applies. A break is consumed.
                match body.exit {
                    Exit::Return => Exit::Return,
                    Exit::Error => Exit::Error,
                    Exit::Break | Exit::Continue | Exit::Normal => Exit::Normal,
                }
            };
        }
        Statement::IfStatement(node) => analyze_if(node, &mut summary),
        Statement::NumericFor(node) => {
            summary.add_branch(&node.start);
            summary.add_decision_points(count_decision_points_expr(&node.limit));
            if let Some(step) = &node.step {
                summary.add_decision_points(count_decision_points_expr(step));
            }
            summary.absorb(analyze_full_block(&node.block));
            // The body may run zero times, so the loop falls through.
            summary.exit = Exit::Normal;
        }
        Statement::GenericFor(node) => {
            summary.add_decision_points(1);
            summary.add_decision_points(count_decision_points_punctuated_exprs(&node.exprs));
            summary.absorb(analyze_full_block(&node.block));
            summary.exit = Exit::Normal;
        }
        Statement::LocalAssignment(node) => {
            if let Some(exprs) = &node.exprs {
                summary.add_decision_points(count_decision_points_punctuated_exprs(exprs));
            }
        }
        // Lua 5.5
        Statement::GlobalDeclaration(node) => {
            if let Some(exprs) = &node.exprs {
                summary.add_decision_points(count_decision_points_punctuated_exprs(exprs));
            }
        }
        // Lua 5.2+: `break` as an ordinary statement.
        Statement::Break(_) => {
            summary.exit = Exit::Break;
        }
        // Luau
        Statement::CompoundAssignment(node) => {
            summary.add_decision_points(count_decision_points_expr(&node.expr));
        }
        // A function's exits and decision points are its own, so callers
        // analyze each body separately. The walker doesn't model `goto`,
        // and the other statements here don't affect control flow.
        Statement::FunctionDecl(_)
        | Statement::LocalFunction(_)
        | Statement::GlobalFunction(_)
        | Statement::EmptyStatement(_)
        | Statement::Goto(_)
        | Statement::Label(_)
        | Statement::GlobalStar(_)
        | Statement::TypeDeclaration(_)
        | Statement::Error(_) => {}
    }

    summary
}

fn analyze_if(node: &IfStatement, summary: &mut BranchSummary) {
    // +1 for the if itself, +1 per elseif. The `else` is the
    // fallthrough, not a decision point in McCabe's definition.
    summary.add_branch(&node.condition);
    for clause in &node.elseif_clauses {
        summary.add_branch(&clause.condition);
    }

    let then_summary = analyze_full_block(&node.block);
    summary.absorb(then_summary);

    let mut branch_exits: Vec<Exit> = vec![then_summary.exit];
    for clause in &node.elseif_clauses {
        let clause_summary = analyze_full_block(&clause.block);
        summary.absorb(clause_summary);
        branch_exits.push(clause_summary.exit);
    }

    if let Some(else_clause) = &node.else_clause {
        let else_summary = analyze_full_block(&else_clause.block);
        summary.absorb(else_summary);
        branch_exits.push(else_summary.exit);
        summary.exit = combine_branches(&branch_exits);
    } else {
        // Without an `else`, it falls through when no condition holds.
        summary.exit = Exit::Normal;
    }
}

/// Exit of a loop whose condition never ends it, given how its body exits.
const fn infinite_loop_exit(body_exit: Exit) -> Exit {
    match body_exit {
        Exit::Return => Exit::Return,
        Exit::Break => Exit::Normal,
        // An erroring body errors, and any other body loops forever, which
        // counts as `Error` too.
        Exit::Error | Exit::Continue | Exit::Normal => Exit::Error,
    }
}

/// Combines the exits of an if statement's branches, `else` included.
///
/// Branches that all share an exit give that exit, and a mix of `Return` and
/// `Error` gives `Return`, since neither falls through. Any other mix gives
/// `Exit::Normal`.
fn combine_branches(exits: &[Exit]) -> Exit {
    let Some(&first) = exits.first() else {
        return Exit::Normal;
    };
    let has_normal_exit = exits.iter().any(|exit| matches!(exit, Exit::Normal));
    if has_normal_exit {
        return Exit::Normal;
    }
    let is_every_exit_return_or_error =
        exits.iter().all(|exit| matches!(exit, Exit::Return | Exit::Error));
    if is_every_exit_return_or_error {
        return if exits.iter().all(|exit| matches!(exit, Exit::Error)) {
            Exit::Error
        } else {
            Exit::Return
        };
    }
    if exits.iter().all(|exit| *exit == first) {
        return first;
    }
    // No single exit fits a mix like break-or-return, and `Normal` is the
    // conservative answer.
    Exit::Normal
}

const fn is_literal_true(expr: &Expression) -> bool {
    matches!(expr, Expression::True(_))
}

const fn is_literal_false(expr: &Expression) -> bool {
    matches!(expr, Expression::False(_))
}

/// Whether `call` is a plain `error(...)` call. A local that shadows `error`
/// still matches.
fn call_is_error(call: &FunctionCall) -> bool {
    if call.method.is_some() {
        return false;
    }
    let Expression::Var(Var::Name(token)) = &call.callee else {
        return false;
    };
    token.identifier() == Some("error")
}

fn count_decision_points_punctuated_exprs(exprs: &Punctuated<Expression>) -> u32 {
    exprs.iter().map(count_decision_points_expr).sum()
}

fn count_decision_points_call(call: &FunctionCall) -> u32 {
    let mut total = Saturating(count_decision_points_expr(&call.callee));
    match &call.args {
        FunctionArgs::Parenthesized { args, .. } => {
            total += count_decision_points_punctuated_exprs(args);
        }
        FunctionArgs::TableConstructor(_) | FunctionArgs::StringLiteral(_) => {}
    }
    total.0
}

/// McCabe-style decision points in an expression.
///
/// Each `and`/`or` adds 1, as does each `if`/`elseif` of a Luau
/// if-expression. Function literals add nothing; their points are their own.
fn count_decision_points_expr(expr: &Expression) -> u32 {
    match expr {
        Expression::Nil(_)
        | Expression::False(_)
        | Expression::True(_)
        | Expression::Number(_)
        | Expression::Integer(_)
        | Expression::StringLiteral(_)
        | Expression::VarArg(_)
        | Expression::FunctionDef(_)
        | Expression::Error(_) => 0,
        Expression::Var(var) => count_decision_points_var(var),
        Expression::FunctionCall(call) => count_decision_points_call(call),
        Expression::Parenthesized(node) => count_decision_points_expr(&node.expr),
        Expression::TableConstructor(node) => {
            let mut total = Saturating(0u32);
            for field in node.fields.iter() {
                match field {
                    Field::Bracketed { key, value, .. } => {
                        total += count_decision_points_expr(key);
                        total += count_decision_points_expr(value);
                    }
                    Field::Named { value, .. } | Field::Positional { value, .. } => {
                        total += count_decision_points_expr(value);
                    }
                }
            }
            total.0
        }
        Expression::BinaryOp(node) => {
            let mut total = Saturating(u32::from(matches!(node.op, BinOp::And | BinOp::Or)));
            total += count_decision_points_expr(&node.left);
            total += count_decision_points_expr(&node.right);
            total.0
        }
        Expression::UnaryOp(node) => count_decision_points_expr(&node.operand),
        // Luau: each `if`/`elseif` in an if-expression is a branch point.
        Expression::IfExpression(node) => {
            let mut total = Saturating(1u32);
            total += count_decision_points_expr(&node.condition);
            total += count_decision_points_expr(&node.then_expr);
            for clause in &node.elseif_clauses {
                total += 1;
                total += count_decision_points_expr(&clause.condition);
                total += count_decision_points_expr(&clause.expr);
            }
            total += count_decision_points_expr(&node.else_expr);
            total.0
        }
        // Luau
        Expression::InterpolatedString(node) => node
            .segments
            .iter()
            .filter_map(|segment| segment.expr.as_ref())
            .map(count_decision_points_expr)
            .sum(),
        // Luau
        Expression::TypeCast(node) => count_decision_points_expr(&node.expr),
        // Luau
        Expression::TypeInstantiation(node) => count_decision_points_expr(&node.expr),
    }
}

fn count_decision_points_var(var: &Var) -> u32 {
    match var {
        Var::Name(_) => 0,
        Var::Index(node) => count_decision_points_expr(&node.prefix)
            .saturating_add(count_decision_points_expr(&node.index)),
        Var::FieldAccess(node) => count_decision_points_expr(&node.prefix),
    }
}

#[cfg(test)]
mod tests {
    use luck_token::LuaVersion;

    use super::*;

    fn parse_block(source: &str) -> Block {
        let parse = luck_parser::parse(source, LuaVersion::Lua54);
        assert!(parse.errors.is_empty(), "parser errors for {source:?}: {:?}", parse.errors);
        parse.block
    }

    fn summarize(source: &str) -> BranchSummary {
        let block = parse_block(source);
        analyze_full_block(&block)
    }

    #[test]
    fn empty_block_is_normal() {
        let summary = summarize("");
        assert_eq!(summary.exit, Exit::Normal);
        assert!(!summary.always_returns());
        assert!(!summary.can_return);
    }

    #[test]
    fn bare_return_terminates() {
        let summary = summarize("return 1");
        assert_eq!(summary.exit, Exit::Return);
        assert!(summary.always_returns());
        assert!(summary.can_return);
    }

    #[test]
    fn if_without_else_does_not_always_return() {
        let summary = summarize("local c = 1\nif c then return 1 end");
        assert_eq!(summary.exit, Exit::Normal);
        assert!(!summary.always_returns());
        assert!(summary.can_return);
    }

    #[test]
    fn if_else_both_return() {
        let summary = summarize("local c = 1\nif c then return 1 else return 2 end");
        assert_eq!(summary.exit, Exit::Return);
        assert!(summary.always_returns());
        assert!(summary.can_return);
    }

    #[test]
    fn if_elseif_else_all_return() {
        let summary = summarize(
            "local c = 1\nlocal d = 1\nif c then return 1 elseif d then return 2 else return 3 end",
        );
        assert_eq!(summary.exit, Exit::Return);
        assert!(summary.always_returns());
    }

    #[test]
    fn while_true_no_break_is_error_exit() {
        let summary = summarize("while true do print(1) end");
        // An infinite loop never falls through, so it counts as `Error` and
        // `always_returns` holds.
        assert_eq!(summary.exit, Exit::Error);
        assert!(summary.always_returns());
        assert!(!summary.can_return);
    }

    #[test]
    fn while_true_with_return_returns() {
        let summary = summarize("while true do return 1 end");
        assert_eq!(summary.exit, Exit::Return);
        assert!(summary.always_returns());
        assert!(summary.can_return);
    }

    #[test]
    fn repeat_until_true_executes_once() {
        let summary = summarize("repeat return 1 until true");
        assert_eq!(summary.exit, Exit::Return);
        assert!(summary.can_return);
    }

    #[test]
    fn numeric_for_may_not_execute() {
        let summary = summarize("for i = 1, 10 do return 1 end");
        // The body may run zero times, so the loop falls through even
        // though it may return.
        assert_eq!(summary.exit, Exit::Normal);
        assert!(!summary.always_returns());
        assert!(summary.can_return);
    }

    #[test]
    fn do_block_return_propagates() {
        let summary = summarize("do return end");
        assert_eq!(summary.exit, Exit::Return);
        assert!(summary.always_returns());
    }

    #[test]
    fn unconditional_error_is_error_exit() {
        let summary = summarize("error(\"x\")");
        assert_eq!(summary.exit, Exit::Error);
        assert!(summary.always_returns());
    }

    #[test]
    fn break_in_inner_loop_is_consumed() {
        let summary = summarize("local c = 1\nwhile c do if c then break end end");
        // The break belongs to the while; the enclosing block falls
        // through normally.
        assert_eq!(summary.exit, Exit::Normal);
        assert!(!summary.always_returns());
    }

    #[test]
    fn decision_points_empty_block() {
        let summary = summarize("");
        // The rule adds McCabe's baseline of 1; the walker reports 0.
        assert_eq!(summary.decision_points, 0);
    }

    #[test]
    fn decision_points_single_if() {
        let summary = summarize("local c = 1\nif c then end");
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
