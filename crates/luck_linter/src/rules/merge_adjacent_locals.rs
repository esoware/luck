use luck_ast::{
    Attribute, ContentEq as _, GetSpan as _, LocalAssignment, LocalAssignmentKind, Statement,
};
use luck_token::Span;

use crate::diagnostic::Finding;
use crate::fix::FixKind;
use crate::nodes::adjacent_stmts;
use crate::rule::{LintContext, WholeRule, impl_rule};

#[derive(Debug, Clone, Copy)]
pub(crate) struct MergeAdjacentLocals;

impl_rule!(
    MergeAdjacentLocals,
    "merge_adjacent_locals",
    Style,
    Warning,
    "adjacent simple `local` declarations can be merged",
    Fix
);

impl WholeRule for MergeAdjacentLocals {
    fn check(&self, context: &LintContext<'_>) -> Vec<Finding> {
        let mut findings = Vec::new();
        for block in context.nodes.blocks() {
            let mut pairs = adjacent_stmts(block);
            while let Some((first_stmt, second_stmt)) = pairs.next() {
                let (
                    Statement::LocalAssignment(first_local),
                    Statement::LocalAssignment(second_local),
                ) = (first_stmt, second_stmt)
                else {
                    continue;
                };
                let Some((first, second)) = mergeable(first_local, second_local, context) else {
                    continue;
                };
                let span = Span::new(first_local.span.start, second_local.span.end);
                findings.push(
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
                // Skip the window that starts at `second_local`, so no two
                // fixes touch the same statement.
                pairs.next();
            }
        }
        findings
    }
}

/// Both bindings, unless merging them could change what the code does, or a
/// comment in the pair, such as one trailing the first, sets them apart.
fn mergeable<'ast>(
    first_local: &'ast LocalAssignment<'ast>,
    second_local: &'ast LocalAssignment<'ast>,
    context: &LintContext<'_>,
) -> Option<(SimpleBinding<'ast>, SimpleBinding<'ast>)> {
    let first = simple_binding(first_local, context)?;
    let second = simple_binding(second_local, context)?;
    let pair_span = Span::new(first_local.span.start, second_local.span.end);
    if !context.comments_within(pair_span).is_empty() {
        return None;
    }
    if !first.attribute.content_eq(&second.attribute) {
        return None;
    }
    // Lua allows only one `<close>` variable per local list.
    if first.attribute.is_some_and(|attribute| attribute.name.text == "close") {
        return None;
    }
    // Both values in `local a, b = x, y` are evaluated before either name is
    // bound, so merging `local a = 1 local b = a` would make `b` read an outer
    // or global `a`.
    let tree = context.semantic.scope_tree();
    let reads_first = tree.references_within(second.initializer).any(|id| {
        tree.reference(id)
            .resolved
            .is_some_and(|symbol| tree.symbol(symbol).definition_span == first.name)
    });
    (!reads_first).then_some((first, second))
}

struct SimpleBinding<'ast> {
    name: Span,
    declaration: Span,
    attribute: Option<&'ast Attribute<'ast>>,
    initializer: Span,
}

fn simple_binding<'ast>(
    local: &'ast LocalAssignment<'ast>,
    context: &LintContext<'_>,
) -> Option<SimpleBinding<'ast>> {
    // `const` and `export` belong to the statement this rule rebuilds.
    if local.kind != LocalAssignmentKind::Local {
        return None;
    }
    let [attributed] = &*local.names else {
        return None;
    };
    let [initializer] = &*local.exprs else {
        return None;
    };
    let declaration_end = attributed
        .type_annotation
        .as_ref()
        .map_or(attributed.name.span.end, |annotation| annotation.span().end);
    Some(SimpleBinding {
        name: attributed.name.span,
        declaration: Span::new(attributed.name.span.start, declaration_end),
        attribute: attributed.attribute.as_deref(),
        initializer: context.extent(initializer),
    })
}

/// Builds the merged statement from source slices, which keeps the
/// author's formatting.
fn build_merged(source: &str, first: &SimpleBinding<'_>, second: &SimpleBinding<'_>) -> String {
    let first_declaration = first.declaration.text(source);
    let second_declaration = second.declaration.text(source);

    let first_expr = first.initializer.text(source);
    let second_expr = second.initializer.text(source);

    // Attributes are per name: `local a <const>, b <const> = ...`.
    let attribute_suffix = first
        .attribute
        .map_or_else(String::new, |attribute| format!(" {}", attribute.span.text(source)));

    format!(
        "local {first_declaration}{attribute_suffix}, {second_declaration}{attribute_suffix} = \
         {first_expr}, {second_expr}"
    )
}

#[cfg(test)]
mod tests {
    use luck_token::LuaVersion;

    use super::*;
    use crate::diagnostic::LintDiagnostic;
    use crate::fixtures::{fix_and_reparse, run_rule};

    fn run(source: &str) -> Vec<LintDiagnostic> {
        run_rule(&MergeAdjacentLocals, source, LuaVersion::Lua54)
    }

    #[test]
    fn flags_and_fixes_two_simple_locals() {
        let source = "\
local a = 1
local b = 2";
        let diags = run(source);
        assert_eq!(diags.len(), 1, "{diags:?}");
        let fixed = fix_and_reparse(source, &diags, LuaVersion::Lua54);
        assert_eq!(fixed, "local a, b = 1, 2");
    }

    #[test]
    fn flags_and_fixes_keeping_the_parentheses_a_value_is_in() {
        let source = "\
local a = (x)
local b = (y)";
        let diags = run(source);
        assert_eq!(diags.len(), 1, "{diags:?}");
        let fixed = fix_and_reparse(source, &diags, LuaVersion::Lua54);
        assert_eq!(fixed, "local a, b = (x), (y)");
    }

