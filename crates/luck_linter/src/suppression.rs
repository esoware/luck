use luck_token::{Comment, LuaVersion, Span, has_line_break, index_to_offset};

use crate::diagnostic::{LintDiagnostic, Severity};
use crate::nodes::{NodeKind, Nodes};

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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DirectiveRule<'source> {
    /// `*`, which matches every rule.
    Wildcard,
    /// A rule name as written, which may not name a registered rule.
    Named(&'source str),
}

impl DirectiveRule<'_> {
    fn matches(self, rule: &str) -> bool {
        match self {
            Self::Wildcard => true,
            Self::Named(name) => name == rule,
        }
    }
}

#[derive(Debug)]
pub(crate) struct DirectiveComment<'source> {
    pub verb: Verb,
    /// `None` when the word after `)` is anything else, which disables the
    /// directive.
    pub extent: Option<Extent>,
    pub rules: Vec<(DirectiveRule<'source>, Span)>,
}

/// `comment_start` is the comment's offset in the source, so name spans come
/// out absolute.
pub(crate) fn parse_directive(text: &str, comment_start: u32) -> Option<DirectiveComment<'_>> {
    // Strip the dashes and a block comment's `[[` or `[=[`.
    let body = text.trim_start_matches('-').trim_start_matches(['[', '=']).trim_start();

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

    let mut rules = Vec::new();
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
            let rule =
                if name == "*" { DirectiveRule::Wildcard } else { DirectiveRule::Named(name) };
            rules.push((rule, Span::new(name_start, name_end)));
        }
        // Step over the entry and the comma `split` consumed.
        entry_start = entry_start.saturating_add(entry.len()).saturating_add(1);
    }
    if rules.is_empty() {
        return None;
    }

    // A block comment leaves a trailing `]]` or `]=]`. Any other word, like a
    // misspelled `start`, disables the whole directive instead of quietly
    // turning a region into a single-statement directive.
    let extent = match (after_close.trim().trim_end_matches([']', '=']).trim(), is_file_level) {
        ("" | "start" | "end", true) => Some(Extent::File),
        ("", false) => Some(Extent::Statement),
        ("start", false) => Some(Extent::RegionStart),
        ("end", false) => Some(Extent::RegionEnd),
        _ => None,
    };

    Some(DirectiveComment { verb, extent, rules })
}

#[derive(Debug)]
struct Directive<'source> {
    rule: DirectiveRule<'source>,
    verb: Verb,
    range: Span,
}

impl Directive<'_> {
    fn covers(&self, diag: &LintDiagnostic) -> bool {
        self.rule.matches(diag.rule)
            && diag.span.start >= self.range.start
            && diag.span.start < self.range.end
    }
}

pub(crate) fn apply(
    diags: &mut Vec<LintDiagnostic>,
    comments: &[Comment],
    source: &str,
    nodes: &Nodes<'_>,
    version: LuaVersion,
) {
    apply_directives(&resolve(comments, source, nodes, version), diags);
}

