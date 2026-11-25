use luck_ast::{
    AstKind, AstType, GetSpan as _, LocalAssignment, LocalAssignmentKind, Name, Statement,
};
use luck_semantic::{AstNode, AstTypesBitset};
use luck_token::Span;

use crate::context::LintContext;
use crate::diagnostic::Finding;
use crate::fix::FixKind;
use crate::rule::{Rule, declare_lint_rule};

#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct MergeAdjacentLocals;

declare_lint_rule!(
    MergeAdjacentLocals,
    Style,
    Warning,
    "adjacent simple `local` declarations can be merged",
    fix = Fix
);

impl Rule for MergeAdjacentLocals {
    const NODE_TYPES: AstTypesBitset = AstTypesBitset::from_types(&[AstType::Block]);

    fn run<'ast>(&self, node: &AstNode<'ast>, context: &LintContext<'_, 'ast>) {
        let AstKind::Block(block) = node.kind() else {
            return;
        };
        let mut pairs = block.stmts.iter().zip(block.stmts.iter().skip(1));
        while let Some(pair) = pairs.next() {
            let (Statement::LocalAssignment(first_local), Statement::LocalAssignment(second_local)) =
                pair
            else {
                continue;
            };
            let Some((first, second)) = mergeable(first_local, second_local, context) else {
                continue;
            };
            let span = Span::new(first_local.span.start, second_local.span.end);
            context.diagnostic(
                Finding::new("adjacent `local` declarations can be merged", first_local.span)
                    .with_help("combine into a single multi-binding `local`")
                    .with_fix(context, FixKind::Fix, |fixer| {
                        fixer.replace(
                            "merge adjacent `local` declarations",
                            span,
                            build_merged(context.source, &first, &second),
                        )
                    }),
            );
            // Skip the window that starts at `second_local`, so no two fixes
            // touch the same statement.
            pairs.next();
        }
    }
}

/// Both bindings, unless merging them could change what the code does, or a
/// comment in the pair, such as one trailing the first, sets them apart.
fn mergeable<'ast>(
    first_local: &'ast LocalAssignment<'ast>,
    second_local: &'ast LocalAssignment<'ast>,
    context: &LintContext<'_, '_>,
) -> Option<(SimpleBinding<'ast>, SimpleBinding<'ast>)> {
    let first = simple_binding(first_local, context)?;
    let second = simple_binding(second_local, context)?;
    let pair_span = Span::new(first_local.span.start, second_local.span.end);
    if !context.comments_within(pair_span).is_empty() {
        return None;
    }
    // Both values in `local a, b = x, y` are evaluated before either name is
    // bound, so merging `local a = 1 local b = a` would make `b` read an outer
    // or global `a`.
    let tree = context.scoping();
    let reads_first = tree.symbol_of(first.name).is_some_and(|symbol| {
        tree.resolved_references(symbol).iter().any(|&id| {
            let span = tree.reference(id).span;
            second.initializer.start <= span.start && span.end <= second.initializer.end
        })
    });
    (!reads_first).then_some((first, second))
}

struct SimpleBinding<'ast> {
    name: &'ast Name<'ast>,
    declaration: Span,
    initializer: Span,
}

fn simple_binding<'ast>(
    local: &'ast LocalAssignment<'ast>,
    context: &LintContext<'_, '_>,
) -> Option<SimpleBinding<'ast>> {
    // `const` and `export` belong to the statement this rule rebuilds.
    if local.kind != LocalAssignmentKind::Local {
        return None;
    }
    let [local_name] = &*local.names else {
        return None;
    };
    let [initializer] = &*local.exprs else {
        return None;
    };
    let declaration_end = local_name
        .type_annotation
        .as_ref()
        .map_or(local_name.name.span.end, |annotation| annotation.span().end);
    Some(SimpleBinding {
        name: &local_name.name,
        declaration: Span::new(local_name.name.span.start, declaration_end),
        initializer: context.extent(initializer),
    })
}

/// Builds the merged statement from source slices, which keeps the
/// author's formatting.
fn build_merged(source: &[u8], first: &SimpleBinding<'_>, second: &SimpleBinding<'_>) -> Vec<u8> {
    [
        &b"local "[..],
        first.declaration.text(source),
        b", ",
        second.declaration.text(source),
        b" = ",
        first.initializer.text(source),
        b", ",
        second.initializer.text(source),
    ]
    .concat()
}

#[cfg(test)]
mod tests {
    use crate::fixtures::Tester;
    use crate::rules::RuleId;

    const PASS: &[&str] = &[
        "\
local a = 1
local b = a + 1",
        "\
local a = 1
local b = function() return a end",
        "\
local a = 1
-- keep apart
local b = 2",
        "\
local a, b = 1, 2
local c = 3",
        "\
local a
local b = 2",
        // The merge would drop `export`.
        "\
export local a = 1
export local b = 2",
        // The merge would make the bindings mutable.
        "\
const a = 1
const b = 2",
        "\
local a = 1
const b = 2",
        "\
local a --[[why]] = 1
local b = 2",
        "\
local a = 1 -- the count
local b = 2",
    ];

    const FAIL: &[&str] = &[
        "\
local a = 1
local b = 2",
        "\
local a = (x)
local b = (y)",
        // A merged pair's second local can't start another pair, so no two fixes
        // overlap.
        "\
local a = 1
local b = 2
local c = 3",
        "\
local a = 1
local b = t.a",
        "\
local a = 1
local b = \"a\"",
        "\
local a = 1
local b = function(a) return a end",
        "local a = 1; local b = 2",
        "\
local a: number = 1
local b: string = \"x\"",
    ];

    #[test]
    fn test() {
        Tester::new(RuleId::MergeAdjacentLocals, PASS, FAIL).test_and_snapshot();
    }
}