    #[test]
    fn flags_and_fixes_repeating_shared_attribute() {
        let source = "\
local a <const> = 1
local b <const> = 2";
        let diags = run(source);
        assert_eq!(diags.len(), 1, "{diags:?}");
        let fixed = fix_and_reparse(source, &diags, LuaVersion::Lua54);
        assert_eq!(fixed, "local a <const>, b <const> = 1, 2");
    }

    #[test]
    fn flags_and_fixes_a_shared_attribute_spelled_with_other_spacing() {
        let source = "\
local a <const> = 1
local b < const > = 2";
        let diags = run(source);
        assert_eq!(diags.len(), 1, "{diags:?}");
        let fixed = fix_and_reparse(source, &diags, LuaVersion::Lua54);
        assert_eq!(fixed, "local a <const>, b <const> = 1, 2");
    }

    #[test]
    fn flags_alternate_pairs_only() {
        // A merged pair's second local can't start another pair, so no two
        // fixes overlap.
        let diags = run("\
local a = 1
local b = 2
local c = 3");
        assert_eq!(diags.len(), 1, "{diags:?}");
    }

    #[test]
    fn ignores_close_attribute() {
        let diags = run("\
local a <close> = nil
local b <close> = nil");
        assert!(diags.is_empty(), "{diags:?}");
    }

    #[test]
    fn ignores_second_value_reading_first_name() {
        let diags = run("\
local a = 1
local b = a + 1");
        assert!(diags.is_empty(), "{diags:?}");
    }

    #[test]
    fn ignores_second_value_reading_first_name_in_a_closure() {
        let diags = run("\
local a = 1
local b = function() return a end");
        assert!(diags.is_empty(), "{diags:?}");
    }

    #[test]
    fn flags_and_fixes_field_named_like_first_name() {
        let source = "\
local a = 1
local b = t.a";
        let diags = run(source);
        assert_eq!(diags.len(), 1, "{diags:?}");
        let fixed = fix_and_reparse(source, &diags, LuaVersion::Lua54);
        assert_eq!(fixed, "local a, b = 1, t.a");
    }

    #[test]
    fn flags_and_fixes_string_spelling_first_name() {
        let source = "\
local a = 1
local b = \"a\"";
        let diags = run(source);
        assert_eq!(diags.len(), 1, "{diags:?}");
        let fixed = fix_and_reparse(source, &diags, LuaVersion::Lua54);
        assert_eq!(fixed, "local a, b = 1, \"a\"");
    }

    #[test]
    fn flags_and_fixes_parameter_named_like_first_name() {
        let source = "\
local a = 1
local b = function(a) return a end";
        let diags = run(source);
        assert_eq!(diags.len(), 1, "{diags:?}");
        let fixed = fix_and_reparse(source, &diags, LuaVersion::Lua54);
        assert_eq!(fixed, "local a, b = 1, function(a) return a end");
    }

    #[test]
    fn ignores_when_comment_between() {
        let source = "\
local a = 1
-- keep apart
local b = 2";
        let diags = run(source);
        assert!(diags.is_empty(), "got: {diags:?}");
    }

    #[test]
    fn ignores_multi_binding_source() {
        let diags = run("\
local a, b = 1, 2
local c = 3");
        assert!(diags.is_empty(), "{diags:?}");
    }

    #[test]
    fn ignores_uninitialized() {
        let diags = run("\
local a
local b = 2");
        assert!(diags.is_empty(), "{diags:?}");
    }

    #[test]
    fn flags_locals_separated_by_semicolon() {
        let source = "local a = 1; local b = 2";
        let diags = run(source);
        assert_eq!(diags.len(), 1, "{diags:?}");
        assert_eq!(fix_and_reparse(source, &diags, LuaVersion::Lua54), "local a, b = 1, 2");
    }

    #[test]
    fn ignores_annotation_mismatch() {
        // Lua 5.4 attribute on only one of the two.
        let diags = run("\
local a <const> = 1
local b = 2");
        assert!(diags.is_empty(), "got: {diags:?}");
    }

    #[test]
    fn flags_and_fixes_keeping_type_annotations() {
        let source = "\
local a: number = 1
local b: string = \"x\"";
        let diags = run_rule(&MergeAdjacentLocals, source, LuaVersion::Luau);
        assert_eq!(diags.len(), 1, "{diags:?}");
        let fixed = fix_and_reparse(source, &diags, LuaVersion::Luau);
        assert_eq!(fixed, "local a: number, b: string = 1, \"x\"");
    }

    #[test]
    fn ignores_export() {
        let diags = run_rule(
            &MergeAdjacentLocals,
            "\
export local a = 1
export local b = 2",
            LuaVersion::Luau,
        );
        assert!(diags.is_empty(), "the merge would drop `export`: {diags:?}");
    }

    #[test]
    fn ignores_const() {
        let diags = run_rule(
            &MergeAdjacentLocals,
            "\
const a = 1
const b = 2",
            LuaVersion::Luau,
        );
        assert!(diags.is_empty(), "the merge would make the bindings mutable: {diags:?}");
    }

    #[test]
    fn ignores_comment_inside_first_local() {
        let diags = run("\
local a --[[why]] = 1
local b = 2");
        assert!(diags.is_empty(), "{diags:?}");
    }

    #[test]
    fn ignores_comment_trailing_first_local() {
        let diags = run("\
local a = 1 -- the count
local b = 2");
        assert!(diags.is_empty(), "{diags:?}");
    }
}
