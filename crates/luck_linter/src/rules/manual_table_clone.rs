use luck_ast::{
    AstKind, AstType, Block, Expression, GenericFor, GetSpan as _, LocalAssignment, Name,
    NumericFor, Statement, Suffix, Var,
};
use luck_semantic::{AstNode, AstTypesBitset, Scoping};
use luck_stdlib::SemanticAnalysis;
use luck_token::{Span, UnOp};

use crate::call::is_global_call;
use crate::context::{ContextHost, LintContext};
use crate::diagnostic::Finding;
use crate::number::has_number_value;
use crate::rule::{Rule, declare_lint_rule};

/// The help suggests `table.clone(src)` where the target's stdlib has it, and
/// `table.move(src, 1, #src, 1, {})` otherwise. A target with neither gets no
/// finding.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct ManualTableClone;

// Manual copy loops are slower than a single stdlib call.
declare_lint_rule!(
    ManualTableClone,
    Performance,
    Warning,
    "manual table copy; use `table.clone` or `table.move`"
);

impl Rule for ManualTableClone {
    const NODE_TYPES: AstTypesBitset = AstTypesBitset::from_types(&[AstType::Block]);

    fn should_run(&self, host: &ContextHost<'_>) -> bool {
        Suggestion::for_target(host.semantic).is_some()
    }

    fn run<'ast>(&self, node: &AstNode<'ast>, context: &LintContext<'ast>) {
        let (AstKind::Block(block), Some(suggestion)) =
            (node.kind(), Suggestion::for_target(context.semantic))
        else {
            return;
        };
        for (first, second) in block.stmts.iter().zip(block.stmts.iter().skip(1)) {
            if let Some(span) = manual_copy_span(first, second, suggestion, context) {
                context.diagnostic(
                    Finding::new("manual table copy can be replaced with a stdlib call", span)
                        .with_help(suggestion.help()),
                );
            }
        }
    }
}

#[derive(Debug, Clone, Copy)]
enum Suggestion {
    TableClone,
    TableMove,
}

impl Suggestion {
    /// `None` when the target's stdlib has neither.
    fn for_target(semantic: &SemanticAnalysis<'_>) -> Option<Self> {
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

/// The span from the `local` through the loop, when `stmt` is
/// `local out = {}` and `next` a copy loop that `suggestion` can replace.
fn manual_copy_span(
    stmt: &Statement<'_>,
    next: &Statement<'_>,
    suggestion: Suggestion,
    context: &LintContext<'_>,
) -> Option<Span> {
    let semantic = context.semantic;
    let Statement::LocalAssignment(local) = stmt else {
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
        Statement::NumericFor(loop_node) => is_numeric_clone(loop_node, destination, context),
        Statement::Assignment(_)
        | Statement::FunctionCall(_)
        | Statement::DoBlock(_)
        | Statement::WhileLoop(_)
        | Statement::RepeatLoop(_)
        | Statement::IfStatement(_)
        | Statement::FunctionDecl(_)
        | Statement::LocalFunction(_)
        | Statement::LocalAssignment(_)
        | Statement::Return(_)
        | Statement::Goto(_)
        | Statement::Label(_)
        | Statement::GlobalDeclaration(_)
        | Statement::GlobalFunction(_)
        | Statement::GlobalStar(_)
        | Statement::Break(_)
        | Statement::Continue(_)
        | Statement::CompoundAssignment(_)
        | Statement::TypeDeclaration(_)
        | Statement::Error(_) => false,
    };
    is_copy.then(|| Span::new(local.span.start, next.span().end))
}

fn single_empty_table_local<'ast>(local: &'ast LocalAssignment<'ast>) -> Option<&'ast Name<'ast>> {
    let [attributed] = &*local.names else {
        return None;
    };
    let [Expression::TableConstructor(table)] = &*local.exprs else {
        return None;
    };
    table.fields.is_empty().then_some(&attributed.name)
}

/// Is this `for k, v in pairs(src) do out[k] = v end`?
fn is_generic_clone(
    loop_node: &GenericFor<'_>,
    destination: &Name<'_>,
    semantic: &SemanticAnalysis<'_>,
) -> bool {
    let [key, value] = &*loop_node.names else {
        return false;
    };
    let [iterator] = &*loop_node.exprs else {
        return false;
    };
    let tree = semantic.scoping();
    is_pairs_call(iterator, semantic)
        && copied_value(&loop_node.block, destination, &key.name, tree)
            .is_some_and(|copied| is_read_of_local(copied, &value.name, tree))
}

/// Is this `for i = 1, #src do out[i] = src[i] end`?
fn is_numeric_clone(
    loop_node: &NumericFor<'_>,
    destination: &Name<'_>,
    context: &LintContext<'_>,
) -> bool {
    let tree = context.scoping();
    let Some(source_table) = length_of_name(&loop_node.limit) else {
        return false;
    };
    let is_one = |expr: &Expression<'_>| has_number_value(expr, context, 1.0);
    let has_unit_step = loop_node.step.as_ref().is_none_or(is_one);
    is_one(&loop_node.start)
        && has_unit_step
        && copied_value(&loop_node.block, destination, &loop_node.name, tree).is_some_and(
            |copied| {
                index_read(copied).is_some_and(|(prefix, index)| {
                    is_same_binding(prefix, source_table, tree)
                        && is_read_of_local(index, &loop_node.name, tree)
                })
            },
        )
}

/// The value assigned when `body` is exactly `destination[key] = value`.
fn copied_value<'ast>(
    body: &'ast Block<'ast>,
    destination: &Name<'_>,
    key: &Name<'_>,
    tree: &Scoping,
) -> Option<&'ast Expression<'ast>> {
    let [Statement::Assignment(assignment)] = &*body.stmts else {
        return None;
    };
    let ([Var::Suffixed(target)], [value]) = (&*assignment.targets, &*assignment.values) else {
        return None;
    };
    let [Suffix::Index { index, .. }] = &*target.suffixes else {
        return None;
    };
    (is_read_of_local(&target.prefix, destination, tree) && is_read_of_local(index, key, tree))
        .then_some(value)
}

