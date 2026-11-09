use luck_ast::{Expression, LocalAssignment, Statement, is_multi_value};
use luck_token::{Comment, Span};

use crate::diagnostic::Finding;
use crate::fix::{Fix, TextEdit, comments_within};
use crate::node_type::{NodeType, NodeTypeSet};
use crate::rule::{LintContext, NodeRule, impl_rule};

/// Flags `nil` initializers on locals, as in `local x = nil` or the trailing
/// `nil` in `local a, b = 1, nil`, since a local starts out `nil` anyway.
#[derive(Debug, Clone, Copy)]
pub(crate) struct RedundantNilInit;

impl_rule!(
    RedundantNilInit,
    "redundant_nil_init",
    Style,
    Warning,
    "local variable initialized to `nil` (the default value)"
);

impl NodeRule for RedundantNilInit {
    fn node_types(&self) -> NodeTypeSet {
        NodeTypeSet::from_types(&[NodeType::LocalAssignment])
    }
    fn on_stmt(&self, stmt: &Statement, context: &LintContext<'_>, findings: &mut Vec<Finding>) {
        if let Statement::LocalAssignment(local) = stmt {
            findings.extend(check_local(local, context.source, context.comments));
        }
    }
}

fn check_local(local: &LocalAssignment, source: &str, comments: &[Comment]) -> Option<Finding> {
    // Luau requires a `const` to have an initializer.
    if local.is_const {
        return None;
    }
    let exprs = local.exprs.as_ref()?;
    let trailing_nil_count =
        exprs.items.iter().rev().take_while(|expr| matches!(expr, Expression::Nil(_))).count();
    if trailing_nil_count == 0 {
        return None;
    }

    let Some(last_kept_expr) = exprs.items.iter().rev().nth(trailing_nil_count) else {
        return Some(
            Finding::new(
                "redundant `= nil` initializer, because uninitialized locals are `nil`",
                local.span,
            )
            .with_help("drop the `= nil` to rely on the default value")
            .with_optional_fix(deletion_fix(
                "drop redundant `= nil` initializer",
                whole_initializer_span(local, exprs.first()?, source, comments),
                comments,
            )),
        );
    };

    // A call or `...` that ended up last would expand to fill the names the
    // `nil`s covered, so leave those alone.
    if is_multi_value(last_kept_expr) {
        return None;
    }

    // Delete from the end of the last kept value, comma included, through
    // the end of the list.
    let delete_start = last_kept_expr.span().end;
    let delete_end = exprs.last().map_or(delete_start, |last_expr| last_expr.span().end);
    Some(
        Finding::new(
            format!(
                "{trailing_nil_count} trailing `nil` value{} in local initializer",
                if trailing_nil_count == 1 { "" } else { "s" }
            ),
            local.span,
        )
        .with_help("drop trailing `nil` values, because the locals default to `nil`")
        .with_optional_fix(deletion_fix(
            "drop trailing `nil` initializers",
            Span::new(delete_start, delete_end),
            comments,
        )),
    )
}

/// A fix that deletes `span`, unless a comment sits inside it.
fn deletion_fix(description: &str, span: Span, comments: &[Comment]) -> Option<Fix> {
    comments_within(comments, span).is_empty().then(|| Fix {
        description: description.to_owned(),
        edits: vec![TextEdit { span, replacement: String::new() }],
    })
}

/// The ` = nil` part of a local, starting after any comments before the `=`.
fn whole_initializer_span(
    local: &LocalAssignment,
    first_expr: &Expression,
    source: &str,
    comments: &[Comment],
) -> Span {
    let binding_end = last_binding_end(local);
    let gap = Span::new(binding_end, first_expr.span().start);
    let delete_start = comments_within(comments, gap)
        .iter()
        .take_while(|comment| {
            !Span::new(binding_end, comment.span.start).text(source).contains('=')
        })
        .last()
        .map_or(binding_end, |comment| comment.span.end);
    Span::new(delete_start, local.span.end)
}

/// Byte offset just past the last name and whatever annotation or attribute
/// follows it.
fn last_binding_end(local: &LocalAssignment) -> u32 {
    let Some(last) = local.names.last() else {
        return local.span.start;
    };
    let annotation_end = last.type_annotation.as_ref().map(|annotation| annotation.span().end);
    let attribute_end = last.attribute.as_ref().map(|attribute| attribute.span.end);
    annotation_end.into_iter().chain(attribute_end).fold(last.name.span.end, u32::max)
}

#[cfg(test)]
mod tests {
    use luck_token::LuaVersion;

    use super::*;
    use crate::diagnostic::LintDiagnostic;
    use crate::fixtures::{fix_and_reparse, run_rule};

