use luck_ast::Expression;
use luck_ast::expr::Var;
use luck_ast::shared::Block;
use luck_ast::stmt::{Assignment, GenericFor, LocalAssignment, NumericFor, Statement};
use luck_ast::visitor::Visitor;
use luck_semantic::SemanticAnalysis;
use luck_token::UnOp;
use luck_token::{LuaVersion, Span, TokenKind};

use crate::diagnostic::*;
use crate::rule::{LintContext, Rule};

/// Detects the two canonical shapes of "hand-rolled `table.clone`":
///
/// 1. `local out = {}; for k, v in pairs(src) do out[k] = v end`
/// 2. `local out = {}; for i = 1, #src do out[i] = src[i] end`
///
/// The recommended replacement depends on the target's stdlib:
/// `table.clone(src)` for Luau (and Lua 5.5+ where the entry now
/// exists), `table.move(src, 1, #src, 1, {})` for Lua 5.3+. Lua 5.1/5.2
/// have neither helper so the rule stays silent there.
pub struct ManualTableClone;

impl Rule for ManualTableClone {
    fn name(&self) -> &'static str {
        "manual_table_clone"
    }
    fn category(&self) -> Category {
        // Manual copy loops are slower than a single stdlib call, hence Performance.
        Category::Performance
    }
    fn default_severity(&self) -> Severity {
        Severity::Warning
    }
    fn description(&self) -> &'static str {
        "manual table copy; use table.clone or table.move"
    }

    fn check(&self, ctx: &LintContext) -> Vec<LintDiagnostic> {
        let block = ctx.block;
        let semantic = ctx.semantic;
        let source = ctx.source;
        let _comments = ctx.comments;
        let suggestion = match suggestion_for_version(semantic.version) {
            Some(label) => label,
            None => return Vec::new(),
        };
        let mut checker = CloneChecker {
            source,
            semantic,
            suggestion,
            diagnostics: Vec::new(),
        };
        checker.visit_block(block);
        checker.diagnostics
    }
}

/// What stdlib helper to suggest for the given Lua target. Returns
/// `None` on versions that have neither helper available (Lua 5.1/5.2).
fn suggestion_for_version(version: LuaVersion) -> Option<SuggestionKind> {
    match version {
        // Luau exposes `table.clone` natively. Lua 5.5 added it too, so
        // we treat them the same.
        LuaVersion::Luau | LuaVersion::Lua55 => Some(SuggestionKind::TableClone),
        // Lua 5.3 and 5.4 have `table.move` but not `table.clone`.
        LuaVersion::Lua53 | LuaVersion::Lua54 => Some(SuggestionKind::TableMove),
        // Lua 5.1 and 5.2 lack both helpers, so stay silent - the rule must
        // never suggest something that won't compile.
        LuaVersion::Lua51 | LuaVersion::Lua52 => None,
    }
}

/// Which idiomatic call to suggest in the diagnostic.
#[derive(Debug, Clone, Copy)]
enum SuggestionKind {
    TableClone,
    TableMove,
}

struct CloneChecker<'src> {
    source: &'src str,
    semantic: &'src SemanticAnalysis,
    suggestion: SuggestionKind,
    diagnostics: Vec<LintDiagnostic>,
}

impl<'src> CloneChecker<'src> {
    fn scan_block(&mut self, block: &Block) {
        // A clone shape is always a local-table-decl immediately followed by
        // a copy-loop: two adjacent statements.
        for window in block.stmts.windows(2) {
            let Statement::LocalAssignment(decl) = &window[0] else {
                continue;
            };
            let Some(dest_name) = single_empty_table_local(decl) else {
                continue;
            };
            let next = &window[1];
            let span = match next {
                Statement::GenericFor(loop_node) => {
                    if !is_generic_clone(loop_node, dest_name, self.source, self.semantic) {
                        continue;
                    }
                    // pairs() copies hash keys, but table.move copies only the
                    // array part, so suggesting table.move here would change
                    // behavior. Only table.clone targets get the hint.
                    if matches!(self.suggestion, SuggestionKind::TableMove) {
                        continue;
                    }
                    loop_node.span
                }
                Statement::NumericFor(loop_node) => {
                    if !is_numeric_clone(loop_node, dest_name, self.source) {
                        continue;
                    }
                    loop_node.span
                }
                _ => continue,
            };
            self.diagnostics.push(
                LintDiagnostic::new(
                    "manual_table_clone",
                    "manual table copy can be replaced with a stdlib call".to_string(),
                    Span::new(decl.span.start, span.end),
                )
                .with_help(self.suggestion_text(dest_name)),
            );
        }
    }