/// The table and the key of `table[key]`.
fn index_read<'ast>(
    expr: &'ast Expression<'ast>,
) -> Option<(&'ast Expression<'ast>, &'ast Expression<'ast>)> {
    let Expression::Suffixed(chain) = expr else {
        return None;
    };
    let [Suffix::Index { index, .. }] = &*chain.suffixes else {
        return None;
    };
    Some((&chain.prefix, index))
}

/// A shadowed one is a user iterator, not a table walk.
fn is_pairs_call(expr: &Expression<'_>, semantic: &SemanticAnalysis<'_>) -> bool {
    let Expression::Suffixed(chain) = expr else {
        return false;
    };
    chain.last_call().is_some_and(|call| {
        is_global_call(call, "pairs", semantic) || is_global_call(call, "ipairs", semantic)
    })
}

fn length_of_name<'ast>(expr: &'ast Expression<'ast>) -> Option<&'ast Name<'ast>> {
    let Expression::UnaryOp(unary) = expr else {
        return None;
    };
    if unary.op != UnOp::Len {
        return None;
    }
    let Expression::Name(name) = &unary.operand else {
        return None;
    };
    Some(name)
}

fn declaration_of(name: &Name<'_>, tree: &Scoping) -> Option<Span> {
    let reference = tree.reference(tree.reference_of(name)?);
    Some(tree.symbol(reference.resolved?).definition_span)
}

fn is_read_of_local(expr: &Expression<'_>, declaration: &Name<'_>, tree: &Scoping) -> bool {
    matches!(expr, Expression::Name(name)
        if declaration_of(name, tree) == Some(declaration.span))
}

/// Whether `expr` is the same name as `source`, resolving the same way, so a
/// loop variable that shadows it doesn't count.
fn is_same_binding(expr: &Expression<'_>, source: &Name<'_>, tree: &Scoping) -> bool {
    matches!(expr, Expression::Name(name)
        if name.text == source.text
            && declaration_of(name, tree) == declaration_of(source, tree))
}

#[cfg(test)]
mod tests {
    use luck_token::LuaVersion;

    use crate::diagnostic::Message;
    use crate::fixtures::run_rule;
    use crate::rules::RuleId;

    fn run(source: &str, version: LuaVersion) -> Vec<Message> {
        run_rule(RuleId::ManualTableClone, source, version)
    }

