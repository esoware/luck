//! `-- luck:` directive comments, and what they do to the diagnostics they
//! cover.

use luck_ast::NodeKind;
use luck_token::{Comment, Span, index_to_offset};

use crate::diagnostic::{LintDiagnostic, Severity};
use crate::nodes::Nodes;

/// Verb of a `-- luck:` directive.
///
/// `Allow` drops diagnostics, `Deny` makes them errors, and `Warn` makes them
/// warnings. Selene uses the same three.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Verb {
    Allow,
    Deny,
    Warn,
}

/// What a directive covers, from its prefix and the word after its `)`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Extent {
    /// `-- #luck:`, which covers the whole file and ignores any `start` or
    /// `end`.
    File,
    /// No word: the statement the comment trails on its line, or else the
    /// next one.
    Statement,
    /// `start`: up to the matching `end`, or the end of the file.
    RegionStart,
    /// `end`: closes the latest open region with the same rule and verb.
    RegionEnd,
}

/// A `-- luck:` or `-- #luck:` comment, before resolution to what it covers.
#[derive(Debug)]
pub(crate) struct DirectiveComment<'source> {
    pub verb: Verb,
    /// `None` when the word after `)` is anything else, which disables the
    /// directive.
    pub extent: Option<Extent>,
    /// Each rule name as written, `*` included, with its span in the source.
    pub names: Vec<(&'source str, Span)>,
}

/// Parses `-- luck: <verb>(rule, ...)` or `-- #luck: <verb>(rule, ...)`.
///
/// `comment_start` is the comment's offset in the source, so name spans come
/// out absolute.
pub(crate) fn parse_directive(text: &str, comment_start: u32) -> Option<DirectiveComment<'_>> {
    // Strip the dashes and a block comment's `[[`.
    let body = text.trim_start_matches('-').trim_start_matches('[').trim_start();

    let (after_prefix, is_file_level) = if let Some(rest) = body.strip_prefix("#luck:") {
        (rest, true)
    } else {
        (body.strip_prefix("luck:")?, false)
    };

    let verb_text = after_prefix.trim_start();
    let (verb, after_verb) = [("allow", Verb::Allow), ("deny", Verb::Deny), ("warn", Verb::Warn)]
        .into_iter()
        .find_map(|(word, verb)| Some((verb, verb_text.strip_prefix(word)?)))?;

    let inside = after_verb.trim_start().strip_prefix('(')?;
    let (name_list, after_close) = inside.split_once(')')?;

    let mut names = Vec::new();
    // Every step trims from the front, so `inside` is a tail of `text` and
    // its length alone locates it in the comment.
    let mut entry_start = text.len().saturating_sub(inside.len());
    for entry in name_list.split(',') {
        let name = entry.trim();
        // Skip empty names like the one in `allow(a,)`. A list with no names
        // at all rejects the directive below.
        if !name.is_empty() {
            let leading_whitespace = entry.len().saturating_sub(entry.trim_start().len());
            let name_start = comment_start
                .saturating_add(index_to_offset(entry_start.saturating_add(leading_whitespace)));
            let name_end = name_start.saturating_add(index_to_offset(name.len()));
            names.push((name, Span::new(name_start, name_end)));
        }
        // Step over the entry and the comma `split` consumed.
        entry_start = entry_start.saturating_add(entry.len()).saturating_add(1);
    }
    if names.is_empty() {
        return None;
    }

    // A block comment leaves a trailing `]]`. Any other word, like a
    // misspelled `start`, disables the whole directive instead of quietly
    // turning a region into a single-statement directive.
    let extent = match (after_close.trim().trim_end_matches(']').trim(), is_file_level) {
        ("" | "start" | "end", true) => Some(Extent::File),
        ("", false) => Some(Extent::Statement),
        ("start", false) => Some(Extent::RegionStart),
        ("end", false) => Some(Extent::RegionEnd),
        _ => None,
    };

    Some(DirectiveComment { verb, extent, names })
}

/// A directive resolved to the byte range it covers. `rule` may be the `*`
/// wildcard.
#[derive(Debug)]
struct Directive<'source> {
    rule: &'source str,
    verb: Verb,
    range: Span,
}

impl Directive<'_> {
    fn covers(&self, diag: &LintDiagnostic) -> bool {
        (self.rule == "*" || self.rule == diag.rule)
            && diag.span.start >= self.range.start
            && diag.span.start < self.range.end
    }
}