/// Regions appear where they close, and unclosed ones come last.
fn resolve<'source>(
    comments: &[Comment],
    source: &'source str,
    nodes: &Nodes<'_>,
    version: LuaVersion,
) -> Vec<Directive<'source>> {
    let mut directives = Vec::new();
    // Open regions as (rule, verb, start). An `end` closes the latest one
    // with the same rule and verb, so `allow(x) start` and
    // `deny(x) start` don't collide.
    let mut open_regions: Vec<(DirectiveRule<'_>, Verb, u32)> = Vec::new();
    // Built on first use, since most files have no statement directive.
    let mut sorted_spans: Option<Vec<Span>> = None;
    let source_end = index_to_offset(source.len());

    for comment in comments {
        let Some(DirectiveComment { verb, extent: Some(extent), rules }) =
            parse_directive(comment.span.text(source), comment.span.start)
        else {
            continue;
        };

        match extent {
            Extent::File => {
                let range = Span::new(0, source_end);
                directives.extend(rules.iter().map(|&(rule, _)| Directive { rule, verb, range }));
            }
            Extent::Statement => {
                let stmt_spans = sorted_spans.get_or_insert_with(|| sorted_stmt_spans(nodes));
                let range = stmt_range(comment.span, stmt_spans, source, source_end, version);
                directives.extend(rules.iter().map(|&(rule, _)| Directive { rule, verb, range }));
            }
            Extent::RegionStart => {
                open_regions.extend(rules.iter().map(|&(rule, _)| (rule, verb, comment.span.end)));
            }
            Extent::RegionEnd => {
                for &(rule, _) in &rules {
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

fn sorted_stmt_spans(nodes: &Nodes<'_>) -> Vec<Span> {
    let mut stmt_spans: Vec<Span> = nodes
        .as_slice()
        .iter()
        .filter_map(|node| match node.kind {
            NodeKind::Statement(stmt) => Some(stmt.span()),
            NodeKind::LastStatement(last_stmt) => Some(last_stmt.span()),
            NodeKind::Expression(_) | NodeKind::Table(_) => None,
        })
        .collect();
    stmt_spans.sort_unstable_by_key(|span| span.start);
    stmt_spans
}

/// A directive trailing a statement on the same line
/// (`local x = 1 -- luck: allow(...)`) covers that statement. Otherwise it
/// covers the next one.
fn stmt_range(
    comment_span: Span,
    stmt_spans: &[Span],
    source: &str,
    source_end: u32,
    version: LuaVersion,
) -> Span {
    let trailed_span = stmt_spans
        .iter()
        .filter(|stmt_span| stmt_span.end <= comment_span.start)
        .max_by_key(|stmt_span| stmt_span.end)
        .filter(|stmt_span| {
            !has_line_break(Span::new(stmt_span.end, comment_span.start).text(source), version)
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
    use crate::fixtures::diagnostic;

    fn directive(rule: DirectiveRule<'_>, verb: Verb) -> Directive<'_> {
        Directive { rule, verb, range: Span::new(0, 100) }
    }

    #[test]
    fn parse_allow_single() {
        let parsed = parse_directive("-- luck: allow(foo)", 0).expect("parse");
        assert_eq!(parsed.verb, Verb::Allow);
        assert_eq!(parsed.extent, Some(Extent::Statement));
        assert_eq!(parsed.rules.len(), 1);
        assert_eq!(parsed.rules[0].0, DirectiveRule::Named("foo"));
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
        let rules: Vec<DirectiveRule<'_>> = parsed.rules.iter().map(|&(rule, _)| rule).collect();
        assert_eq!(
            rules,
            [DirectiveRule::Named("a"), DirectiveRule::Named("b"), DirectiveRule::Named("c")]
        );
    }

    #[test]
    fn parse_file_level() {
        let parsed = parse_directive("-- #luck: allow(foo)", 0).expect("parse");
        assert_eq!(parsed.extent, Some(Extent::File));
        assert_eq!(parsed.rules[0].0, DirectiveRule::Named("foo"));
        let with_start = parse_directive("-- #luck: allow(foo) start", 0).expect("parse");
        assert_eq!(with_start.extent, Some(Extent::File));
    }

    #[test]
    fn parse_block_comment_ignores_closing_brackets() {
        let parsed = parse_directive("--[[ luck: allow(foo) end ]]", 0).expect("parse");
        assert_eq!(parsed.extent, Some(Extent::RegionEnd));
    }

    #[test]
    fn parse_leveled_block_comment_ignores_its_brackets() {
        let parsed = parse_directive("--[==[ luck: allow(foo) start ]==]", 0).expect("parse");
        assert_eq!(parsed.extent, Some(Extent::RegionStart));
        assert_eq!(parsed.rules[0].0, DirectiveRule::Named("foo"));
    }

    #[test]
    fn name_spans_point_at_each_name() {
        // -- luck: allow(foo, bar)
        // 012345678901234567890123, shifted by the comment's start of 10
        let parsed = parse_directive("-- luck: allow(foo, bar)", 10).expect("parse");
        let spans: Vec<Span> = parsed.rules.iter().map(|&(_, span)| span).collect();
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
        assert_eq!(parsed.rules[0].0, DirectiveRule::Named("foo"));
    }

    #[test]
    fn allow_drops_covered_diag() {
        let mut diags = vec![diagnostic("x", Span::new(10, 20))];
        apply_directives(&[directive(DirectiveRule::Named("x"), Verb::Allow)], &mut diags);
        assert_eq!(diags, Vec::<LintDiagnostic>::new());
    }

    #[test]
    fn deny_makes_covered_diag_an_error() {
        let mut diags = vec![diagnostic("x", Span::new(10, 20))];
        apply_directives(&[directive(DirectiveRule::Named("x"), Verb::Deny)], &mut diags);
        assert_eq!(diags.len(), 1);
        assert_eq!(diags[0].severity, Severity::Error);
    }

    #[test]
    fn warn_makes_covered_diag_a_warning() {
        let mut diag = diagnostic("x", Span::new(10, 20));
        diag.severity = Severity::Error;
        let mut diags = vec![diag];
        apply_directives(&[directive(DirectiveRule::Named("x"), Verb::Warn)], &mut diags);
        assert_eq!(diags.len(), 1);
        assert_eq!(diags[0].severity, Severity::Warning);
    }

    #[test]
    fn wildcard_matches_any_rule() {
        let mut diags = vec![diagnostic("anything", Span::new(10, 20))];
        apply_directives(&[directive(DirectiveRule::Wildcard, Verb::Allow)], &mut diags);
        assert_eq!(diags, Vec::<LintDiagnostic>::new());
    }
}