    #[test]
    fn flags_generic_for_pairs_on_luau() {
        let source = "\
local out = {}
for k, v in pairs(src) do out[k] = v end";
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
        let source = "\
local out = {}
for i = 1, #src do out[i] = src[i] end";
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
        let source = "\
local out = {}
for k, v in pairs(src) do out[k] = v end";
        let diags = run(source, LuaVersion::Lua54);
        assert!(diags.is_empty(), "got: {diags:?}");
    }

    #[test]
    fn ignores_extra_body_stmt() {
        let source = "\
local out = {}
for k, v in pairs(src) do out[k] = v
print(k) end";
        let diags = run(source, LuaVersion::Luau);
        assert!(diags.is_empty(), "got: {diags:?}");
    }

    #[test]
    fn flags_numeric_for_on_lua55_with_table_move() {
        let source = "\
local out = {}
for i = 1, #src do out[i] = src[i] end";
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
        let source = "\
local src = {1}
local out = {}
for k, v in pairs(src) do out[k] = v end";
        let diags = run(source, LuaVersion::Lua55);
        assert!(diags.is_empty(), "got: {diags:?}");
    }

    #[test]
    fn flags_hex_start_and_step() {
        let source = "\
local out = {}
for i = 0x1, #src, 0x1 do out[i] = src[i] end";
        let diags = run(source, LuaVersion::Lua54);
        assert_eq!(diags.len(), 1, "got: {diags:?}");
    }

    #[test]
    fn ignores_generic_key_shadowing_destination() {
        let source = "\
local src = {1}
local out = {}
for out, v in pairs(src) do out[out] = v end
return out";
        let diags = run(source, LuaVersion::Luau);
        assert!(diags.is_empty(), "got: {diags:?}");
    }

    #[test]
    fn ignores_generic_value_shadowing_destination() {
        let source = "\
local out = {}
for k, out in pairs(src) do out[k] = out end";
        let diags = run(source, LuaVersion::Luau);
        assert!(diags.is_empty(), "got: {diags:?}");
    }

    #[test]
    fn ignores_numeric_variable_shadowing_destination() {
        let source = "\
local out = {}
for out = 1, #src do out[out] = src[out] end";
        let diags = run(source, LuaVersion::Lua54);
        assert!(diags.is_empty(), "got: {diags:?}");
    }

    #[test]
    fn ignores_numeric_variable_shadowing_source_table() {
        let source = "\
local out = {}
for src = 1, #src do out[src] = src[src] end";
        let diags = run(source, LuaVersion::Lua54);
        assert!(diags.is_empty(), "got: {diags:?}");
    }

    #[test]
    fn ignores_lua51() {
        // Lua 5.1 has neither helper, so the rule stays silent.
        let source = "\
local out = {}
for i = 1, #src do out[i] = src[i] end";
        let diags = run(source, LuaVersion::Lua51);
        assert!(diags.is_empty(), "got: {diags:?}");
    }

    #[test]
    fn ignores_local_not_empty_table() {
        let source = "\
local out = {1, 2}
for k, v in pairs(src) do out[k] = v end";
        let diags = run(source, LuaVersion::Luau);
        assert!(diags.is_empty(), "got: {diags:?}");
    }

    #[test]
    fn ignores_destination_differs() {
        // The assignment writes to a different table than the one
        // declared above the loop.
        let source = "\
local out = {}
for k, v in pairs(src) do other[k] = v end";
        let diags = run(source, LuaVersion::Luau);
        assert!(diags.is_empty(), "got: {diags:?}");
    }

    #[test]
    fn ignores_shadowed_pairs() {
        let source = "\
local pairs = next
local out = {}
for k, v in pairs(src) do out[k] = v end";
        let diags = run(source, LuaVersion::Luau);
        assert!(diags.is_empty(), "got: {diags:?}");
    }

    #[test]
    fn flags_ipairs_form() {
        let source = "\
local out = {}
for k, v in ipairs(src) do out[k] = v end";
        let diags = run(source, LuaVersion::Luau);
        assert_eq!(diags.len(), 1, "got: {diags:?}");
    }

    #[test]
    fn flags_copy_after_semicolon() {
        let source = "\
local out = {};
for i = 1, #src do out[i] = src[i] end";
        let diags = run(source, LuaVersion::Lua54);
        assert_eq!(diags.len(), 1, "a `;` doesn't change the copy: {diags:?}");
    }
}