/// Drops the diagnostics an `allow` covers and sets the severity of those a
/// `deny` or `warn` covers.
pub(crate) fn apply(
    diags: &mut Vec<LintDiagnostic>,
    comments: &[Comment],
    source: &str,
    nodes: &Nodes<'_>,
) {
    apply_directives(&resolve(comments, source, nodes), diags);
}

/// Resolves every directive in `comments`. Regions appear where they close,
/// and unclosed ones come last.
fn resolve<'source>(
    comments: &[Comment],
    source: &'source str,
    nodes: &Nodes<'_>,
) -> Vec<Directive<'source>> {
    let mut directives = Vec::new();
    // Open regions as (rule, verb, start). An `end` closes the latest one
    // with the same rule and verb, so `allow(x) start` and
    // `deny(x) start` don't collide.
    let mut open_regions: Vec<(&str, Verb, u32)> = Vec::new();
    // Built on first use, since most files have no statement directive.
    let mut sorted_spans: Option<Vec<Span>> = None;
    let source_end = index_to_offset(source.len());

    for comment in comments {
        let Some(DirectiveComment { verb, extent: Some(extent), names }) =
            parse_directive(comment.span.text(source), comment.span.start)
        else {
            continue;
        };

        match extent {
            Extent::File => {
                let range = Span::new(0, source_end);
                directives.extend(names.iter().map(|&(rule, _)| Directive { rule, verb, range }));
            }
            Extent::Statement => {
                let stmt_spans = sorted_spans.get_or_insert_with(|| sorted_stmt_spans(nodes));
                let range = stmt_range(comment.span, stmt_spans, source, source_end);
                directives.extend(names.iter().map(|&(rule, _)| Directive { rule, verb, range }));
            }
            Extent::RegionStart => {
                open_regions.extend(names.iter().map(|&(rule, _)| (rule, verb, comment.span.end)));
            }
            Extent::RegionEnd => {
                for &(rule, _) in &names {
                    if let Some(position) =
                        open_regions.iter().rposition(|&(open_rule, open_verb, _)| {
                            open_rule == rule && open_verb == verb
                        })
                    {
                        let (_, _, start) = open_regions.remove(position);
                        directives.push(Directive {
                            rule,
                            verb,
                            range: Span::new(start, comment.span.start),
                        });
                    }
                }
            }
        }
    }

    // A region with no `end` runs to the end of the file.
    directives.extend(open_regions.into_iter().map(|(rule, verb, start)| Directive {
        rule,
        verb,
        range: Span::new(start, source_end),
    }));
    directives
}

/// Every statement's span, nested ones included, sorted by start.
fn sorted_stmt_spans(nodes: &Nodes<'_>) -> Vec<Span> {
    let mut stmt_spans: Vec<Span> = nodes
        .as_slice()
        .iter()
        .filter_map(|node| match node.kind {
            NodeKind::Statement(stmt) => Some(stmt.span()),
            NodeKind::LastStatement(last_stmt) => Some(last_stmt.span()),
            NodeKind::Expression(_) => None,
        })
        .collect();
    stmt_spans.sort_unstable_by_key(|span| span.start);
    stmt_spans
}

/// The range a directive without `start` or `end` covers.
///
/// A directive trailing a statement on the same line
/// (`local x = 1 -- luck: allow(...)`) covers that statement. Otherwise it
/// covers the next one.
fn stmt_range(comment_span: Span, stmt_spans: &[Span], source: &str, source_end: u32) -> Span {
    let trailed_span = stmt_spans
        .iter()
        .filter(|stmt_span| stmt_span.end <= comment_span.start)
        .max_by_key(|stmt_span| stmt_span.end)
        .filter(|stmt_span| {
            !Span::new(stmt_span.end, comment_span.start).text(source).contains('\n')
        });
    if let Some(&stmt_span) = trailed_span {
        return stmt_span;
    }
    let next_end = stmt_spans
        .iter()
        .find(|stmt_span| stmt_span.start >= comment_span.end)
        .map_or(source_end, |stmt_span| stmt_span.end);
    Span::new(comment_span.end, next_end)
}

