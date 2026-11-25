use luck_ast::{AstKind, AstType, Expression, GetSpan as _, LocalAssignment};
use luck_semantic::{AstNode, AstTypesBitset};
use luck_token::Span;

use crate::context::LintContext;
use crate::diagnostic::Finding;
use crate::fix::FixKind;
use crate::rule::{Rule, declare_lint_rule};

/// Flags `nil` initializers on locals, as in `local x = nil` or the trailing
/// `nil` in `local a, b = 1, nil`, since a local starts out `nil` anyway.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct RedundantNilInit;

declare_lint_rule!(
    RedundantNilInit,
    Style,
    Warning,
    "local variable initialized to `nil` (the default value)",
    fix = Fix
);

impl Rule for RedundantNilInit {
    const NODE_TYPES: AstTypesBitset = AstTypesBitset::from_types(&[AstType::LocalAssignment]);

    fn run<'ast>(&self, node: &AstNode<'ast>, context: &LintContext<'_, 'ast>) {
        if let AstKind::LocalAssignment(local) = node.kind()
            && let Some(finding) = check_local(local, context)
        {
            context.diagnostic(finding);
        }
    }
}

fn check_local(local: &LocalAssignment<'_>, context: &LintContext<'_, '_>) -> Option<Finding> {
    // A `const` needs its initializer.
    if local.kind.is_const() {
        return None;
    }
    let exprs = &local.exprs;
    let trailing_nil_count =
        exprs.iter().rev().take_while(|expr| matches!(expr, Expression::Nil(_))).count();
    if trailing_nil_count == 0 {
        return None;
    }

    let Some(last_kept_expr) = exprs.iter().rev().nth(trailing_nil_count) else {
        let first_expr = exprs.first()?;
        return Some(
            Finding::new(
                "redundant `= nil` initializer, because uninitialized locals are `nil`",
                local.span,
            )
            .with_help("drop the `= nil` to rely on the default value")
            .with_fix(context, FixKind::Fix, |fixer| {
                fixer.delete(
                    "drop redundant `= nil` initializer",
                    whole_initializer_span(local, first_expr, context),
                )
            }),
        );
    };

    // A call or `...` that ended up last would expand to fill the names the
    // `nil`s covered, so leave those alone.
    if last_kept_expr.is_multi_value() {
        return None;
    }

    // Delete from the end of the last kept value, comma included, through
    // the end of the list, each with the parentheses it's in.
    let delete_start = context.extent(last_kept_expr).end;
    let delete_end = exprs.last().map_or(delete_start, |last_expr| context.extent(last_expr).end);
    Some(
        Finding::new(
            format!(
                "{trailing_nil_count} trailing `nil` value{} in local initializer",
                if trailing_nil_count == 1 { "" } else { "s" }
            ),
            local.span,
        )
        .with_help("drop trailing `nil` values, because the locals default to `nil`")
        .with_fix(context, FixKind::Fix, |fixer| {
            fixer.delete("drop trailing `nil` initializers", Span::new(delete_start, delete_end))
        }),
    )
}

/// The ` = nil` part of a local, starting after any comments before the `=`.
fn whole_initializer_span(
    local: &LocalAssignment<'_>,
    first_expr: &Expression<'_>,
    context: &LintContext<'_, '_>,
) -> Span {
    let binding_end = last_binding_end(local);
    let gap = Span::new(binding_end, context.extent(first_expr).start);
    let delete_start = context
        .comments_within(gap)
        .iter()
        .take_while(|comment| {
            !Span::new(binding_end, comment.span.start).text(context.source).contains(&b'=')
        })
        .last()
        .map_or(binding_end, |comment| comment.span.end);
    Span::new(delete_start, local.span.end)
}

fn last_binding_end(local: &LocalAssignment<'_>) -> u32 {
    let Some(last) = local.names.last() else {
        return local.span.start;
    };
    last.type_annotation.as_ref().map_or(last.name.span.end, |annotation| annotation.span().end)
}

#[cfg(test)]
mod tests {
    use crate::fixtures::Tester;
    use crate::rules::RuleId;

    const PASS: &[&str] = &[
        "local x = 1",
        "local x",
        "local a, b = nil, 1",
        // Dropping the nil here would change `b` from `nil` to the second return
        // value of `f()`.
        "local a, b = f(), nil",
        // Likewise after a `...`.
        "local function g(...) local a, b = ..., nil end",
        // A `const` needs its initializer.
        "const x = nil",
    ];

    const FAIL: &[&str] = &[
        "local x = nil",
        "local a, b = nil, nil",
        "local a, b = 1, nil",
        "local a, b = (x), (nil)",
        "local c = (nil)",
        // The fix should keep the annotation.
        "local x: number? = nil",
        // The fix shouldn't delete the comment.
        "local x --[[why]] = nil",
        // No fix, since it would delete the comment.
        "local x = --[[why]] nil",
        "local a, b = 1, --[[why]] nil",
        "export local x = nil",
    ];

    #[test]
    fn test() {
        Tester::new(RuleId::RedundantNilInit, PASS, FAIL).test_and_snapshot();
    }
}
