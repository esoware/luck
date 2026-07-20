use luck_ast::Statement;
use luck_ast::shared::Block;
use luck_ast::stmt::LocalAssignment;
use luck_ast::visitor::Visitor;
use luck_token::{Comment, Span};

use crate::diagnostic::*;
use crate::rule::{LintContext, Rule};

/// Two adjacent single-binding `local NAME = VALUE` statements can be
/// folded into one `local A, B = X, Y` declaration.
pub struct MergeAdjacentLocals;

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

    fn check(&self, ctx: &LintContext) -> Vec<LintDiagnostic> {
        let block = ctx.block;
        let _semantic = ctx.semantic;
        let source = ctx.source;
        let comments = ctx.comments;
        let mut checker = MergeChecker {
            source,
            comments,
            diagnostics: Vec::new(),
        };
        checker.visit_block(block);
        checker.diagnostics
    }
}

struct MergeChecker<'src> {
    source: &'src str,
    comments: &'src [Comment],
    diagnostics: Vec<LintDiagnostic>,
}

impl MergeChecker<'_> {
    fn process_block(&mut self, block: &Block) {
        let mut i = 0usize;
        while i + 1 < block.stmts.len() {
            let (Statement::LocalAssignment(a), Statement::LocalAssignment(b)) =
                (&block.stmts[i], &block.stmts[i + 1])
            else {
                i += 1;
                continue;
            };

            if !is_simple_single_binding(a) || !is_simple_single_binding(b) {
                i += 1;
                continue;
            }

            if self.comment_between(a.span.end, b.span.start) {
                i += 1;
                continue;
            }

            if !annotations_match(self.source, a, b) {
                i += 1;
                continue;
            }

            // Lua permits only ONE to-be-closed variable per local list;
            // merging two `<close>` declarations is a syntax error.
            let attr_is_close = a
                .names
                .iter()
                .filter_map(|attributed| attributed.attrib.as_ref())
                .next()
                .is_some_and(|attr| source_for(self.source, attr.name.span) == "close");
            if attr_is_close {
                i += 1;
                continue;
            }

            // In `local a, b = EXPR_A, EXPR_B` both RHS evaluate before
            // either binding exists - if b's initializer mentions a's
            // name it would resolve to the OUTER a (or nil), changing
            // behavior. `local a = 1 local b = a` must not merge.
            let a_name = source_for(self.source, name_span(a));
            let b_init = source_for(self.source, init_expr_span(b));
            if references_identifier(b_init, a_name) {
                i += 1;
                continue;
            }

            let merged = build_merged(self.source, a, b);
            self.diagnostics.push(
                LintDiagnostic::new(
                    "merge_adjacent_locals",
                    "adjacent `local` declarations can be merged".to_string(),
                    a.span,
                )
                .with_help("combine into a single multi-binding `local`".to_string())
                .with_fix(Fix {
                    description: "merge adjacent `local` declarations".to_string(),
                    edits: vec![TextEdit {
                        span: Span::new(a.span.start, b.span.end),
                        replacement: merged,
                    }],
                }),
            );

            // Skip over the consumed pair to avoid double-firing on the
            // overlap with the next iteration; this also keeps the fix
            // edits non-overlapping for a single pass of apply_fixes.
            i += 2;
        }
    }

    fn comment_between(&self, start: u32, end: u32) -> bool {
        self.comments
            .iter()
            .any(|c| c.span.start >= start && c.span.end <= end)
    }
}

impl<'ast> Visitor<'ast> for MergeChecker<'_> {
    fn visit_block(&mut self, block: &'ast Block) {
        self.process_block(block);
        self.walk_block(block);
    }
}

/// A "simple single binding" local is exactly `local NAME [<attrib>] = EXPR`,
/// with one name, optionally one attribute, and exactly one initializer.
fn is_simple_single_binding(local: &LocalAssignment) -> bool {
    if local.names.len() != 1 {
        return false;
    }
    let Some(exprs) = &local.exprs else {
        return false;
    };
    exprs.len() == 1
}

