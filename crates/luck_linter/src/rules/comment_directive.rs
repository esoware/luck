use luck_ast::shared::Block;
use luck_token::{CommentKind, Span};

use crate::diagnostic::{Category, LintDiagnostic, Severity};
use crate::rule::{LintContext, Rule};

pub struct CommentDirective;

const KNOWN_DIRECTIVES: &[&str] = &[
    "nolint",
    "nocheck",
    "nonstrict",
    "strict",
    "optimize",
    "native",
];

impl Rule for CommentDirective {
    fn name(&self) -> &'static str {
        "comment_directive"
    }

    fn category(&self) -> Category {
        Category::Correctness
    }

    fn default_severity(&self) -> Severity {
        Severity::Warning
    }

    fn description(&self) -> &'static str {
        "Invalid or misplaced --! comment directive."
    }

    fn check(&self, ctx: &LintContext) -> Vec<LintDiagnostic> {
        if !ctx.semantic.version.is_luau() {
            return Vec::new();
        }
        let first_code_start = first_code_start(ctx.block);
        let mut diagnostics = Vec::new();
        let mut has_seen_mode = false;
        for comment in ctx.comments {
            if comment.kind != CommentKind::Line {
                continue;
            }
            let text = &ctx.source[comment.span.start as usize..comment.span.end as usize];
            let Some(body) = text
                .strip_prefix("--")
                .and_then(|rest| rest.strip_prefix('!'))
            else {
                continue;
            };
            // Luau only honors hot comments that precede all code; a
            // directive after the first statement is dead weight.
            if first_code_start.is_some_and(|start| comment.span.start > start) {
                diagnostics.push(LintDiagnostic::new(
                    "comment_directive",
                    "comment directive is ignored because it is placed after the first statement",
                    comment.span,
                ));
                continue;
            }
            check_directive(
                body,
                comment.span.start + 3,
                &mut has_seen_mode,
                &mut diagnostics,
            );
        }
        diagnostics
    }
}

fn first_code_start(block: &Block) -> Option<u32> {
    let stmt_start = block.stmts.first().map(|stmt| stmt.span().start);
    let last_start = block.last_stmt.as_ref().map(|last| last.span().start);
    match (stmt_start, last_start) {
        (Some(a), Some(b)) => Some(a.min(b)),
        (a, b) => a.or(b),
    }
}

fn check_directive(
    body: &str,
    word_start: u32,
    has_seen_mode: &mut bool,
    diagnostics: &mut Vec<LintDiagnostic>,
) {
    let word_len = body.find(char::is_whitespace).unwrap_or(body.len());
    let (word, rest) = body.split_at(word_len);
    let word_span = Span::new(word_start, word_start + word_len as u32);
    match word {
        // Nolint arguments are Luau's own lint names, not ours; validating
        // them here would flag every legitimate use.
        "nolint" => {}
        "native" => {
            if let Some(extra_span) = trailing_symbols_span(rest, word_span.end) {
                diagnostics.push(LintDiagnostic::new(
                    "comment_directive",
                    "native directive has extra symbols at the end of the line",
                    extra_span,
                ));
            }
        }
        "nocheck" | "nonstrict" | "strict" => {
            if let Some(extra_span) = trailing_symbols_span(rest, word_span.end) {
                diagnostics.push(LintDiagnostic::new(
                    "comment_directive",
                    "comment directive with the type checking mode has extra symbols at the end of the line",
                    extra_span,
                ));
            } else if *has_seen_mode {
                diagnostics.push(LintDiagnostic::new(
                    "comment_directive",
                    "comment directive with the type checking mode has already been used",
                    word_span,
                ));
            } else {
                *has_seen_mode = true;
            }
        }
        "optimize" => {
            let level = rest.trim();
            if level.is_empty() {
                diagnostics.push(LintDiagnostic::new(
                    "comment_directive",
                    "optimize directive requires an optimization level",
                    word_span,
                ));
            } else if !matches!(level, "0" | "1" | "2") {
                let level_offset = (rest.len() - rest.trim_start().len()) as u32;
                let level_start = word_span.end + level_offset;
                diagnostics.push(LintDiagnostic::new(
                    "comment_directive",
                    format!("unknown optimization level '{level}', 0..2 expected"),
                    Span::new(level_start, level_start + level.len() as u32),
                ));
            }
        }
        unknown => {
            let mut message = format!("unknown comment directive '{unknown}'");
            if let Some(suggestion) = closest_directive(unknown) {
                message.push_str(&format!("; did you mean '{suggestion}'?"));
            }
            diagnostics.push(LintDiagnostic::new("comment_directive", message, word_span));
        }
    }
}

/// Span of non-whitespace trailing content after a directive word, or
/// `None` when the rest of the line is blank. `base` is the byte offset
/// of the first character of `rest`.
fn trailing_symbols_span(rest: &str, base: u32) -> Option<Span> {
    let trimmed_len = rest.trim_end().len();
    let first = rest[..trimmed_len].find(|c: char| !c.is_whitespace())?;
    Some(Span::new(base + first as u32, base + trimmed_len as u32))
}