    fn run(source: &str) -> Vec<LintDiagnostic> {
        run_rule(&RedundantNilInit, source, LuaVersion::Lua54)
    }

    #[test]
    fn flags_single_nil() {
        let source = "local x = nil";
        let diags = run(source);
        assert_eq!(diags.len(), 1, "expected one diag, got {diags:?}");
        let fixed = fix_and_reparse(source, &diags, LuaVersion::Lua54);
        assert_eq!(fixed, "local x");
    }

    #[test]
    fn flags_multiple_nil_values() {
        let source = "local a, b = nil, nil";
        let diags = run(source);
        assert_eq!(diags.len(), 1, "{diags:?}");
        let fixed = fix_and_reparse(source, &diags, LuaVersion::Lua54);
        assert_eq!(fixed, "local a, b");
    }

    #[test]
    fn flags_and_fixes_keeping_attribute() {
        let source = "local x <const> = nil";
        let diags = run(source);
        assert_eq!(diags.len(), 1, "{diags:?}");
        let fixed = fix_and_reparse(source, &diags, LuaVersion::Lua54);
        assert_eq!(fixed, "local x <const>");
    }

    #[test]
    fn flags_trailing_nil_only() {
        let source = "local a, b = 1, nil";
        let diags = run(source);
        assert_eq!(diags.len(), 1, "{diags:?}");
        let fixed = fix_and_reparse(source, &diags, LuaVersion::Lua54);
        assert_eq!(fixed, "local a, b = 1");
    }

    #[test]
    fn ignores_non_nil_rhs() {
        let diags = run("local x = 1");
        assert!(diags.is_empty(), "{diags:?}");
    }

    #[test]
    fn ignores_uninitialized() {
        let diags = run("local x");
        assert!(diags.is_empty(), "{diags:?}");
    }

    #[test]
    fn ignores_leading_nil_when_more_values_follow() {
        let diags = run("local a, b = nil, 1");
        assert!(diags.is_empty(), "got: {diags:?}");
    }

    #[test]
    fn ignores_trailing_nil_after_function_call() {
        // Dropping the nil here would change `b` from `nil` to the
        // second return value of `f()`.
        let diags = run("local a, b = f(), nil");
        assert!(diags.is_empty(), "must not drop nil after call; got {diags:?}");
    }

    #[test]
    fn ignores_trailing_nil_after_vararg() {
        let diags = run("local function g(...) local a, b = ..., nil end");
        assert!(diags.is_empty(), "must not drop nil after vararg; got {diags:?}");
    }

    #[test]
    fn flags_and_fixes_keeping_type_annotation() {
        let source = "local x: number? = nil";
        let diags = run_rule(&RedundantNilInit, source, LuaVersion::Luau);
        assert_eq!(diags.len(), 1, "{diags:?}");
        let fixed = fix_and_reparse(source, &diags, LuaVersion::Luau);
        assert_eq!(fixed, "local x: number?", "the fix should keep the annotation");
    }

    #[test]
    fn ignores_luau_const_nil() {
        let diags = run_rule(&RedundantNilInit, "const x = nil", LuaVersion::Luau);
        assert!(diags.is_empty(), "Luau requires an initializer on `const`: {diags:?}");
    }

    #[test]
    fn flags_and_fixes_keeping_comment() {
        let source = "local x --[[why]] = nil";
        let diags = run(source);
        assert_eq!(diags.len(), 1, "{diags:?}");
        let fixed = fix_and_reparse(source, &diags, LuaVersion::Lua54);
        assert!(fixed.contains("--[[why]]"), "the fix shouldn't delete the comment: {fixed}");
    }

    #[test]
    fn flags_without_fix_for_comment_after_the_equals() {
        let diags = run("local x = --[[why]] nil");
        assert_eq!(diags.len(), 1, "{diags:?}");
        assert!(diags[0].fix.is_none(), "the fix would delete the comment: {diags:?}");
    }

    #[test]
    fn flags_without_fix_for_comment_among_trailing_nils() {
        let diags = run("local a, b = 1, --[[why]] nil");
        assert_eq!(diags.len(), 1, "{diags:?}");
        assert!(diags[0].fix.is_none(), "{diags:?}");
    }

    #[test]
    fn flags_and_fixes_luau_export_nil() {
        let source = "export local x = nil";
        let diags = run_rule(&RedundantNilInit, source, LuaVersion::Luau);
        assert_eq!(diags.len(), 1, "{diags:?}");
        let fixed = fix_and_reparse(source, &diags, LuaVersion::Luau);
        assert_eq!(fixed, "export local x");
    }
}
