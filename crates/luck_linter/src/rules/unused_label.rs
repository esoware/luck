use luck_ast::Expression;
use luck_ast::expr::Var;
use luck_ast::shared::{Block, Field, FunctionBody};
use luck_ast::stmt::{LastStatement, Statement};
use luck_token::{Span, TokenKind};

use crate::diagnostic::*;
use crate::rule::{LintContext, Rule};

/// Labels (`::name::`) that are never targeted by a `goto` in the same
/// function. `goto` cannot cross a function boundary in Lua, so a label
/// in one function and a goto in another are unrelated. The check scans
/// label/goto pairs per function scope, treating the chunk's top-level
/// block as the implicit "main" function.
pub struct UnusedLabel;

impl Rule for UnusedLabel {
    fn name(&self) -> &'static str {
        "unused_label"
    }
    fn category(&self) -> Category {
        Category::Style
    }
    fn default_severity(&self) -> Severity {
        Severity::Warning
    }
    fn description(&self) -> &'static str {
        "label declared but never targeted by a goto in the same function"
    }

    fn check(&self, ctx: &LintContext) -> Vec<LintDiagnostic> {
        let mut diagnostics = Vec::new();
        check_scope(ctx.block, &mut diagnostics);
        diagnostics
    }
}

/// Process one function scope: collect every label and every goto name
/// reachable without crossing a nested function boundary, then emit a
/// diagnostic for every label whose name has no matching goto.
fn check_scope<'ast>(block: &'ast Block, diagnostics: &mut Vec<LintDiagnostic>) {
    let mut labels: Vec<(&'ast str, Span)> = Vec::new();
    let mut goto_names: Vec<&'ast str> = Vec::new();
    let mut nested: Vec<&'ast FunctionBody> = Vec::new();

    walk_block(block, &mut labels, &mut goto_names, &mut nested);

    for (name, span) in &labels {
        // A label is reachable if any goto in the same function uses it.
        // We don't try to enforce Lua's own scoping rules (the parser
        // already validates them); presence anywhere in the same function
        // suffices to mute this lint and avoid false positives on
        // cross-block jumps.
        if !goto_names.iter().any(|goto| goto == name) {
            diagnostics.push(
                LintDiagnostic::new(
                    "unused_label",
                    format!("label `{name}` is never targeted by a goto"),
                    *span,
                )
                .with_help("remove the label or add a `goto` that targets it".to_string()),
            );
        }
    }

    for body in nested {
        check_scope(&body.block, diagnostics);
    }
}

fn token_identifier(kind: &TokenKind) -> Option<&str> {
    if let TokenKind::Identifier(name) = kind {
        return Some(name.as_str());
    }
    None
}

fn walk_block<'ast>(
    block: &'ast Block,
    labels: &mut Vec<(&'ast str, Span)>,
    goto_names: &mut Vec<&'ast str>,
    nested: &mut Vec<&'ast FunctionBody>,
) {
    for stmt in &block.stmts {
        walk_statement(stmt, labels, goto_names, nested);
    }
    if let Some(last) = &block.last_stmt {
        walk_last_statement(last, labels, goto_names, nested);
    }
}

fn walk_statement<'ast>(
    stmt: &'ast Statement,
    labels: &mut Vec<(&'ast str, Span)>,
    goto_names: &mut Vec<&'ast str>,
    nested: &mut Vec<&'ast FunctionBody>,
) {
    // Exhaustive match per CLAUDE.md invariant 3 - every Statement
    // variant gets explicit handling; no catch-alls.
    match stmt {
        Statement::Label(label) => {
            if let Some(name) = token_identifier(&label.name.kind) {
                labels.push((name, label.span));
            }
        }
        Statement::Goto(goto) => {
            if let Some(name) = token_identifier(&goto.name.kind) {
                goto_names.push(name);
            }
        }
        Statement::FunctionDecl(decl) => {
            nested.push(&decl.body);
        }
        Statement::LocalFunction(local) => {
            nested.push(&local.body);
        }
        Statement::GlobalFunction(global) => {
            nested.push(&global.body);
        }
        Statement::Assignment(assignment) => {
            for var in assignment.targets.iter() {
                walk_var(var, labels, goto_names, nested);
            }
            for expr in assignment.values.iter() {
                walk_expression(expr, labels, goto_names, nested);
            }
        }
        Statement::FunctionCall(call_stmt) => {
            walk_function_call(&call_stmt.call, labels, goto_names, nested);
        }
        Statement::DoBlock(do_block) => {
            walk_block(&do_block.block, labels, goto_names, nested);
        }
        Statement::WhileLoop(while_loop) => {
            walk_expression(&while_loop.condition, labels, goto_names, nested);
            walk_block(&while_loop.block, labels, goto_names, nested);
        }
        Statement::RepeatLoop(repeat_loop) => {
            walk_block(&repeat_loop.block, labels, goto_names, nested);
            walk_expression(&repeat_loop.condition, labels, goto_names, nested);
        }
        Statement::IfStatement(if_stmt) => {
            walk_expression(&if_stmt.condition, labels, goto_names, nested);
            walk_block(&if_stmt.block, labels, goto_names, nested);
            for clause in &if_stmt.elseif_clauses {
                walk_expression(&clause.condition, labels, goto_names, nested);
                walk_block(&clause.block, labels, goto_names, nested);
            }
            if let Some(else_clause) = &if_stmt.else_clause {
                walk_block(&else_clause.block, labels, goto_names, nested);
            }
        }
        Statement::NumericFor(num_for) => {
            walk_expression(&num_for.start, labels, goto_names, nested);
            walk_expression(&num_for.limit, labels, goto_names, nested);
            if let Some(step) = &num_for.step {
                walk_expression(step, labels, goto_names, nested);
            }
            walk_block(&num_for.block, labels, goto_names, nested);
        }
        Statement::GenericFor(generic_for) => {
            for expr in generic_for.exprs.iter() {
                walk_expression(expr, labels, goto_names, nested);
            }
            walk_block(&generic_for.block, labels, goto_names, nested);
        }
        Statement::LocalAssignment(local) => {
            if let Some(exprs) = &local.exprs {
                for expr in exprs.iter() {
                    walk_expression(expr, labels, goto_names, nested);
                }
            }
        }
        Statement::CompoundAssignment(compound) => {
            walk_var(&compound.var, labels, goto_names, nested);
            walk_expression(&compound.expr, labels, goto_names, nested);
        }
        Statement::GlobalDeclaration(global) => {
            if let Some(exprs) = &global.exprs {
                for expr in exprs.iter() {
                    walk_expression(expr, labels, goto_names, nested);
                }
            }
        }
        Statement::EmptyStatement(_)
        | Statement::GlobalStar(_)
        | Statement::Break(_)
        | Statement::TypeDeclaration(_)
        | Statement::Error(_) => {}
    }
}

fn walk_last_statement<'ast>(
    last: &'ast LastStatement,
    labels: &mut Vec<(&'ast str, Span)>,
    goto_names: &mut Vec<&'ast str>,
    nested: &mut Vec<&'ast FunctionBody>,
) {
    match last {
        LastStatement::Return(ret) => {
            for expr in ret.exprs.iter() {
                walk_expression(expr, labels, goto_names, nested);
            }
        }
        LastStatement::Break(_) | LastStatement::Continue(_) | LastStatement::Error(_) => {}
    }
}

fn walk_var<'ast>(
    var: &'ast Var,
    labels: &mut Vec<(&'ast str, Span)>,
    goto_names: &mut Vec<&'ast str>,
    nested: &mut Vec<&'ast FunctionBody>,
) {
    match var {
        Var::Name(_) => {}
        Var::Index(idx) => {
            walk_expression(&idx.prefix, labels, goto_names, nested);
            walk_expression(&idx.index, labels, goto_names, nested);
        }
        Var::FieldAccess(fld) => {
            walk_expression(&fld.prefix, labels, goto_names, nested);
        }
    }
}

fn walk_expression<'ast>(
    expr: &'ast Expression,
    labels: &mut Vec<(&'ast str, Span)>,
    goto_names: &mut Vec<&'ast str>,
    nested: &mut Vec<&'ast FunctionBody>,
) {
    // Exhaustive match per CLAUDE.md invariant 3.
    match expr {
        // Function definitions open a new scope - collected here and
        // processed separately by `check_scope` to honor the
        // "goto cannot cross function boundaries" rule.
        Expression::FunctionDef(func_def) => {
            nested.push(&func_def.body);
        }
        Expression::Var(var) => {
            walk_var(var, labels, goto_names, nested);
        }
        Expression::FunctionCall(call) => {
            walk_function_call(call, labels, goto_names, nested);
        }
        Expression::Parenthesized(paren) => {
            walk_expression(&paren.expr, labels, goto_names, nested);
        }
        Expression::TableConstructor(table) => {
            for field in table.fields.iter() {
                match field {
                    Field::Positional { value, .. } => {
                        walk_expression(value, labels, goto_names, nested);
                    }
                    Field::Named { value, .. } => {
                        walk_expression(value, labels, goto_names, nested);
                    }
                    Field::Bracketed { key, value, .. } => {
                        walk_expression(key, labels, goto_names, nested);
                        walk_expression(value, labels, goto_names, nested);
                    }
                }
            }
        }
        Expression::BinaryOp(binop) => {
            walk_expression(&binop.left, labels, goto_names, nested);
            walk_expression(&binop.right, labels, goto_names, nested);
        }
        Expression::UnaryOp(unop) => {
            walk_expression(&unop.operand, labels, goto_names, nested);
        }
        Expression::IfExpression(if_expr) => {
            walk_expression(&if_expr.condition, labels, goto_names, nested);
            walk_expression(&if_expr.then_expr, labels, goto_names, nested);
            for clause in &if_expr.elseif_clauses {
                walk_expression(&clause.condition, labels, goto_names, nested);
                walk_expression(&clause.expr, labels, goto_names, nested);
            }
            walk_expression(&if_expr.else_expr, labels, goto_names, nested);
        }
        Expression::InterpolatedString(interp) => {
            for segment in &interp.segments {
                if let Some(expr) = &segment.expr {
                    walk_expression(expr, labels, goto_names, nested);
                }
            }
        }
        Expression::TypeCast(cast) => {
            walk_expression(&cast.expr, labels, goto_names, nested);
        }
        Expression::Nil(_)
        | Expression::False(_)
        | Expression::True(_)
        | Expression::Number(_)
        | Expression::StringLiteral(_)
        | Expression::VarArg(_)
        | Expression::Error(_) => {}
    }
}

fn walk_function_call<'ast>(
    call: &'ast luck_ast::expr::FunctionCall,
    labels: &mut Vec<(&'ast str, Span)>,
    goto_names: &mut Vec<&'ast str>,
    nested: &mut Vec<&'ast FunctionBody>,
) {
    walk_expression(&call.callee, labels, goto_names, nested);
    walk_function_args(&call.args, labels, goto_names, nested);
}

fn walk_function_args<'ast>(
    args: &'ast luck_ast::expr::FunctionArgs,
    labels: &mut Vec<(&'ast str, Span)>,
    goto_names: &mut Vec<&'ast str>,
    nested: &mut Vec<&'ast FunctionBody>,
) {
    use luck_ast::expr::FunctionArgs;
    match args {
        FunctionArgs::Parenthesized { args, .. } => {
            for expr in args.iter() {
                walk_expression(expr, labels, goto_names, nested);
            }
        }
        FunctionArgs::StringLiteral(_) => {}
        FunctionArgs::TableConstructor(table) => {
            for field in table.fields.iter() {
                match field {
                    Field::Positional { value, .. } => {
                        walk_expression(value, labels, goto_names, nested);
                    }
                    Field::Named { value, .. } => {
                        walk_expression(value, labels, goto_names, nested);
                    }
                    Field::Bracketed { key, value, .. } => {
                        walk_expression(key, labels, goto_names, nested);
                        walk_expression(value, labels, goto_names, nested);
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use luck_token::LuaVersion;

    fn run(source: &str) -> Vec<LintDiagnostic> {
        crate::test_support::run_rule(&UnusedLabel, source, LuaVersion::Lua54)
    }

    #[test]
    fn ignores_label_with_matching_goto() {
        let diags = run("do ::a:: print(1) goto a end");
        assert!(diags.is_empty(), "{diags:?}");
    }

    #[test]
    fn flags_label_with_no_goto() {
        let diags = run("do ::a:: print(1) end");
        assert_eq!(diags.len(), 1, "{diags:?}");
        assert!(diags[0].message.contains("`a`"), "{diags:?}");
    }

    #[test]
    fn flags_label_in_different_function() {
        // g's goto targets g's own label (a cross-function goto would
        // not parse); f's label has no goto anywhere.
        let diags = run("function f() ::a:: print() end function g() ::a:: goto a end");
        assert_eq!(diags.len(), 1, "{diags:?}");
    }

    #[test]
    fn ignores_goto_in_inner_block() {
        let diags = run("do ::a:: do print() goto a end end");
        assert!(diags.is_empty(), "{diags:?}");
    }

    #[test]
    fn flags_unused_top_level_label() {
        let diags = run("::start:: print(1)");
        assert_eq!(diags.len(), 1, "{diags:?}");
    }

    #[test]
    fn ignores_top_level_label_used() {
        let diags = run("::start:: print(1) goto start");
        assert!(diags.is_empty(), "{diags:?}");
    }

    #[test]
    fn flags_label_unreachable_from_inner_function() {
        // The inner goto binds to the inner function's own label; the
        // outer label stays unused (labels are not visible across
        // function boundaries).
        let diags = run("::a:: local f = function() ::a:: goto a end");
        assert_eq!(
            diags.len(),
            1,
            "outer label is unreachable from inner function: {diags:?}"
        );
    }
}