fn closest_directive(word: &str) -> Option<&'static str> {
    KNOWN_DIRECTIVES
        .iter()
        .copied()
        .map(|candidate| (levenshtein(word, candidate), candidate))
        .filter(|&(distance, _)| distance <= 2)
        .min_by_key(|&(distance, _)| distance)
        .map(|(_, candidate)| candidate)
}

/// Iterative two-row Levenshtein; directive names are short, so the
/// quadratic cost is irrelevant.
fn levenshtein(a: &str, b: &str) -> usize {
    if a == b {
        return 0;
    }
    let a_bytes = a.as_bytes();
    let b_bytes = b.as_bytes();
    if a_bytes.is_empty() {
        return b_bytes.len();
    }
    if b_bytes.is_empty() {
        return a_bytes.len();
    }
    let mut prev: Vec<usize> = (0..=b_bytes.len()).collect();
    let mut curr: Vec<usize> = vec![0; b_bytes.len() + 1];
    for i in 1..=a_bytes.len() {
        curr[0] = i;
        for j in 1..=b_bytes.len() {
            let cost = usize::from(a_bytes[i - 1] != b_bytes[j - 1]);
            curr[j] = (prev[j] + 1).min(curr[j - 1] + 1).min(prev[j - 1] + cost);
        }
        std::mem::swap(&mut prev, &mut curr);
    }
    prev[b_bytes.len()]
}

#[cfg(test)]
mod tests {
    use super::*;
    use luck_token::LuaVersion;

    fn run(source: &str) -> Vec<LintDiagnostic> {
        crate::test_support::run_rule(&CommentDirective, source, LuaVersion::Luau)
    }

    #[test]
    fn flags_unknown_directive() {
        let diags = run("--!foobar\nlocal _x = 1");
        assert_eq!(diags.len(), 1, "{diags:?}");
        assert!(
            diags[0]
                .message
                .contains("unknown comment directive 'foobar'")
        );
        assert!(!diags[0].message.contains("did you mean"));
    }

    #[test]
    fn flags_unknown_directive_with_suggestion() {
        let diags = run("--!strcit\nlocal _x = 1");
        assert_eq!(diags.len(), 1, "{diags:?}");
        assert!(
            diags[0].message.contains("did you mean 'strict'"),
            "{diags:?}"
        );
    }

    #[test]
    fn flags_directive_after_first_statement() {
        let diags = run("local _x = 1\n--!strict");
        assert_eq!(diags.len(), 1, "{diags:?}");
        assert!(diags[0].message.contains("after the first statement"));
    }

    #[test]
    fn flags_directive_after_lone_return() {
        let diags = run("return 1\n--!strict");
        assert_eq!(diags.len(), 1, "{diags:?}");
        assert!(diags[0].message.contains("after the first statement"));
    }

    #[test]
    fn flags_extra_symbols_after_mode() {
        let diags = run("--!strict yes\nlocal _x = 1");
        assert_eq!(diags.len(), 1, "{diags:?}");
        assert!(diags[0].message.contains("extra symbols"));
    }

    #[test]
    fn flags_second_mode_directive() {
        let diags = run("--!strict\n--!nocheck\nlocal _x = 1");
        assert_eq!(diags.len(), 1, "{diags:?}");
        assert!(diags[0].message.contains("already been used"));
    }

    #[test]
    fn flags_optimize_without_level() {
        let diags = run("--!optimize\nlocal _x = 1");
        assert_eq!(diags.len(), 1, "{diags:?}");
        assert!(diags[0].message.contains("requires an optimization level"));
    }

    #[test]
    fn flags_optimize_bad_level() {
        let diags = run("--!optimize 3\nlocal _x = 1");
        assert_eq!(diags.len(), 1, "{diags:?}");
        assert!(
            diags[0]
                .message
                .contains("unknown optimization level '3', 0..2 expected"),
            "{diags:?}"
        );
    }

    #[test]
    fn flags_extra_symbols_after_native() {
        let diags = run("--!native on\nlocal _x = 1");
        assert_eq!(diags.len(), 1, "{diags:?}");
        assert!(diags[0].message.contains("extra symbols"));
    }

    #[test]
    fn ignores_valid_directives() {
        let diags = run("--!strict\n--!native\n--!optimize 2\nlocal _x = 1");
        assert!(diags.is_empty(), "{diags:?}");
    }

    #[test]
    fn ignores_nolint_with_argument() {
        let diags = run("--!nolint UnknownGlobal\nlocal _x = 1");
        assert!(diags.is_empty(), "{diags:?}");
    }

    #[test]
    fn ignores_plain_comments() {
        let diags = run("-- not a directive\nlocal _x = 1");
        assert!(diags.is_empty(), "{diags:?}");
    }

    #[test]
    fn ignores_non_luau() {
        let diags = crate::test_support::run_rule(
            &CommentDirective,
            "--!foobar\nlocal _x = 1",
            LuaVersion::Lua54,
        );
        assert!(diags.is_empty(), "{diags:?}");
    }
}
