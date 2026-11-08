use luck_ast::{
    Assignment, Block, Expression, GenericFor, LocalAssignment, NumericFor, Statement, Var, Visitor,
};
use luck_semantic::SemanticAnalysis;
use luck_token::{LuaVersion, Span, UnOp};

use crate::call::is_global_call;
use crate::diagnostic::{Category, Finding, Severity};
use crate::rule::{LintContext, Rule, WholeRule};

/// Flags the two common hand-written table copies:
///
/// 1. `local out = {}; for k, v in pairs(src) do out[k] = v end`
/// 2. `local out = {}; for i = 1, #src do out[i] = src[i] end`
///
/// The help suggests `table.clone(src)` on Luau and Lua 5.5, and
/// `table.move(src, 1, #src, 1, {})` on Lua 5.3 and 5.4. Lua 5.1 and 5.2
/// have neither, so the rule stays silent there.
#[derive(Debug, Clone, Copy)]
pub(crate) struct ManualTableClone;

impl Rule for ManualTableClone {
    fn name(&self) -> &'static str {
        "manual_table_clone"
    }
    fn category(&self) -> Category {
        // Manual copy loops are slower than a single stdlib call.
        Category::Performance
    }
    fn default_severity(&self) -> Severity {
        Severity::Warning
    }
    fn description(&self) -> &'static str {
        "manual table copy; use `table.clone` or `table.move`"
    }
}

impl WholeRule for ManualTableClone {
    fn check(&self, context: &LintContext<'_>) -> Vec<Finding> {
        let semantic = context.semantic;
        let Some(suggestion) = Suggestion::for_version(semantic.version) else {
            return Vec::new();
        };
        let mut checker = CloneChecker { semantic, suggestion, findings: Vec::new() };
        checker.visit_block(context.block);
        checker.findings
    }
}

/// The stdlib call the help suggests.
#[derive(Debug, Clone, Copy)]
enum Suggestion {
    TableClone,
    TableMove,
}

impl Suggestion {
    /// `None` on Lua 5.1 and 5.2, which have neither.
    const fn for_version(version: LuaVersion) -> Option<Self> {
        match version {
            LuaVersion::Luau | LuaVersion::Lua55 => Some(Self::TableClone),
            LuaVersion::Lua53 | LuaVersion::Lua54 => Some(Self::TableMove),
            LuaVersion::Lua51 | LuaVersion::Lua52 => None,
        }
    }

    /// `src` stands for whichever table the loop copies from.
    const fn help(self) -> &'static str {
        match self {
            Self::TableClone => "use `table.clone(src)` instead",
            Self::TableMove => "use `table.move(src, 1, #src, 1, {})` instead",
        }
    }
}

struct CloneChecker<'document> {
    semantic: &'document SemanticAnalysis,
    suggestion: Suggestion,
    findings: Vec<Finding>,
}

impl CloneChecker<'_> {
    fn check_block(&mut self, block: &Block) {
        for window in block.stmts.windows(2) {
            let [Statement::LocalAssignment(local), next] = window else {
                continue;
            };
            let Some(destination) = single_empty_table_local(local) else {
                continue;
            };
            let loop_span = match next {
                Statement::GenericFor(loop_node) => {
                    if !is_generic_clone(loop_node, destination, self.semantic) {
                        continue;
                    }
                    // `table.move` copies only the array part, so it can't
                    // stand in for a `pairs` loop.
                    if matches!(self.suggestion, Suggestion::TableMove) {
                        continue;
                    }
                    loop_node.span
                }
                Statement::NumericFor(loop_node) => {
                    if !is_numeric_clone(loop_node, destination) {
                        continue;
                    }
                    loop_node.span
                }
                Statement::Assignment(_)
                | Statement::FunctionCall(_)
                | Statement::DoBlock(_)
                | Statement::WhileLoop(_)
                | Statement::RepeatLoop(_)
                | Statement::IfStatement(_)
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
                | Statement::Error(_) => continue,
            };
            self.findings.push(
                Finding::new(
                    "manual table copy can be replaced with a stdlib call",
                    Span::new(local.span.start, loop_span.end),
                )
                .with_help(self.suggestion.help()),
            );
        }
    }
}

impl<'ast> Visitor<'ast> for CloneChecker<'_> {
    fn visit_block(&mut self, block: &'ast Block) {
        self.check_block(block);
        self.walk_block(block);
    }
}

/// If the local assignment is `local NAME = {}`, return `NAME`.
fn single_empty_table_local(local: &LocalAssignment) -> Option<&str> {
    let [attributed] = local.names.items.as_slice() else {
        return None;
    };
    let name = attributed.name.identifier()?;
    let [Expression::TableConstructor(table)] = local.exprs.as_ref()?.items.as_slice() else {
        return None;
    };
    table.fields.is_empty().then_some(name)
}

