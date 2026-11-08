use luck_ast::{Attribute, LocalAssignment, Statement};
use luck_token::{Comment, Span};

use crate::diagnostic::{Category, Finding, Severity};
use crate::fix::{Fix, TextEdit};
use crate::rule::{LintContext, Rule, WholeRule};

/// Flags two adjacent `local NAME = VALUE` statements that could be one
/// `local A, B = X, Y`.
#[derive(Debug, Clone, Copy)]
pub(crate) struct MergeAdjacentLocals;

impl Rule for MergeAdjacentLocals {
    fn name(&self) -> &'static str {
        "merge_adjacent_locals"
    }
    fn category(&self) -> Category {
        Category::Style
    }
    fn default_severity(&self) -> Severity {
        Severity::Warning
    }
    fn description(&self) -> &'static str {
        "adjacent simple `local` declarations can be merged"
    }
}

impl WholeRule for MergeAdjacentLocals {
    fn check(&self, context: &LintContext<'_>) -> Vec<Finding> {
        let mut findings = Vec::new();
        for block in context.blocks {
            let mut pairs = block.stmts.windows(2);
            while let Some(pair) = pairs.next() {
                let [
                    Statement::LocalAssignment(first_local),
                    Statement::LocalAssignment(second_local),
                ] = pair
                else {
                    continue;
                };
                let Some(merged) = merged_local(first_local, second_local, context) else {
                    continue;
                };
                findings.push(
                    Finding::new("adjacent `local` declarations can be merged", first_local.span)
                        .with_help("combine into a single multi-binding `local`")
                        .with_fix(Fix {
                            description: "merge adjacent `local` declarations".to_owned(),
                            edits: vec![TextEdit {
                                span: Span::new(first_local.span.start, second_local.span.end),
                                replacement: merged,
                            }],
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

/// The one `local` that declares both bindings, unless merging them could
/// change what the code does or drop a comment.
fn merged_local(
    first_local: &LocalAssignment,
    second_local: &LocalAssignment,
    context: &LintContext<'_>,
) -> Option<String> {
    let first = simple_binding(first_local)?;
    let second = simple_binding(second_local)?;
    if has_comment_between(context.comments, first_local.span.end, second_local.span.start) {
        return None;
    }
    let source = context.source;
    if !annotations_match(source, &first, &second) {
        return None;
    }
    // Lua allows only one `<close>` variable per local list.
    if first.attribute.is_some_and(|attribute| attribute.name.identifier() == Some("close")) {
        return None;
    }
    // Both values in `local a, b = x, y` are evaluated before either name is
    // bound, so merging `local a = 1 local b = a` would make `b` read an outer
    // or global `a`.
    if references_identifier(second.initializer.text(source), first.name.text(source)) {
        return None;
    }
    Some(build_merged(source, &first, &second))
}

fn has_comment_between(comments: &[Comment], start: u32, end: u32) -> bool {
    comments.iter().any(|comment| comment.span.start >= start && comment.span.end <= end)
}

/// A `local` with one name and one value: the name, its attribute if any,
/// and the value.
struct SimpleBinding<'ast> {
    name: Span,
    attribute: Option<&'ast Attribute>,
    initializer: Span,
}

fn simple_binding(local: &LocalAssignment) -> Option<SimpleBinding<'_>> {
    let [attributed] = local.names.items.as_slice() else {
        return None;
    };
    let [initializer] = local.exprs.as_ref()?.items.as_slice() else {
        return None;
    };
    Some(SimpleBinding {
        name: attributed.name.span,
        attribute: attributed.attribute.as_deref(),
        initializer: initializer.span(),
    })
}

/// Whether both bindings have the same attribute text, or neither has one.
fn annotations_match(source: &str, first: &SimpleBinding<'_>, second: &SimpleBinding<'_>) -> bool {
    match (first.attribute, second.attribute) {
        (None, None) => true,
        (Some(first_attribute), Some(second_attribute)) => {
            first_attribute.span.text(source) == second_attribute.span.text(source)
        }
        (Some(_), None) | (None, Some(_)) => false,
    }
}

/// Builds the merged statement from source slices, which keeps the
/// author's formatting.
fn build_merged(source: &str, first: &SimpleBinding<'_>, second: &SimpleBinding<'_>) -> String {
    let first_name = first.name.text(source);
    let second_name = second.name.text(source);

    let first_expr = first.initializer.text(source);
    let second_expr = second.initializer.text(source);

    // Attributes are per name: `local a <const>, b <const> = ...`.
    let attribute_suffix = first
        .attribute
        .map_or_else(String::new, |attribute| format!(" {}", attribute.span.text(source)));

    format!(
        "local {first_name}{attribute_suffix}, {second_name}{attribute_suffix} = {first_expr}, \
         {second_expr}"
    )
}

/// Whole-word search for `name` in `haystack`. A match inside a string
/// literal only costs a skipped merge.
fn references_identifier(haystack: &str, name: &str) -> bool {
    let is_word_byte = |byte: &u8| byte.is_ascii_alphanumeric() || *byte == b'_';
    haystack.match_indices(name).any(|(start, _)| {
        let (before, from_match) = haystack.split_at(start);
        let after = from_match.strip_prefix(name).unwrap_or_default();
        !before.as_bytes().last().is_some_and(is_word_byte)
            && !after.as_bytes().first().is_some_and(is_word_byte)
    })
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
        let source = "local a = 1\nlocal b = 2";
        let diags = run(source);
        assert_eq!(diags.len(), 1, "{diags:?}");
        let fixed = fix_and_reparse(source, &diags, LuaVersion::Lua54);
        assert_eq!(fixed, "local a, b = 1, 2");
    }

    #[test]
    fn flags_and_fixes_repeating_shared_attribute() {
        let source = "local a <const> = 1\nlocal b <const> = 2";
        let diags = run(source);
        assert_eq!(diags.len(), 1, "{diags:?}");
        let fixed = fix_and_reparse(source, &diags, LuaVersion::Lua54);
        assert_eq!(fixed, "local a <const>, b <const> = 1, 2");
    }

    #[test]
    fn flags_alternate_pairs_only() {
        // A merged pair's second local can't start another pair, so no two
        // fixes overlap.
        let diags = run("local a = 1\nlocal b = 2\nlocal c = 3");
        assert_eq!(diags.len(), 1, "{diags:?}");
    }

    #[test]
    fn ignores_close_attribute() {
        let diags = run("local a <close> = nil\nlocal b <close> = nil");
        assert!(diags.is_empty(), "{diags:?}");
    }

    #[test]
    fn ignores_second_value_reading_first_name() {
        let diags = run("local a = 1\nlocal b = a + 1");
        assert!(diags.is_empty(), "{diags:?}");
    }

    #[test]
    fn ignores_when_comment_between() {
        let source = "local a = 1\n-- keep apart\nlocal b = 2";
        let diags = run(source);
        assert!(diags.is_empty(), "got: {diags:?}");
    }

    #[test]
    fn ignores_multi_binding_source() {
        let diags = run("local a, b = 1, 2\nlocal c = 3");
        assert!(diags.is_empty(), "{diags:?}");
    }

    #[test]
    fn ignores_uninitialized() {
        let diags = run("local a\nlocal b = 2");
        assert!(diags.is_empty(), "{diags:?}");
    }

    #[test]
    fn ignores_annotation_mismatch() {
        // Lua 5.4 attribute on only one of the two.
        let diags = run("local a <const> = 1\nlocal b = 2");
        assert!(diags.is_empty(), "got: {diags:?}");
    }
}
