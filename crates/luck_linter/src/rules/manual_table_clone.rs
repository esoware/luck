use luck_ast::{Block, Expression, GenericFor, LocalAssignment, NumericFor, Statement, Var};
use luck_semantic::{ScopeTree, SemanticAnalysis};
use luck_token::{Span, Token, UnOp};

use crate::call::is_global_call;
use crate::diagnostic::Finding;
use crate::number::is_number_literal;
use crate::rule::{LintContext, WholeRule, impl_rule};

/// Flags the two common hand-written table copies:
///
/// 1. `local out = {}; for k, v in pairs(src) do out[k] = v end`
/// 2. `local out = {}; for i = 1, #src do out[i] = src[i] end`
///
/// The help suggests `table.clone(src)` where the target's stdlib has it, and
/// `table.move(src, 1, #src, 1, {})` otherwise. A target with neither gets no
/// finding.
#[derive(Debug, Clone, Copy)]
pub(crate) struct ManualTableClone;

// Manual copy loops are slower than a single stdlib call.
impl_rule!(
    ManualTableClone,
    "manual_table_clone",
    Performance,
    Warning,
    "manual table copy; use `table.clone` or `table.move`"
);

impl WholeRule for ManualTableClone {
    fn check(&self, context: &LintContext<'_>) -> Vec<Finding> {
        let semantic = context.semantic;
        let Some(suggestion) = Suggestion::for_target(semantic) else {
            return Vec::new();
        };
        context
            .blocks
            .iter()
            .flat_map(|block| block.stmts.windows(2))
            .filter_map(|pair| manual_copy_span(pair, suggestion, semantic))
            .map(|span| {
                Finding::new("manual table copy can be replaced with a stdlib call", span)
                    .with_help(suggestion.help())
            })
            .collect()
    }
}

/// The stdlib call the help suggests.
#[derive(Debug, Clone, Copy)]
enum Suggestion {
    TableClone,
    TableMove,
}