    fn suggestion_text(&self, _dest: &str) -> String {
        // The diagnostic only knows the destination's name; the source
        // table comes from the loop, which the caller has already
        // matched. Spell out the canonical replacement form per target.
        match self.suggestion {
            SuggestionKind::TableClone => "use table.clone(src) instead".to_string(),
            SuggestionKind::TableMove => "use table.move(src, 1, #src, 1, {}) instead".to_string(),
        }
    }
}

impl<'ast> Visitor<'ast> for CloneChecker<'_> {
    fn visit_block(&mut self, block: &'ast Block) {
        self.scan_block(block);
        self.walk_block(block);
    }
}

/// If the local assignment is `local NAME = {}`, return `NAME`.
fn single_empty_table_local(decl: &LocalAssignment) -> Option<&str> {
    let names: Vec<_> = decl.names.iter().collect();
    if names.len() != 1 {
        return None;
    }
    let TokenKind::Identifier(name) = &names[0].name.kind else {
        return None;
    };
    let exprs = decl.exprs.as_ref()?;
    let values: Vec<&Expression> = exprs.iter().collect();
    if values.len() != 1 {
        return None;
    }
    let Expression::TableConstructor(table) = &values[0] else {
        return None;
    };
    if !table.fields.is_empty() {
        return None;
    }
    Some(name.as_str())
}

/// Is this `for k, v in pairs(src) do out[k] = v end`?
fn is_generic_clone(
    loop_node: &GenericFor,
    dest: &str,
    _source: &str,
    semantic: &SemanticAnalysis,
) -> bool {
    let names: Vec<_> = loop_node.names.iter().collect();
    if names.len() != 2 {
        return false;
    }
    let (TokenKind::Identifier(key_name), TokenKind::Identifier(val_name)) =
        (&names[0].name.kind, &names[1].name.kind)
    else {
        return false;
    };

    let exprs: Vec<&Expression> = loop_node.exprs.iter().collect();
    if exprs.len() != 1 {
        return false;
    }
    if !is_pairs_call(exprs[0], semantic) {
        return false;
    }

    if loop_node.block.stmts.len() != 1 || loop_node.block.last_stmt.is_some() {
        return false;
    }
    let Statement::Assignment(assign) = &loop_node.block.stmts[0] else {
        return false;
    };
    is_index_copy_assignment(assign, dest, key_name.as_str(), val_name.as_str(), None)
}

/// Is this `for i = 1, #src do out[i] = src[i] end`?
fn is_numeric_clone(loop_node: &NumericFor, dest: &str, source: &str) -> bool {
    let TokenKind::Identifier(loop_var) = &loop_node.name.kind else {
        return false;
    };

    if !is_number_one(&loop_node.start, source) {
        return false;
    }
    let Some(src_name) = length_of_identifier(&loop_node.limit) else {
        return false;
    };
    // Step (if present) must also be 1; we only model the simple case.
    if let Some(step) = &loop_node.step {
        if !is_number_one(step, source) {
            return false;
        }
    }

    if loop_node.block.stmts.len() != 1 || loop_node.block.last_stmt.is_some() {
        return false;
    }
    let Statement::Assignment(assign) = &loop_node.block.stmts[0] else {
        return false;
    };
    is_index_copy_assignment(
        assign,
        dest,
        loop_var.as_str(),
        loop_var.as_str(),
        Some(src_name),
    )
}

/// Recognize `dest[key] = val` (generic-for shape) or
/// `dest[idx] = src[idx]` (numeric-for shape, with `src_for_index`
/// supplied). The key and val identifier names come from the loop's
/// declared variables.
fn is_index_copy_assignment(
    assign: &Assignment,
    dest: &str,
    key_name: &str,
    val_name: &str,
    src_for_index: Option<&str>,
) -> bool {
    let targets: Vec<&Var> = assign.targets.iter().collect();
    let values: Vec<&Expression> = assign.values.iter().collect();
    if targets.len() != 1 || values.len() != 1 {
        return false;
    }

    let Var::Index(index_expr) = targets[0] else {
        return false;
    };
    if !is_named_prefix(&index_expr.prefix, dest) {
        return false;
    }
    if !is_named_var(&index_expr.index, key_name) {
        return false;
    }

    match src_for_index {
        None => is_named_var(values[0], val_name),
        Some(src) => match values[0] {
            Expression::Var(Var::Index(rhs_index)) => {
                is_named_prefix(&rhs_index.prefix, src) && is_named_var(&rhs_index.index, key_name)
            }
            _ => false,
        },
    }
}

