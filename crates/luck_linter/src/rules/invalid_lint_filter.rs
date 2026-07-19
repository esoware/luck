use luck_token::Span;

use crate::diagnostic::*;
use crate::rule::{LintContext, Rule};

/// Catch typos in lint suppression directives. Without this, a stray
/// `-- luck: allow(unsued_variable)` silently does nothing, leaving the
/// author convinced their suppression is in effect.
pub struct InvalidLintFilter;

impl Rule for InvalidLintFilter {
    fn name(&self) -> &'static str {
        "invalid_lint_filter"
    }
    fn category(&self) -> Category {
        Category::Correctness
    }
    fn default_severity(&self) -> Severity {
        Severity::Warning
    }
    fn description(&self) -> &'static str {
        "lint suppression names a rule that does not exist"
    }

    fn check(&self, ctx: &LintContext) -> Vec<LintDiagnostic> {
        let _block = ctx.block;
        let _semantic = ctx.semantic;
        let source = ctx.source;
        let comments = ctx.comments;
        let known = crate::rules::registered_rule_names();
        let mut diagnostics = Vec::new();

        // Iterate comments and re-extract each directive site, then
        // validate names. We deliberately don't go through the
        // `Suppression` struct here - that's parametric on statement
        // spans, but the meta-rule only cares about the rule-name
        // tokens, which `directive_sites` is enough for.
        for comment in comments {
            let text = &source[comment.span.start as usize..comment.span.end as usize];
            for site in directive_sites(text, comment.span.start) {
                if site.rule == "*" {
                    continue;
                }
                if known.contains(&site.rule.as_str()) {
                    continue;
                }

                let mut message = format!("unknown lint rule `{}`", site.rule);
                let mut help = "this rule is not registered with the linter".to_string();
                if let Some(suggestion) = closest_match(&site.rule, &known) {
                    message.push_str(&format!("; did you mean `{suggestion}`?"));
                    help = format!("rename to `{suggestion}` to suppress that rule");
                }

                diagnostics.push(
                    LintDiagnostic::new("invalid_lint_filter", message, site.name_span)
                        .with_help(help),
                );
            }
        }

        diagnostics
    }
}

/// Minimal directive-site parser, mirroring the structure of
/// `suppression::parse_directive` but standalone - we only need rule
/// names and their positions, not the verb or modifier semantics.
struct Site {
    rule: String,
    name_span: Span,
}

fn directive_sites(text: &str, base: u32) -> Vec<Site> {
    let trimmed = text.trim_start_matches('-');
    let trimmed = trimmed.trim_start_matches('[').trim_start_matches('[');
    let body = trimmed.trim_start();
    let leading_offset = (text.len() - body.len()) as u32;
    let after_prefix = if let Some(rest) = body.strip_prefix("#luck:") {
        rest
    } else if let Some(rest) = body.strip_prefix("luck:") {
        rest
    } else {
        return Vec::new();
    };
    let prefix_len = (body.len() - after_prefix.len()) as u32;
    let after_prefix_trimmed = after_prefix.trim_start();
    let verb_offset =
        leading_offset + prefix_len + (after_prefix.len() - after_prefix_trimmed.len()) as u32;

    let (verb_len, after_verb) = if let Some(rest) = after_prefix_trimmed.strip_prefix("allow") {
        (5u32, rest)
    } else if let Some(rest) = after_prefix_trimmed.strip_prefix("deny") {
        (4u32, rest)
    } else if let Some(rest) = after_prefix_trimmed.strip_prefix("warn") {
        (4u32, rest)
    } else {
        return Vec::new();
    };

    let after_verb_trim = after_verb.trim_start();
    let after_verb_offset =
        verb_offset + verb_len + (after_verb.len() - after_verb_trim.len()) as u32;

    let Some(inside) = after_verb_trim.strip_prefix('(') else {
        return Vec::new();
    };
    let Some(close_paren) = inside.find(')') else {
        return Vec::new();
    };
    let names_str = &inside[..close_paren];
    let names_base = after_verb_offset + 1;

    let mut sites = Vec::new();
    let mut cursor = 0usize;
    for raw in names_str.split(',') {
        let raw_len = raw.len();
        let trimmed_name = raw.trim();
        if !trimmed_name.is_empty() {
            let leading_ws = raw.find(|c: char| !c.is_whitespace()).unwrap_or(0);
            let name_start = base + names_base + cursor as u32 + leading_ws as u32;
            let name_end = name_start + trimmed_name.len() as u32;
            sites.push(Site {
                rule: trimmed_name.to_string(),
                name_span: Span::new(name_start, name_end),
            });
        }
        cursor += raw_len + 1; // Skip the comma consumed by split(',').
    }
    sites
}

