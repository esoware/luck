use luck_token::{Comment, Span};

use crate::diagnostic::{LintDiagnostic, Severity};

/// Suppression verb specified by a `-- luck:` directive.
///
/// `Allow` filters diagnostics out, `Deny` forces them to `Error`, and
/// `Warn` forces them to `Warning`. The same verb-set Selene uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SuppressionKind {
    Allow,
    Deny,
    Warn,
}

/// One resolved suppression directive: covers `[start, end)` byte range
/// for `rule` (or `*` wildcard) with the given verb.
#[derive(Debug, Clone)]
pub struct Directive {
    pub rule: String,
    pub kind: SuppressionKind,
    pub start: u32,
    pub end: u32,
}

/// One parsed comment that referenced suppression directives. Used by the
/// `invalid_lint_filter` meta-rule to point diagnostics back at the comment.
#[derive(Debug, Clone)]
pub struct DirectiveSite {
    pub rule: String,
    pub kind: SuppressionKind,
    /// Span of the rule-name token inside the comment.
    pub name_span: Span,
    /// File-level prefix (`-- #luck:`) doesn't have a target statement -
    /// invalid filters still get flagged but we won't double-count them
    /// against statement-level region tracking.
    pub is_file_level: bool,
}

/// Manages lint suppression via comments.
#[derive(Default)]
pub struct Suppression {
    directives: Vec<Directive>,
    /// Every rule-name reference we saw, regardless of whether it
    /// produced a directive (file-level too). The meta-rule consumes this.
    sites: Vec<DirectiveSite>,
}

impl Suppression {
    /// Parse `-- luck:` and `-- #luck:` directives.
    ///
    /// `statement_spans` is a sorted list of `(start, end)` byte offsets for every
    /// statement in the file (including nested ones). Used to resolve the end
    /// of single-statement (non-region) suppressions.
    pub fn from_comments(
        comments: &[Comment],
        source: &str,
        statement_spans: &[(u32, u32)],
    ) -> Self {
        let mut directives = Vec::new();
        let mut sites = Vec::new();
        // (rule, kind, start_byte). One stack per (rule, kind) pair so
        // `allow(x) start` and `deny(x) start` don't collide.
        let mut region_starts: Vec<(String, SuppressionKind, u32)> = Vec::new();
        let source_end = source.len() as u32;

        for comment in comments {
            let text = &source[comment.span.start as usize..comment.span.end as usize];

            let Some(parsed) = parse_directive(text, comment.span.start) else {
                continue;
            };

            for entry in &parsed.entries {
                sites.push(DirectiveSite {
                    rule: entry.rule.clone(),
                    kind: parsed.kind,
                    name_span: entry.name_span,
                    is_file_level: parsed.is_file_level,
                });
            }

            // File-level directives ignore region/single semantics - they
            // cover the entire file.
            if parsed.is_file_level {
                for entry in parsed.entries {
                    directives.push(Directive {
                        rule: entry.rule,
                        kind: parsed.kind,
                        start: 0,
                        end: source_end,
                    });
                }
                continue;
            }

            match parsed.modifier {
                Modifier::Start => {
                    for entry in parsed.entries {
                        region_starts.push((entry.rule, parsed.kind, comment.span.end));
                    }
                }
                Modifier::End => {
                    for entry in &parsed.entries {
                        if let Some(position) = region_starts.iter().rposition(|(name, kind, _)| {
                            name == &entry.rule && *kind == parsed.kind
                        }) {
                            let (rule, kind, start) = region_starts.remove(position);
                            directives.push(Directive {
                                rule,
                                kind,
                                start,
                                end: comment.span.start,
                            });
                        }
                    }
                }
                Modifier::None => {
                    let comment_end = comment.span.end;
                    // A trailing directive on a statement's own line covers
                    // THAT statement (`local x = 1 -- luck: allow(...)`),
                    // matching Luacheck/ESLint same-line semantics.
                    // Otherwise it covers the next statement.
                    let same_line_stmt = statement_spans
                        .iter()
                        .filter(|(_start, end)| *end <= comment.span.start)
                        .max_by_key(|(_start, end)| *end)
                        .filter(|(_start, end)| {
                            !source[*end as usize..comment.span.start as usize].contains('\n')
                        })
                        .copied();
                    let (range_start, range_end) = match same_line_stmt {
                        Some((stmt_start, stmt_end)) => (stmt_start, stmt_end),
                        None => {
                            let next_end = statement_spans
                                .iter()
                                .find(|(start, _end)| *start >= comment_end)
                                .map(|(_start, end)| *end)
                                .unwrap_or(source_end);
                            (comment_end, next_end)
                        }
                    };
                    for entry in parsed.entries {
                        directives.push(Directive {
                            rule: entry.rule,
                            kind: parsed.kind,
                            start: range_start,
                            end: range_end,
                        });
                    }
                }
            }
        }

        // Unclosed regions extend to the end of the file - matches the
        // intent of "I forgot the end" rather than silently dropping it.
        for (rule, kind, start) in region_starts {
            directives.push(Directive {
                rule,
                kind,
                start,
                end: source_end,
            });
        }

        Suppression { directives, sites }
    }