/// Annotations match iff both have the same attribute presence/text.
/// We compare textual spans via the source slice to avoid having to
/// reconstruct the attribute structurally.
fn annotations_match(source: &str, a: &LocalAssignment, b: &LocalAssignment) -> bool {
    let a_attr = a
        .names
        .iter()
        .filter_map(|attributed| attributed.attrib.as_ref())
        .next();
    let b_attr = b
        .names
        .iter()
        .filter_map(|attributed| attributed.attrib.as_ref())
        .next();
    match (a_attr, b_attr) {
        (None, None) => true,
        (Some(x), Some(y)) => {
            let xs = &source[x.span.start as usize..x.span.end as usize];
            let ys = &source[y.span.start as usize..y.span.end as usize];
            xs == ys
        }
        _ => false,
    }
}

/// Build the merged replacement text. Falls back to source slicing -
/// we never reconstruct from tokens because that risks losing original
/// formatting niceties.
fn build_merged(source: &str, a: &LocalAssignment, b: &LocalAssignment) -> String {
    // If annotations differ, the caller shouldn't have invoked us; we
    // re-check defensively and bail to a no-op merge that's identical
    // to the original.
    if !annotations_match(source, a, b) {
        return format!(
            "{}\n{}",
            &source[a.span.start as usize..a.span.end as usize],
            &source[b.span.start as usize..b.span.end as usize]
        );
    }

    let a_name = source_for(source, name_span(a));
    let b_name = source_for(source, name_span(b));

    let a_expr = source_for(source, init_expr_span(a));
    let b_expr = source_for(source, init_expr_span(b));

    // Per-name attributes: `local a <const>, b <const> = ...`. A single
    // trailing attribute would silently drop a's.
    let attr_suffix = match a
        .names
        .iter()
        .filter_map(|attributed| attributed.attrib.as_ref())
        .next()
    {
        Some(at) => format!(" {}", source_for(source, at.span)),
        None => String::new(),
    };

    format!("local {a_name}{attr_suffix}, {b_name}{attr_suffix} = {a_expr}, {b_expr}")
}

/// Whole-word identifier scan. Conservative: a hit inside a string
/// literal also bails, which only means a merge is skipped.
fn references_identifier(haystack: &str, name: &str) -> bool {
    let bytes = haystack.as_bytes();
    let mut search_from = 0;
    while let Some(found) = haystack[search_from..].find(name) {
        let start = search_from + found;
        let end = start + name.len();
        let before_ok = start == 0 || {
            let b = bytes[start - 1];
            !b.is_ascii_alphanumeric() && b != b'_'
        };
        let after_ok = end == bytes.len() || {
            let b = bytes[end];
            !b.is_ascii_alphanumeric() && b != b'_'
        };
        if before_ok && after_ok {
            return true;
        }
        search_from = start + 1;
    }
    false
}

fn source_for(source: &str, span: Span) -> &str {
    &source[span.start as usize..span.end as usize]
}

fn name_span(local: &LocalAssignment) -> Span {
    // `is_simple_single_binding` guarantees `names.first` is Some.
    local
        .names
        .first()
        .map(|attributed| attributed.name.span)
        .expect("checked single-binding")
}

fn init_expr_span(local: &LocalAssignment) -> Span {
    let exprs = local.exprs.as_ref().expect("checked initializer");
    exprs.first().expect("checked single-expr").span()
}

#[cfg(test)]
mod tests {
    use super::*;
    use luck_token::LuaVersion;

    fn run(source: &str) -> Vec<LintDiagnostic> {
        crate::test_support::run_rule(&MergeAdjacentLocals, source, LuaVersion::Lua54)
    }

    fn apply(source: &str, diag: &LintDiagnostic) -> String {
        let fix = diag.fix.as_ref().expect("fix");
        let edit = &fix.edits[0];
        let mut out = String::with_capacity(source.len());
        out.push_str(&source[..edit.span.start as usize]);
        out.push_str(&edit.replacement);
        out.push_str(&source[edit.span.end as usize..]);
        let parse = luck_parser::parse(&out, LuaVersion::Lua54);
        assert!(parse.errors.is_empty(), "reparse: {:?}", parse.errors);
        out
    }

    #[test]
    fn merges_two_simple_locals() {
        let source = "local a = 1\nlocal b = 2";
        let diags = run(source);
        assert_eq!(diags.len(), 1, "{diags:?}");
        let fixed = apply(source, &diags[0]);
        assert_eq!(fixed, "local a, b = 1, 2");
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