/// Return the closest match (by Levenshtein distance) from `candidates`
/// to `needle`, but only if the distance is below a small threshold.
/// Without the threshold a totally-different typo would surface a
/// nonsensical "did you mean" - worse than no suggestion.
fn closest_match(needle: &str, candidates: &[&str]) -> Option<String> {
    let mut best: Option<(usize, &str)> = None;
    for &candidate in candidates {
        let distance = levenshtein(needle, candidate);
        let max_allowed = (needle.len().max(candidate.len()) / 3).max(2);
        if distance <= max_allowed && best.map(|(b, _)| distance < b).unwrap_or(true) {
            best = Some((distance, candidate));
        }
    }
    best.map(|(_, s)| s.to_string())
}

/// Iterative two-row Levenshtein. Compact and good enough at this
/// scale - rule names are short.
fn levenshtein(a: &str, b: &str) -> usize {
    if a == b {
        return 0;
    }
    let a_bytes = a.as_bytes();
    let b_bytes = b.as_bytes();
    let m = a_bytes.len();
    let n = b_bytes.len();
    if m == 0 {
        return n;
    }
    if n == 0 {
        return m;
    }
    let mut prev: Vec<usize> = (0..=n).collect();
    let mut curr: Vec<usize> = vec![0; n + 1];
    for i in 1..=m {
        curr[0] = i;
        for j in 1..=n {
            let cost = if a_bytes[i - 1] == b_bytes[j - 1] {
                0
            } else {
                1
            };
            curr[j] = (prev[j] + 1).min(curr[j - 1] + 1).min(prev[j - 1] + cost);
        }
        std::mem::swap(&mut prev, &mut curr);
    }
    prev[n]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn levenshtein_basic() {
        assert_eq!(levenshtein("", ""), 0);
        assert_eq!(levenshtein("abc", "abc"), 0);
        assert_eq!(levenshtein("abc", "abd"), 1);
        assert_eq!(levenshtein("abc", "ab"), 1);
        assert_eq!(levenshtein("kitten", "sitting"), 3);
    }

    #[test]
    fn closest_match_picks_near_neighbor() {
        let known = &["unused_variable", "shadowing"];
        assert_eq!(
            closest_match("unsued_variable", known).as_deref(),
            Some("unused_variable")
        );
    }

    #[test]
    fn closest_match_none_for_far_strings() {
        let known = &["unused_variable"];
        assert!(closest_match("xx", known).is_none());
    }

    #[test]
    fn directive_sites_basic() {
        let text = "-- luck: allow(foo, bar)";
        let sites = directive_sites(text, 0);
        assert_eq!(sites.len(), 2);
        assert_eq!(sites[0].rule, "foo");
        assert_eq!(sites[1].rule, "bar");
        assert_eq!(sites[0].name_span.start, 15);
        assert_eq!(sites[0].name_span.end, 18);
    }

    #[test]
    fn directive_sites_file_level() {
        let sites = directive_sites("-- #luck: deny(unused_variable)", 0);
        assert_eq!(sites.len(), 1);
        assert_eq!(sites[0].rule, "unused_variable");
    }

    #[test]
    fn directive_sites_skips_non_luck() {
        assert!(directive_sites("-- just a comment", 0).is_empty());
    }

    #[test]
    fn flags_unknown_name() {
        // Use the linter's full pipeline so registered names are real.
        let diags = crate::test_support::run_rule(
            &InvalidLintFilter,
            "-- luck: allow(not_a_real_rule)\nlocal _x = 1",
            luck_token::LuaVersion::Lua54,
        );
        assert_eq!(diags.len(), 1, "{diags:?}");
        assert!(diags[0].message.contains("not_a_real_rule"));
    }

    #[test]
    fn ignores_wildcard() {
        let diags = crate::test_support::run_rule(
            &InvalidLintFilter,
            "-- luck: allow(*)\nlocal _x = 1",
            luck_token::LuaVersion::Lua54,
        );
        assert!(diags.is_empty(), "{diags:?}");
    }

    #[test]
    fn ignores_known_name() {
        let diags = crate::test_support::run_rule(
            &InvalidLintFilter,
            "-- luck: allow(unused_variable)\nlocal _x = 1",
            luck_token::LuaVersion::Lua54,
        );
        assert!(diags.is_empty(), "{diags:?}");
    }
}