impl Suggestion {
    /// `None` when the target's stdlib has neither.
    fn for_target(semantic: &SemanticAnalysis) -> Option<Self> {
        let stdlib = semantic.stdlib();
        if stdlib.lookup(&["table", "clone"]).is_some() {
            Some(Self::TableClone)
        } else if stdlib.lookup(&["table", "move"]).is_some() {
            Some(Self::TableMove)
        } else {
            None
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

/// The span from the `local` through the loop, when `pair` is
/// `local out = {}` and then a copy loop that `suggestion` can replace.
fn manual_copy_span(
    pair: &[Statement],
    suggestion: Suggestion,
    semantic: &SemanticAnalysis,
) -> Option<Span> {
    let [Statement::LocalAssignment(local), next] = pair else {
        return None;
    };
    let destination = single_empty_table_local(local)?;
    let is_copy = match next {
        // `table.move` copies only the array part, so it can't stand in for
        // a `pairs` loop.
        Statement::GenericFor(loop_node) => {
            is_generic_clone(loop_node, destination, semantic)
                && !matches!(suggestion, Suggestion::TableMove)
        }
        Statement::NumericFor(loop_node) => {
            is_numeric_clone(loop_node, destination, &semantic.scope_tree)
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
        | Statement::Error(_) => false,
    };
    is_copy.then(|| Span::new(local.span.start, next.span().end))
}

/// If the local assignment is `local NAME = {}`, return `NAME`.
fn single_empty_table_local(local: &LocalAssignment) -> Option<&Token> {
    let [attributed] = local.names.items.as_slice() else {
        return None;
    };
    let [Expression::TableConstructor(table)] = local.exprs.as_ref()?.items.as_slice() else {
        return None;
    };
    table.fields.is_empty().then_some(&attributed.name)
}

/// Is this `for k, v in pairs(src) do out[k] = v end`?
fn is_generic_clone(
    loop_node: &GenericFor,
    destination: &Token,
    semantic: &SemanticAnalysis,
) -> bool {
    let [key, value] = loop_node.names.items.as_slice() else {
        return false;
    };
    let [iterator] = loop_node.exprs.items.as_slice() else {
        return false;
    };
    let tree = &semantic.scope_tree;
    is_pairs_call(iterator, semantic)
        && copied_value(&loop_node.block, destination, &key.name, tree)
            .is_some_and(|copied| is_read_of_local(copied, &value.name, tree))
}

/// Is this `for i = 1, #src do out[i] = src[i] end`?
fn is_numeric_clone(loop_node: &NumericFor, destination: &Token, tree: &ScopeTree) -> bool {
    let Some(source_table) = length_of_name(&loop_node.limit) else {
        return false;
    };
    let has_unit_step = loop_node.step.as_ref().is_none_or(|step| is_number_literal(step, 1.0));
    is_number_literal(&loop_node.start, 1.0)
        && has_unit_step
        && copied_value(&loop_node.block, destination, &loop_node.name, tree).is_some_and(
            |copied| {
                matches!(copied, Expression::Var(Var::Index(copied_index))
                    if is_same_binding(&copied_index.prefix, source_table, tree)
                        && is_read_of_local(&copied_index.index, &loop_node.name, tree))
            },
        )
}

/// The value assigned when `body` is exactly `destination[key] = value`.
fn copied_value<'ast>(
    body: &'ast Block,
    destination: &Token,
    key: &Token,
    tree: &ScopeTree,
) -> Option<&'ast Expression> {
    if body.last_stmt.is_some() {
        return None;
    }
    let [Statement::Assignment(assignment)] = body.stmts.as_slice() else {
        return None;
    };
    let ([Var::Index(target)], [value]) =
        (assignment.targets.items.as_slice(), assignment.values.items.as_slice())
    else {
        return None;
    };
    (is_read_of_local(&target.prefix, destination, tree)
        && is_read_of_local(&target.index, key, tree))
    .then_some(value)
}

/// Matches a call to the global `pairs` or `ipairs`. A shadowed one is a
/// user iterator, not a table walk.
fn is_pairs_call(expr: &Expression, semantic: &SemanticAnalysis) -> bool {
    let Expression::FunctionCall(call) = expr else {
        return false;
    };
    is_global_call(call, "pairs", semantic) || is_global_call(call, "ipairs", semantic)
}

/// Recognize `#name` and return that name.
fn length_of_name(expr: &Expression) -> Option<&Token> {
    let Expression::UnaryOp(unary) = expr else {
        return None;
    };
    if unary.op != UnOp::Len {
        return None;
    }
    let Expression::Var(Var::Name(token)) = &unary.operand else {
        return None;
    };
    Some(token)
}

/// The span of the declaration that `name` resolves to, or `None` for a
/// global.
fn declaration_of(name: &Token, tree: &ScopeTree) -> Option<Span> {
    let reference = tree.reference(*tree.references_by_span.get(&name.span)?);
    Some(tree.symbol(reference.resolved?).definition_span)
}

/// Whether `expr` is a name that resolves to the local `declaration` declares.
fn is_read_of_local(expr: &Expression, declaration: &Token, tree: &ScopeTree) -> bool {
    matches!(expr, Expression::Var(Var::Name(token))
        if declaration_of(token, tree) == Some(declaration.span))
}

/// Whether `expr` is the same name as `source`, resolving the same way, so a
/// loop variable that shadows it doesn't count.
fn is_same_binding(expr: &Expression, source: &Token, tree: &ScopeTree) -> bool {
    matches!(expr, Expression::Var(Var::Name(token))
        if token.identifier() == source.identifier()
            && declaration_of(token, tree) == declaration_of(source, tree))
}

#[cfg(test)]
mod tests {
    use luck_token::LuaVersion;

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
    fn ignores_extra_body_stmt() {
        let source = "local out = {}\nfor k, v in pairs(src) do out[k] = v\nprint(k) end";
        let diags = run(source, LuaVersion::Luau);
        assert!(diags.is_empty(), "got: {diags:?}");
    }

    #[test]
    fn flags_numeric_for_on_lua55_with_table_move() {
        let source = "local out = {}\nfor i = 1, #src do out[i] = src[i] end";
        let diags = run(source, LuaVersion::Lua55);
        assert_eq!(diags.len(), 1, "got: {diags:?}");
        assert!(
            diags[0].help.as_deref().unwrap_or("").contains("table.move"),
            "help: {:?}",
            diags[0].help
        );
    }

    #[test]
    fn ignores_pairs_copy_on_lua55() {
        // The 5.5 stdlib has no `table.clone`, and `table.move` can't copy a
        // `pairs` loop.
        let source = "local src = {1}\nlocal out = {}\nfor k, v in pairs(src) do out[k] = v end";
        let diags = run(source, LuaVersion::Lua55);
        assert!(diags.is_empty(), "got: {diags:?}");
    }

    #[test]
    fn flags_hex_start_and_step() {
        let source = "local out = {}\nfor i = 0x1, #src, 0x1 do out[i] = src[i] end";
        let diags = run(source, LuaVersion::Lua54);
        assert_eq!(diags.len(), 1, "got: {diags:?}");
    }

    #[test]
    fn ignores_generic_key_shadowing_destination() {
        let source = "local src = {1}\nlocal out = {}\nfor out, v in pairs(src) do out[out] = v end\nreturn out";
        let diags = run(source, LuaVersion::Luau);
        assert!(diags.is_empty(), "got: {diags:?}");
    }

    #[test]
    fn ignores_generic_value_shadowing_destination() {
        let source = "local out = {}\nfor k, out in pairs(src) do out[k] = out end";
        let diags = run(source, LuaVersion::Luau);
        assert!(diags.is_empty(), "got: {diags:?}");
    }

    #[test]
    fn ignores_numeric_variable_shadowing_destination() {
        let source = "local out = {}\nfor out = 1, #src do out[out] = src[out] end";
        let diags = run(source, LuaVersion::Lua54);
        assert!(diags.is_empty(), "got: {diags:?}");
    }

    #[test]
    fn ignores_numeric_variable_shadowing_source_table() {
        let source = "local out = {}\nfor src = 1, #src do out[src] = src[src] end";
        let diags = run(source, LuaVersion::Lua54);
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

    #[test]
    fn bug_flags_copy_after_semicolon() {
        let source = "local out = {};\nfor i = 1, #src do out[i] = src[i] end";
        let diags = run(source, LuaVersion::Lua54);
        assert_eq!(diags.len(), 1, "a `;` doesn't change the copy: {diags:?}");
    }
}
