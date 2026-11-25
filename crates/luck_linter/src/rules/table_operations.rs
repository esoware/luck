use luck_ast::{AstKind, AstType, Call, ChainHead, Expression, FunctionArgs, GetSpan as _, Suffix};
use luck_semantic::{AstNode, AstTypesBitset};
use luck_token::{BinOp, UnOp};
use luck_value::ValueContext;

use crate::call::dotted_callee;
use crate::context::LintContext;
use crate::diagnostic::Finding;
use crate::number::has_number_value;
use crate::rule::{Rule, declare_lint_rule};

#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct TableOperations;

declare_lint_rule!(
    TableOperations,
    Suspicious,
    Warning,
    "misuse of `table.insert`, `table.remove`, `table.move`, or `table.create`"
);

impl Rule for TableOperations {
    const NODE_TYPES: AstTypesBitset = AstTypesBitset::from_types(&[AstType::SuffixedExpression]);

    fn run<'ast>(&self, node: &AstNode<'ast>, context: &LintContext<'_, 'ast>) {
        let AstKind::SuffixedExpression(chain) = node.kind() else {
            return;
        };
        for (_, call) in chain.calls() {
            check_call(call, context);
        }
    }
}

fn check_call(call: Call<'_>, context: &LintContext<'_, '_>) {
    let Some(function_name) = table_stdlib_function(call, context) else {
        return;
    };
    let FunctionArgs::Parenthesized { args, .. } = call.args else {
        return;
    };
    let finding = |message: &'static str| Finding::new(message, call.span());
    let is_zero = |expr: &Expression<'_>| has_number_value(expr, context, 0.0);
    match (function_name, &**args) {
        ("insert", [table, index, _]) => {
            if is_zero(index) {
                context.diagnostic(
                    finding("`table.insert` with index 0, but Luau arrays are 1-based")
                        .with_help("did you mean index 1?"),
                );
            } else if length_operand(index).is_some_and(|target| same_target(target, table)) {
                context.diagnostic(
                    finding("`table.insert(t, #t, v)` inserts the value before the last element")
                        .with_help("to append, use `table.insert(t, v)`"),
                );
            } else if length_offset_operand(index, BinOp::Add, context)
                .is_some_and(|target| same_target(target, table))
            {
                context.diagnostic(
                    finding(
                        "`table.insert(t, #t + 1, v)` appends, so the index argument is redundant",
                    )
                    .with_help("use `table.insert(t, v)` instead"),
                );
            }
        }
        ("remove", [table, index]) => {
            if is_zero(index) {
                context.diagnostic(
                    finding("`table.remove` with index 0, but Luau arrays are 1-based")
                        .with_help("did you mean index 1?"),
                );
            } else if length_offset_operand(index, BinOp::Sub, context)
                .is_some_and(|target| same_target(target, table))
            {
                context.diagnostic(
                    finding("`table.remove(t, #t - 1)` removes the value before the last element")
                        .with_help("`table.remove(t)` removes the last element"),
                );
            }
        }
        ("move", move_args) => {
            if move_args.get(1).is_some_and(is_zero) {
                context.diagnostic(finding(
                    "`table.move` with start index 0, but Luau arrays are 1-based",
                ));
            }
            if move_args.get(3).is_some_and(is_zero) {
                context.diagnostic(finding(
                    "`table.move` with destination index 0, but Luau arrays are 1-based",
                ));
            }
        }
        ("create", [_, Expression::TableConstructor(_)]) => {
            context.diagnostic(
                finding("`table.create(n, {...})` fills every slot with the same table object")
                    .with_help("use a `for` loop to give each element its own table"),
            );
        }
        _ => {}
    }
}

/// The `field` of a `table.field(...)` call, if `table` is the stdlib global.
fn table_stdlib_function<'call>(
    call: Call<'call>,
    context: &LintContext<'_, '_>,
) -> Option<&'call str> {
    let (root, field) = dotted_callee(call)?;
    (root.text == "table" && context.scoping().reaches_host_global(root)).then_some(field)
}

fn length_operand<'ast>(expr: &'ast Expression<'ast>) -> Option<&'ast Expression<'ast>> {
    let Expression::UnaryOp(unary) = expr else {
        return None;
    };
    (unary.op == UnOp::Len).then_some(&unary.operand)
}

fn length_offset_operand<'ast>(
    expr: &'ast Expression<'ast>,
    op: BinOp,
    context: &dyn ValueContext,
) -> Option<&'ast Expression<'ast>> {
    let Expression::BinaryOp(run) = expr else {
        return None;
    };
    let [link] = &*run.links else {
        return None;
    };
    if link.op != op || !has_number_value(&link.operand, context, 1.0) {
        return None;
    }
    length_operand(&run.first)
}

/// The same name, or the same name with the same fields after it. Anything
/// else, such as an index or a call, never matches.
fn same_target(left: &Expression<'_>, right: &Expression<'_>) -> bool {
    let (left_head, right_head) = (ChainHead::from(left), ChainHead::from(right));
    let (Expression::Name(left_root), Expression::Name(right_root)) =
        (&left_head.prefix, &right_head.prefix)
    else {
        return false;
    };
    left_root.text == right_root.text
        && left_head.suffixes.len() == right_head.suffixes.len()
        && left_head.suffixes.iter().zip(right_head.suffixes).all(|suffixes| {
            matches!(suffixes, (Suffix::Field(left_name), Suffix::Field(right_name))
                if left_name.text == right_name.text)
        })
}

#[cfg(test)]
mod tests {
    use crate::fixtures::Tester;
    use crate::rules::RuleId;

    const PASS: &[&str] = &[
        "table.insert(t, v)",
        "table.insert(t, 1, v)",
        "table.insert(t, #u, v)",
        "table.insert(a.b, #a.c, v)",
        "\
local table = {}
table.insert(t, 0, v)",
        "\
table = {}
table.insert(t, 0, v)",
        "table.move(t, 1, #t, 1, u)",
        "local x = table.create(3, 0)",
    ];

    const FAIL: &[&str] = &[
        "table.insert(t, 0, v)",
        "table.insert(t, 0x0, v)",
        "table.remove(t, 0x0)",
        "table.move(t, 0x0, #t, 1)",
        "table.move(t, 1, #t, 0x0, u)",
        "table.insert(t, #t + 0x1, v)",
        "table.remove(t, #t - 0x1)",
        "table.insert(t, #t, v)",
        "table.insert(t, #t + 1, v)",
        "table.insert(a.b, #a.b, v)",
        "table.remove(t, 0)",
        "table.remove(t, #t - 1)",
        "table.move(t, 0, #t, 1)",
        "table.move(t, 1, #t, 0, u)",
        "local x = table.create(3, {})",
        "local last = table.remove(t, 0)",
    ];

    #[test]
    fn test() {
        Tester::new(RuleId::TableOperations, PASS, FAIL).test_and_snapshot();
    }
}