fn apply_directives(directives: &[Directive<'_>], diags: &mut Vec<LintDiagnostic>) {
    diags.retain_mut(|diag| {
        // The narrowest covering directive wins, so a statement-level one
        // beats a region and a region beats the whole file. Parse order
        // would let an unclosed region, appended last, override them all.
        let winner = directives
            .iter()
            .filter(|directive| directive.covers(diag))
            .min_by_key(|directive| directive.range.len());
        match winner.map(|directive| directive.verb) {
            Some(Verb::Allow) => false,
            Some(Verb::Deny) => {
                diag.severity = Severity::Error;
                true
            }
            Some(Verb::Warn) => {
                diag.severity = Severity::Warning;
                true
            }
            None => true,
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn directive(rule: &str, verb: Verb) -> Directive<'_> {
        Directive { rule, verb, range: Span::new(0, 100) }
    }

    fn diag_at(rule: &'static str, start: u32, end: u32) -> LintDiagnostic {
        LintDiagnostic::new(rule, "test", Span::new(start, end))
    }

    #[test]
    fn parse_allow_single() {
        let parsed = parse_directive("-- luck: allow(foo)", 0).expect("parse");
        assert_eq!(parsed.verb, Verb::Allow);
        assert_eq!(parsed.extent, Some(Extent::Statement));
        assert_eq!(parsed.names.len(), 1);
        assert_eq!(parsed.names[0].0, "foo");
    }

    #[test]
    fn parse_deny_region() {
        let parsed = parse_directive("-- luck: deny(foo) start", 0).expect("parse");
        assert_eq!(parsed.verb, Verb::Deny);
        assert_eq!(parsed.extent, Some(Extent::RegionStart));
    }

    #[test]
    fn parse_warn_multi() {
        let parsed = parse_directive("-- luck: warn(a, b, c)", 0).expect("parse");
        assert_eq!(parsed.verb, Verb::Warn);
        let rule_names: Vec<&str> = parsed.names.iter().map(|&(rule, _)| rule).collect();
        assert_eq!(rule_names, ["a", "b", "c"]);
    }

    #[test]
    fn parse_file_level() {
        let parsed = parse_directive("-- #luck: allow(foo)", 0).expect("parse");
        assert_eq!(parsed.extent, Some(Extent::File));
        assert_eq!(parsed.names[0].0, "foo");
        let with_start = parse_directive("-- #luck: allow(foo) start", 0).expect("parse");
        assert_eq!(with_start.extent, Some(Extent::File));
    }

    #[test]
    fn parse_block_comment_ignores_closing_brackets() {
        let parsed = parse_directive("--[[ luck: allow(foo) end ]]", 0).expect("parse");
        assert_eq!(parsed.extent, Some(Extent::RegionEnd));
    }

    #[test]
    fn name_spans_point_at_each_name() {
        // -- luck: allow(foo, bar)
        // 012345678901234567890123, shifted by the comment's start of 10
        let parsed = parse_directive("-- luck: allow(foo, bar)", 10).expect("parse");
        let spans: Vec<Span> = parsed.names.iter().map(|&(_, span)| span).collect();
        assert_eq!(spans, [Span::new(25, 28), Span::new(30, 33)]);
    }

    #[test]
    fn unknown_verb_rejected() {
        assert!(parse_directive("-- luck: forbid(foo)", 0).is_none());
    }

    #[test]
    fn plain_comment_rejected() {
        assert!(parse_directive("-- just a comment", 0).is_none());
    }

    #[test]
    fn misspelled_extent_disables_directive_but_keeps_names() {
        // `invalid_lint_filter` still checks the names of a disabled
        // directive.
        let parsed = parse_directive("-- luck: allow(foo) strat", 0).expect("parse");
        assert_eq!(parsed.extent, None);
        assert_eq!(parsed.names[0].0, "foo");
    }

    #[test]
    fn allow_drops_covered_diag() {
        let mut diags = vec![diag_at("x", 10, 20)];
        apply_directives(&[directive("x", Verb::Allow)], &mut diags);
        assert_eq!(diags, Vec::<LintDiagnostic>::new());
    }

    #[test]
    fn deny_makes_covered_diag_an_error() {
        let mut diags = vec![diag_at("x", 10, 20)];
        apply_directives(&[directive("x", Verb::Deny)], &mut diags);
        assert_eq!(diags.len(), 1);
        assert_eq!(diags[0].severity, Severity::Error);
    }

    #[test]
    fn warn_makes_covered_diag_a_warning() {
        let mut diag = diag_at("x", 10, 20);
        diag.severity = Severity::Error;
        let mut diags = vec![diag];
        apply_directives(&[directive("x", Verb::Warn)], &mut diags);
        assert_eq!(diags.len(), 1);
        assert_eq!(diags[0].severity, Severity::Warning);
    }

    #[test]
    fn wildcard_matches_any_rule() {
        let mut diags = vec![diag_at("anything", 10, 20)];
        apply_directives(&[directive("*", Verb::Allow)], &mut diags);
        assert_eq!(diags, Vec::<LintDiagnostic>::new());
    }
}
