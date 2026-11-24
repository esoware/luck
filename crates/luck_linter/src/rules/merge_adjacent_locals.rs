use luck_ast::{
    AstKind, AstType, Attribute, ContentEq as _, GetSpan as _, LocalAssignment,
    LocalAssignmentKind, Statement,
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

    fn run<'ast>(&self, node: &AstNode<'ast>, context: &LintContext<'ast>) {
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
    let tree = context.semantic.scoping();
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
        attribute: attributed.attribute_or(local.attribute.as_deref()),
        initializer: context.extent(initializer),
    })
}

/// Builds the merged statement from source slices, which keeps the
/// author's formatting.
fn build_merged(source: &[u8], first: &SimpleBinding<'_>, second: &SimpleBinding<'_>) -> Vec<u8> {
    // Attributes are per name: `local a <const>, b <const> = ...`.
    let (space, attribute) = first
        .attribute
        .map_or((&b""[..], &b""[..]), |attribute| (b" ", attribute.span.text(source)));
    [
        &b"local "[..],
        first.declaration.text(source),
        space,
        attribute,
        b", ",
        second.declaration.text(source),
        space,
        attribute,
        b" = ",
        first.initializer.text(source),
        b", ",
        second.initializer.text(source),
    ]
    .concat()
}

#[cfg(test)]
mod tests {
    use luck_token::LuaVersion;

    use crate::diagnostic::Message;
    use crate::fixtures::{fix_and_reparse, run_rule};
    use crate::rules::RuleId;

    fn run(source: &str) -> Vec<Message> {
        run_rule(RuleId::MergeAdjacentLocals, source, LuaVersion::Lua54)
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
        let diags = run_rule(RuleId::MergeAdjacentLocals, source, LuaVersion::Luau);
        assert_eq!(diags.len(), 1, "{diags:?}");
        let fixed = fix_and_reparse(source, &diags, LuaVersion::Luau);
        assert_eq!(fixed, "local a: number, b: string = 1, \"x\"");
    }

    #[test]
    fn ignores_export() {
        let diags = run_rule(
            RuleId::MergeAdjacentLocals,
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
            RuleId::MergeAdjacentLocals,
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