    /// All directives in source order. Exposed for the meta-rule.
    pub fn directives(&self) -> &[Directive] {
        &self.directives
    }

    /// Every rule-name reference parsed from comments (allow/deny/warn,
    /// statement / region / file). Used by `invalid_lint_filter`.
    pub fn sites(&self) -> &[DirectiveSite] {
        &self.sites
    }

    /// Apply suppression to diagnostics: drop those covered by `allow`,
    /// adjust severity for those covered by `deny`/`warn`.
    pub fn apply(&self, diagnostics: &mut Vec<LintDiagnostic>) {
        diagnostics.retain_mut(|diag| {
            // Narrowest covering directive wins: a statement-level marker
            // beats a region, a region beats file-level - regardless of
            // where each was parsed. Parse order settled collisions
            // before, which let an unclosed region (appended last)
            // override every narrower directive in the file.
            let winner = self
                .directives
                .iter()
                .filter(|directive| covers(directive, diag))
                .min_by_key(|directive| directive.end - directive.start);
            match winner.map(|directive| directive.kind) {
                Some(SuppressionKind::Allow) => false,
                Some(SuppressionKind::Deny) => {
                    diag.severity = Severity::Error;
                    true
                }
                Some(SuppressionKind::Warn) => {
                    diag.severity = Severity::Warning;
                    true
                }
                None => true,
            }
        });
    }
}

fn covers(directive: &Directive, diag: &LintDiagnostic) -> bool {
    (directive.rule == "*" || directive.rule == diag.rule)
        && diag.span.start >= directive.start
        && diag.span.start < directive.end
}

/// Parsed directive header, before resolution to a span.
struct Parsed {
    kind: SuppressionKind,
    modifier: Modifier,
    is_file_level: bool,
    entries: Vec<ParsedEntry>,
}

struct ParsedEntry {
    rule: String,
    /// Absolute byte span of the rule-name token inside the source.
    name_span: Span,
}

#[derive(Debug, Clone, Copy)]
enum Modifier {
    None,
    Start,
    End,
}