/// Match `pairs(x)` or `ipairs(x)`; anything else disqualifies.
fn is_pairs_call(expr: &Expression, semantic: &SemanticAnalysis) -> bool {
    let Expression::FunctionCall(call) = expr else {
        return false;
    };
    if call.method.is_some() {
        return false;
    }
    let Expression::Var(var) = &call.callee else {
        return false;
    };
    let Var::Name(token) = var else {
        return false;
    };
    let TokenKind::Identifier(name) = &token.kind else {
        return false;
    };
    // Shadowed pairs/ipairs is a user iterator, not a table walk.
    matches!(name.as_str(), "pairs" | "ipairs")
        && !semantic.resolves_to_local(name.as_str(), token.span)
}

/// Recognize `#identifier` and return that identifier.
fn length_of_identifier(expr: &Expression) -> Option<&str> {
    let Expression::UnaryOp(unop) = expr else {
        return None;
    };
    if unop.op != UnOp::Len {
        return None;
    }
    let Expression::Var(var) = &unop.operand else {
        return None;
    };
    let Var::Name(token) = var else {
        return None;
    };
    let TokenKind::Identifier(name) = &token.kind else {
        return None;
    };
    Some(name.as_str())
}

/// Whether the expression is the literal number `1`.
fn is_number_one(expr: &Expression, source: &str) -> bool {
    let Expression::Number(token) = expr else {
        return false;
    };
    let text = &source[token.span.start as usize..token.span.end as usize];
    text.parse::<f64>() == Ok(1.0)
}

/// Whether `var.prefix` is a bare reference to `name`.
fn is_named_prefix(expr: &Expression, name: &str) -> bool {
    is_named_var(expr, name)
}

/// Whether an expression is `Var::Name(name)`.
fn is_named_var(expr: &Expression, name: &str) -> bool {
    let Expression::Var(var) = expr else {
        return false;
    };
    let Var::Name(token) = var else {
        return false;
    };
    let TokenKind::Identifier(token_name) = &token.kind else {
        return false;
    };
    token_name.as_str() == name
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(source: &str, version: LuaVersion) -> Vec<LintDiagnostic> {
        crate::test_support::run_rule(&ManualTableClone, source, version)
    }

    #[test]
    fn flags_generic_for_pairs_on_luau() {
        let source = "local out = {}\nfor k, v in pairs(src) do out[k] = v end";
        let diags = run(source, LuaVersion::Luau);
        assert_eq!(diags.len(), 1, "got: {diags:?}");
        assert!(
            diags[0]
                .help
                .as_deref()
                .unwrap_or("")
                .contains("table.clone"),
            "help: {:?}",
            diags[0].help
        );
    }

    #[test]
    fn flags_numeric_for_on_lua54() {
        let source = "local out = {}\nfor i = 1, #src do out[i] = src[i] end";
        let diags = run(source, LuaVersion::Lua54);
        assert_eq!(diags.len(), 1, "got: {diags:?}");
        assert!(
            diags[0]
                .help
                .as_deref()
                .unwrap_or("")
                .contains("table.move"),
            "help: {:?}",
            diags[0].help
        );
    }

    #[test]
    fn ignores_extra_body_statement() {
        let source = "local out = {}\nfor k, v in pairs(src) do out[k] = v\nprint(k) end";
        let diags = run(source, LuaVersion::Luau);
        assert!(diags.is_empty(), "got: {diags:?}");
    }

    #[test]
    fn ignores_lua51() {
        // Lua 5.1 has neither helper, so the rule stays silent.
        let source = "local out = {}\nfor i = 1, #src do out[i] = src[i] end";
        let diags = run(source, LuaVersion::Lua51);
        assert!(diags.is_empty(), "got: {diags:?}");
    }

    #[test]
    fn ignores_local_not_empty_table() {
        let source = "local out = {1, 2}\nfor k, v in pairs(src) do out[k] = v end";
        let diags = run(source, LuaVersion::Luau);
        assert!(diags.is_empty(), "got: {diags:?}");
    }

    #[test]
    fn ignores_destination_differs() {
        // The assignment writes to a different table than the one
        // declared above the loop.
        let source = "local out = {}\nfor k, v in pairs(src) do other[k] = v end";
        let diags = run(source, LuaVersion::Luau);
        assert!(diags.is_empty(), "got: {diags:?}");
    }

    #[test]
    fn flags_ipairs_form() {
        let source = "local out = {}\nfor k, v in ipairs(src) do out[k] = v end";
        let diags = run(source, LuaVersion::Luau);
        assert_eq!(diags.len(), 1, "got: {diags:?}");
    }
}
