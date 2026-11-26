use luck_ast::{
    AstKind, AstType, Block, Expression, GenericFor, GetSpan as _, LocalAssignment, Name,
    NumericFor, Statement, Suffix, Var,
};
use luck_semantic::{AstNode, AstTypesBitset, Scoping, SymbolId};
use luck_token::{Span, UnOp};

use crate::call::is_global_call;
use crate::context::LintContext;
use crate::diagnostic::Finding;
use crate::number::has_number_value;
use crate::rule::{Rule, declare_lint_rule};

#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct ManualTableClone;

// Manual copy loops are slower than a single stdlib call.
declare_lint_rule!(ManualTableClone, Performance, Warning, "manual table copy; use `table.clone`");

impl Rule for ManualTableClone {
    const NODE_TYPES: AstTypesBitset = AstTypesBitset::from_types(&[AstType::Block]);

    fn run<'ast>(&self, node: &AstNode<'ast>, context: &LintContext<'_, 'ast>) {
        let AstKind::Block(block) = node.kind() else {
            return;
        };
        for (first, second) in block.stmts.iter().zip(block.stmts.iter().skip(1)) {
            if let Some(span) = manual_copy(first, second, context) {
                context.diagnostic(
                    Finding::new("manual table copy can be replaced with a stdlib call", span)
                        .with_help("use `table.clone(src)` instead"),
                );
            }
        }
    }
}

/// The span from the `local` through the loop, when `stmt` is
/// `local out = {}` and `next` a loop that copies a table into it.
fn manual_copy(
    stmt: &Statement<'_>,
    next: &Statement<'_>,
    context: &LintContext<'_, '_>,
) -> Option<Span> {
    let Statement::LocalAssignment(local) = stmt else {
        return None;
    };
    let destination = empty_table_local(local, context.scoping())?;
    let is_copy = match next {
        Statement::GenericFor(generic_for) => is_generic_copy(generic_for, destination, context),
        Statement::NumericFor(numeric_for) => is_numeric_copy(numeric_for, destination, context),
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
        | Statement::Break(_)
        | Statement::Continue(_)
        | Statement::CompoundAssignment(_)
        | Statement::TypeDeclaration(_)
        | Statement::Declaration(_)
        | Statement::Error(_) => false,
    };
    is_copy.then(|| Span::new(local.span.start, next.span().end))
}

/// The local `local` declares, when it's exactly `local out = {}`.
fn empty_table_local(local: &LocalAssignment<'_>, tree: &Scoping) -> Option<SymbolId> {
    let ([local_name], [Expression::TableConstructor(table)]) = (&*local.names, &*local.exprs)
    else {
        return None;
    };
    if !table.fields.is_empty() {
        return None;
    }
    tree.symbol_of(&local_name.name)
}

/// Whether `generic_for` is `for k, v in pairs(src) do out[k] = v end`.
fn is_generic_copy(
    generic_for: &GenericFor<'_>,
    destination: SymbolId,
    context: &LintContext<'_, '_>,
) -> bool {
    let ([key, value], [iterator]) = (&*generic_for.names, &*generic_for.exprs) else {
        return false;
    };
    let tree = context.scoping();
    let (Some(key), Some(value)) = (tree.symbol_of(&key.name), tree.symbol_of(&value.name)) else {
        return false;
    };
    is_pairs_call(iterator, tree)
        && copied_value(&generic_for.block, destination, key, tree)
            .is_some_and(|copied| is_name_of(copied, value, tree))
}

/// Whether `numeric_for` is `for i = 1, #src do out[i] = src[i] end`.
fn is_numeric_copy(
    numeric_for: &NumericFor<'_>,
    destination: SymbolId,
    context: &LintContext<'_, '_>,
) -> bool {
    let tree = context.scoping();
    let (Some(source_table), Some(index)) =
        (length_of_name(&numeric_for.limit), tree.symbol_of(&numeric_for.variable.name))
    else {
        return false;
    };
    let is_one = |expr: &Expression<'_>| has_number_value(expr, context, 1.0);
    is_one(&numeric_for.start)
        && numeric_for.step.as_ref().is_none_or(is_one)
        && copied_value(&numeric_for.block, destination, index, tree).is_some_and(|copied| {
            index_read(copied).is_some_and(|(table, key)| {
                is_same_binding(table, source_table, tree) && is_name_of(key, index, tree)
            })
        })
}

/// The value assigned when `body` is exactly `destination[key] = value`.
fn copied_value<'ast>(
    body: &'ast Block<'ast>,
    destination: SymbolId,
    key: SymbolId,
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
    (is_name_of(&target.prefix, destination, tree) && is_name_of(index, key, tree)).then_some(value)
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
fn is_pairs_call(expr: &Expression<'_>, tree: &Scoping) -> bool {
    let Expression::Suffixed(chain) = expr else {
        return false;
    };
    chain.last_call().is_some_and(|call| {
        is_global_call(call, "pairs", tree) || is_global_call(call, "ipairs", tree)
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

fn is_name_of(expr: &Expression<'_>, symbol: SymbolId, tree: &Scoping) -> bool {
    matches!(expr, Expression::Name(name) if tree.resolved_symbol_of(name) == Some(symbol))
}

/// Whether `expr` is the same name as `source`, resolving the same way, so a
/// loop variable that shadows it doesn't count.
fn is_same_binding(expr: &Expression<'_>, source: &Name<'_>, tree: &Scoping) -> bool {
    matches!(expr, Expression::Name(name)
        if name.text == source.text
            && tree.resolved_symbol_of(name) == tree.resolved_symbol_of(source))
}

#[cfg(test)]
mod tests {
    use crate::fixtures::Tester;
    use crate::rules::RuleId;

    const PASS: &[&str] = &[
        "\
local out = {}
for k, v in pairs(src) do out[k] = v
print(k) end",
        "\
local src = {1}
local out = {}
for out, v in pairs(src) do out[out] = v end
return out",
        "\
local out = {}
for k, out in pairs(src) do out[k] = out end",
        "\
local out = {}
for out = 1, #src do out[out] = src[out] end",
        "\
local out = {}
for src = 1, #src do out[src] = src[src] end",
        "\
local out = {1, 2}
for k, v in pairs(src) do out[k] = v end",
        // The assignment writes to a different table than the one declared above
        // the loop.
        "\
local out = {}
for k, v in pairs(src) do other[k] = v end",
        "\
local pairs = next
local out = {}
for k, v in pairs(src) do out[k] = v end",
    ];

    const FAIL: &[&str] = &[
        "\
local out = {}
for k, v in pairs(src) do out[k] = v end",
        "\
local out = {}
for i = 1, #src do out[i] = src[i] end",
        "\
local out = {}
for i = 0x1, #src, 0x1 do out[i] = src[i] end",
        "\
local out = {}
for k, v in ipairs(src) do out[k] = v end",
        // A `;` doesn't change the copy.
        "\
local out = {};
for i = 1, #src do out[i] = src[i] end",
    ];

    #[test]
    fn test() {
        Tester::new(RuleId::ManualTableClone, PASS, FAIL).test_and_snapshot();
    }
}