/// Parse `-- luck: <verb>(rule, ...)` or `-- #luck: <verb>(rule, ...)`.
///
/// `comment_base` is the absolute byte offset of the comment's first
/// character - we need it to compute name spans for the meta-rule.
fn parse_directive(text: &str, comment_base: u32) -> Option<Parsed> {
    // Strip leading dashes (line comment) or block-comment delimiters.
    // We don't care about the comment kind here; we only need to find the
    // `luck:` or `#luck:` marker inside.
    let trimmed = text.trim_start_matches('-');
    let trimmed = trimmed.trim_start_matches('[').trim_start_matches('[');
    let body = trimmed.trim_start();

    let leading_offset = (text.len() - body.len()) as u32;
    let (after_prefix, is_file_level) = if let Some(rest) = body.strip_prefix("#luck:") {
        (rest, true)
    } else {
        (body.strip_prefix("luck:")?, false)
    };

    let prefix_len = (body.len() - after_prefix.len()) as u32;
    let after_prefix_trimmed = after_prefix.trim_start();
    let verb_offset =
        leading_offset + prefix_len + (after_prefix.len() - after_prefix_trimmed.len()) as u32;

    let kind = after_prefix_trimmed
        .strip_prefix("allow")
        .map(|rest| (SuppressionKind::Allow, rest, "allow"))
        .or_else(|| {
            after_prefix_trimmed
                .strip_prefix("deny")
                .map(|rest| (SuppressionKind::Deny, rest, "deny"))
        })
        .or_else(|| {
            after_prefix_trimmed
                .strip_prefix("warn")
                .map(|rest| (SuppressionKind::Warn, rest, "warn"))
        })?;

    let (verb_kind, after_verb, verb_str) = kind;
    // Measure the gap BEFORE trimming - shadowing first made the
    // whitespace term always zero, skewing name spans in `allow (rule)`.
    let after_verb_trimmed = after_verb.trim_start();
    let after_verb_offset =
        verb_offset + verb_str.len() as u32 + (after_verb.len() - after_verb_trimmed.len()) as u32;
    let after_verb = after_verb_trimmed;

    let inside = after_verb.strip_prefix('(')?;
    let close_paren = inside.find(')')?;
    let names_str = &inside[..close_paren];
    let after_close = inside[close_paren + 1..].trim();

    // Parenthesis sits one byte before the name list.
    let names_base = after_verb_offset + 1;

    let mut entries = Vec::new();
    let mut cursor = 0usize;
    for raw in names_str.split(',') {
        let raw_len = raw.len();
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            // `allow()` is a syntax error; skip and let validation note it
            // elsewhere. Returning None would silently drop the comment.
            cursor += raw_len + 1;
            continue;
        }
        let leading_ws = raw.find(|c: char| !c.is_whitespace()).unwrap_or(0);
        let name_start = comment_base + names_base + cursor as u32 + leading_ws as u32;
        let name_end = name_start + trimmed.len() as u32;
        entries.push(ParsedEntry {
            rule: trimmed.to_string(),
            name_span: Span::new(name_start, name_end),
        });
        cursor += raw_len + 1; // +1 for the comma we split on
    }
    if entries.is_empty() {
        return None;
    }

    let modifier = match after_close {
        "start" => Modifier::Start,
        "end" => Modifier::End,
        "" => Modifier::None,
        // Block comments may have a trailing `]]`. Strip it. Anything
        // else (`strat`, `ned`, stray words) rejects the whole directive:
        // silently downgrading a typo'd region marker to statement-level
        // changes what gets suppressed without any signal to the user.
        other => {
            let stripped = other.trim_end_matches(']').trim();
            match stripped {
                "start" => Modifier::Start,
                "end" => Modifier::End,
                "" => Modifier::None,
                _ => return None,
            }
        }
    };

    Some(Parsed {
        kind: verb_kind,
        modifier,
        is_file_level,
        entries,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diagnostic::{Category, LintDiagnostic};
    use luck_token::Span;

    fn diag(rule: &'static str, start: u32, end: u32) -> LintDiagnostic {
        LintDiagnostic {
            rule,
            category: Category::Correctness,
            severity: Severity::Warning,
            message: "test".into(),
            span: Span::new(start, end),
            help: None,
            fix: None,
        }
    }

    #[test]
    fn parse_allow_single() {
        let parsed = parse_directive("-- luck: allow(foo)", 0).expect("parse");
        assert_eq!(parsed.kind, SuppressionKind::Allow);
        assert!(matches!(parsed.modifier, Modifier::None));
        assert_eq!(parsed.entries.len(), 1);
        assert_eq!(parsed.entries[0].rule, "foo");
    }

    #[test]
    fn parse_deny_region() {
        let parsed = parse_directive("-- luck: deny(foo) start", 0).expect("parse");
        assert_eq!(parsed.kind, SuppressionKind::Deny);
        assert!(matches!(parsed.modifier, Modifier::Start));
    }

    #[test]
    fn parse_warn_multi() {
        let parsed = parse_directive("-- luck: warn(a, b, c)", 0).expect("parse");
        assert_eq!(parsed.kind, SuppressionKind::Warn);
        assert_eq!(parsed.entries.len(), 3);
        assert_eq!(parsed.entries[0].rule, "a");
        assert_eq!(parsed.entries[1].rule, "b");
        assert_eq!(parsed.entries[2].rule, "c");
    }

    #[test]
    fn parse_file_level() {
        let parsed = parse_directive("-- #luck: allow(foo)", 0).expect("parse");
        assert!(parsed.is_file_level);
        assert_eq!(parsed.entries[0].rule, "foo");
    }

    #[test]
    fn name_span_points_at_token() {
        // -- luck: allow(foo)
        // 0123456789012345678
        let parsed = parse_directive("-- luck: allow(foo)", 0).expect("parse");
        let span = parsed.entries[0].name_span;
        assert_eq!(span.start, 15);
        assert_eq!(span.end, 18);
    }

    #[test]
    fn unknown_verb_rejected() {
        assert!(parse_directive("-- luck: forbid(foo)", 0).is_none());
    }

    #[test]
    fn covers_allow_filters() {
        let supp = Suppression {
            directives: vec![Directive {
                rule: "x".into(),
                kind: SuppressionKind::Allow,
                start: 0,
                end: 100,
            }],
            sites: Vec::new(),
        };
        let mut diags = vec![diag("x", 10, 20)];
        supp.apply(&mut diags);
        assert!(diags.is_empty());
    }

    #[test]
    fn covers_deny_escalates_severity() {
        let supp = Suppression {
            directives: vec![Directive {
                rule: "x".into(),
                kind: SuppressionKind::Deny,
                start: 0,
                end: 100,
            }],
            sites: Vec::new(),
        };
        let mut diags = vec![diag("x", 10, 20)];
        supp.apply(&mut diags);
        assert_eq!(diags.len(), 1);
        assert_eq!(diags[0].severity, Severity::Error);
    }

    #[test]
    fn covers_warn_downgrades_severity() {
        let mut d = diag("x", 10, 20);
        d.severity = Severity::Error;
        let supp = Suppression {
            directives: vec![Directive {
                rule: "x".into(),
                kind: SuppressionKind::Warn,
                start: 0,
                end: 100,
            }],
            sites: Vec::new(),
        };
        let mut diags = vec![d];
        supp.apply(&mut diags);
        assert_eq!(diags.len(), 1);
        assert_eq!(diags[0].severity, Severity::Warning);
    }

    #[test]
    fn wildcard_matches_any_rule() {
        let supp = Suppression {
            directives: vec![Directive {
                rule: "*".into(),
                kind: SuppressionKind::Allow,
                start: 0,
                end: 100,
            }],
            sites: Vec::new(),
        };
        let mut diags = vec![diag("anything", 10, 20)];
        supp.apply(&mut diags);
        assert!(diags.is_empty());
    }
}