/// Is this `for k, v in pairs(src) do out[k] = v end`?
fn is_generic_clone(
    loop_node: &GenericFor,
    destination: &str,
    semantic: &SemanticAnalysis,
) -> bool {
    let [key, value] = loop_node.names.items.as_slice() else {
        return false;
    };
    let (Some(key_name), Some(value_name)) = (key.name.identifier(), value.name.identifier())
    else {
        return false;
    };

    let [iterator] = loop_node.exprs.items.as_slice() else {
        return false;
    };
    if !is_pairs_call(iterator, semantic) {
        return false;
    }

    if loop_node.block.last_stmt.is_some() {
        return false;
    }
    let [Statement::Assignment(assignment)] = loop_node.block.stmts.as_slice() else {
        return false;
    };
    is_index_copy_assignment(assignment, destination, key_name, value_name, None)
}

/// Is this `for i = 1, #src do out[i] = src[i] end`?
fn is_numeric_clone(loop_node: &NumericFor, destination: &str) -> bool {
    let Some(loop_variable) = loop_node.name.identifier() else {
        return false;
    };

    if !is_number_one(&loop_node.start) {
        return false;
    }
    let Some(source_table) = length_of_identifier(&loop_node.limit) else {
        return false;
    };
    if let Some(step) = &loop_node.step
        && !is_number_one(step)
    {
        return false;
    }

    if loop_node.block.last_stmt.is_some() {
        return false;
    }
    let [Statement::Assignment(assignment)] = loop_node.block.stmts.as_slice() else {
        return false;
    };
    is_index_copy_assignment(
        assignment,
        destination,
        loop_variable,
        loop_variable,
        Some(source_table),
    )
}

/// Matches `dest[key] = value`, or `dest[key] = src[key]` when
/// `indexed_source` names `src`.
fn is_index_copy_assignment(
    assignment: &Assignment,
    destination: &str,
    key_name: &str,
    value_name: &str,
    indexed_source: Option<&str>,
) -> bool {
    let ([Var::Index(index_expr)], [value]) =
        (assignment.targets.items.as_slice(), assignment.values.items.as_slice())
    else {
        return false;
    };
    if !is_named_var(&index_expr.prefix, destination) {
        return false;
    }
    if !is_named_var(&index_expr.index, key_name) {
        return false;
    }

    indexed_source.map_or_else(
        || is_named_var(value, value_name),
        |source_table| {
            matches!(value, Expression::Var(Var::Index(value_index))
                if is_named_var(&value_index.prefix, source_table)
                    && is_named_var(&value_index.index, key_name))
        },
    )
}

/// Matches a call to the global `pairs` or `ipairs`. A shadowed one is a
/// user iterator, not a table walk.
fn is_pairs_call(expr: &Expression, semantic: &SemanticAnalysis) -> bool {
    let Expression::FunctionCall(call) = expr else {
        return false;
    };
    is_global_call(call, "pairs", semantic) || is_global_call(call, "ipairs", semantic)
}

/// Recognize `#identifier` and return that identifier.
fn length_of_identifier(expr: &Expression) -> Option<&str> {
    let Expression::UnaryOp(unary) = expr else {
        return None;
    };
    if unary.op != UnOp::Len {
        return None;
    }
    let Expression::Var(Var::Name(token)) = &unary.operand else {
        return None;
    };
    token.identifier()
}

fn is_number_one(expr: &Expression) -> bool {
    matches!(expr, Expression::Number(literal) if literal.text.parse::<f64>() == Ok(1.0))
}

fn is_named_var(expr: &Expression, name: &str) -> bool {
    matches!(expr, Expression::Var(Var::Name(token)) if token.identifier() == Some(name))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diagnostic::LintDiagnostic;
    use crate::fixtures::run_rule;

    fn run(source: &str, version: LuaVersion) -> Vec<LintDiagnostic> {
        run_rule(&ManualTableClone, source, version)
    }

    #[test]
    fn flags_generic_for_pairs_on_luau() {
        let source = "local out = {}\nfor k, v in pairs(src) do out[k] = v end";
        let diags = run(source, LuaVersion::Luau);
        assert_eq!(diags.len(), 1, "got: {diags:?}");
        assert!(
            diags[0].help.as_deref().unwrap_or("").contains("table.clone"),
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
            diags[0].help.as_deref().unwrap_or("").contains("table.move"),
            "help: {:?}",
            diags[0].help
        );
    }

    #[test]
    fn ignores_pairs_copy_on_lua54() {
        // `table.move` copies only the array part.
        let source = "local out = {}\nfor k, v in pairs(src) do out[k] = v end";
        let diags = run(source, LuaVersion::Lua54);
        assert!(diags.is_empty(), "got: {diags:?}");
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
    fn ignores_shadowed_pairs() {
        let source = "local pairs = next\nlocal out = {}\nfor k, v in pairs(src) do out[k] = v end";
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
