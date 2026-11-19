use alloc::borrow::Cow;
use alloc::collections::BTreeMap;
use core::cmp::Reverse;
use core::error::Error;
use core::fmt;
use core::ops::Range;

use bitflags::bitflags;
use luck_allocator::Allocator;
use luck_core::LuaTarget;
use luck_parser::ParseResult;
use luck_token::Span;

use crate::config::LintConfig;
use crate::diagnostic::LintDiagnostic;
use crate::driver::lint_parsed;

/// `ESLint` stops at the same count. Needing more almost always means two
/// rules are undoing each other.
const ROUND_BUDGET: usize = 10;

bitflags! {
    /// A set of fix kinds: the kind of one fix, the kinds a rule's fixes come
    /// in, or the kinds a host applies, which are the only ones the linter
    /// builds.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
    pub struct FixKind: u8 {
        /// Keeps what the code does. `--fix` and the editor's fix-all apply
        /// it.
        const Fix = 1;
        /// Can change what the code does, or hides a problem rather than
        /// solving it, as renaming an unused variable to `_x` does. Only the
        /// user applies it, as an editor quick fix or with
        /// `--fix-suggestions`.
        const Suggestion = 1 << 1;
    }
}

/// Edits that fix one diagnostic, applied together or not at all, because
/// half of them could change what the code means.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fix {
    /// Exactly one kind.
    pub kind: FixKind,
    /// What the fix does, which an editor shows as its code action.
    pub message: Cow<'static, str>,
    pub edits: Box<[TextEdit]>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TextEdit {
    pub span: Span,
    pub replacement: Cow<'static, str>,
}

/// Builds a finding's fix, of the kind the rule reports it as. A rule gets
/// one only when the host applies fixes of that kind.
#[derive(Debug, Clone, Copy)]
pub(crate) struct RuleFixer {
    kind: FixKind,
}

impl RuleFixer {
    pub(crate) const fn new(kind: FixKind) -> Self {
        Self { kind }
    }

    pub(crate) fn fix<Message, Edits>(self, message: Message, edits: Edits) -> Fix
    where
        Message: Into<Cow<'static, str>>,
        Edits: Into<Box<[TextEdit]>>,
    {
        Fix { kind: self.kind, message: message.into(), edits: edits.into() }
    }

    pub(crate) fn replace<Message, Replacement>(
        self,
        message: Message,
        span: Span,
        replacement: Replacement,
    ) -> Fix
    where
        Message: Into<Cow<'static, str>>,
        Replacement: Into<Cow<'static, str>>,
    {
        self.fix(message, [TextEdit { span, replacement: replacement.into() }])
    }

    pub(crate) fn delete<Message>(self, message: Message, span: Span) -> Fix
    where
        Message: Into<Cow<'static, str>>,
    {
        self.replace(message, span, "")
    }
}

/// What [`fix`] leaves once no fix of the kinds it applies is left.
#[derive(Debug)]
pub struct Fixed<'ast> {
    /// The fixed source's parse, which is the parse of the source itself
    /// when nothing applied.
    pub parse: ParseResult<'ast>,
    /// The fixed source's diagnostics.
    pub diags: Vec<LintDiagnostic>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FixError {
    /// Fixes were still coming after `rounds` rounds.
    BudgetExhausted { rounds: usize },
    /// The fixes of round `round`, counted from 1, together broke the
    /// syntax, which fails with `message`.
    ReparseFailed { round: usize, message: String },
}

impl fmt::Display for FixError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::BudgetExhausted { rounds } => write!(
                f,
                "fixes kept coming after {rounds} rounds, so two rules may be undoing each other"
            ),
            Self::ReparseFailed { round, message } => {
                write!(f, "the fixes of round {round} broke the syntax: {message}")
            }
        }
    }
}

impl Error for FixError {}

/// Lints `source` for `target` and applies its fixes of the kinds `fixes`
/// holds, round after round, until none is left. Each round's text is parsed
/// into `allocator` once, both to check it and to lint it.
///
/// Of two overlapping fixes a round applies one, and the next round lints
/// again and applies the other if it still holds.
///
/// # Errors
///
/// [`FixError::BudgetExhausted`] when fixes are still coming after 10
/// rounds, and [`FixError::ReparseFailed`] when a round's fixes break the
/// syntax. Neither leaves a fixed text, so a caller writes back all of the
/// fixes or none.
///
/// # Panics
///
/// In debug builds, as [`lint_parsed`] does.
pub fn fix<'ast>(
    allocator: &'ast Allocator,
    source: &'ast str,
    target: LuaTarget,
    config: &LintConfig,
    fixes: FixKind,
) -> Result<Fixed<'ast>, FixError> {
    fix_within(allocator, source, target, config, fixes, ROUND_BUDGET)
}

/// # Errors
///
/// As for [`fix`], with `budget` as the limit.
fn fix_within<'ast>(
    allocator: &'ast Allocator,
    source: &'ast str,
    target: LuaTarget,
    config: &LintConfig,
    fixes: FixKind,
    budget: usize,
) -> Result<Fixed<'ast>, FixError> {
    let version = target.lua_version();
    let mut parse = luck_parser::parse(allocator, source, version);
    for round in 1..=budget {
        let diags = lint_parsed(&parse, target.stdlib_environment(), config, fixes);
        let Some(text) = apply(parse.source, &diags, fixes) else {
            return Ok(Fixed { parse, diags });
        };
        let reparse = luck_parser::parse(allocator, allocator.alloc_str(&text), version);
        if let Some(error) = reparse.errors.first() {
            return Err(FixError::ReparseFailed { round, message: error.message.clone() });
        }
        parse = reparse;
    }
    Err(FixError::BudgetExhausted { rounds: budget })
}

/// `source` with the fixes of `diags` of the kinds `fixes` holds that
/// [`non_overlapping_edits`] keeps, or `None` when they change nothing.
pub(crate) fn apply(source: &str, diags: &[LintDiagnostic], fixes: FixKind) -> Option<String> {
    let edits = non_overlapping_edits(diags, fixes);
    if edits.is_empty() {
        return None;
    }
    let mut text = String::with_capacity(source.len());
    let mut copied_to = 0;
    for edit in edits {
        let Range { start, end } = Range::<usize>::from(edit.span);
        text.push_str(source.get(copied_to..start).unwrap_or_default());
        text.push_str(&edit.replacement);
        copied_to = end;
    }
    text.push_str(source.get(copied_to..).unwrap_or_default());
    (text != source).then_some(text)
}

/// The edits of `diags`'s fixes of the kinds `fixes` holds that one change
/// can make together, in source order.
///
/// The fixes reaching furthest into the source are taken first, so of two
/// overlapping ones the later wins. A fix is kept or dropped whole, because
/// half of a multi-edit fix changes the code's meaning. Edits at one offset
/// keep a fixed order, whichever rule produced them.
#[must_use]
pub fn non_overlapping_edits(diags: &[LintDiagnostic], fixes: FixKind) -> Vec<&TextEdit> {
    let mut candidates: Vec<&Fix> = diags
        .iter()
        .filter_map(|diag| diag.fix.as_ref())
        .filter(|candidate| fixes.contains(candidate.kind))
        .collect();
    candidates.sort_by_key(|candidate| {
        let furthest_edit = candidate
            .edits
            .iter()
            .map(|edit| (edit.span.start, edit.span.end, Reverse(&edit.replacement)))
            .max();
        Reverse(furthest_edit)
    });
    // Each kept span's end by its start, so a candidate has two neighbors to
    // check. Two inserts at one offset don't conflict, and the wider span at
    // an offset is the one that can.
    let mut kept_spans: BTreeMap<u32, u32> = BTreeMap::new();
    let mut edits: Vec<&TextEdit> = Vec::new();
    for candidate in candidates {
        if candidate.edits.iter().any(|edit| overlaps_kept(&kept_spans, edit.span)) {
            continue;
        }
        for edit in &candidate.edits {
            let end = kept_spans.entry(edit.span.start).or_insert(edit.span.end);
            *end = edit.span.end.max(*end);
        }
        edits.extend(&candidate.edits);
    }
    edits.sort_by_key(|edit| (edit.span.start, edit.span.end, Reverse(&edit.replacement)));
    edits
}

/// Half-open overlap, so edits that only touch, like an insert at the end of
/// a replaced range, don't conflict.
fn overlaps_kept(kept_spans: &BTreeMap<u32, u32>, span: Span) -> bool {
    let is_overlapping = |(&start, &end): (&u32, &u32)| start < span.end && span.start < end;
    kept_spans.range(..=span.start).next_back().is_some_and(is_overlapping)
        || kept_spans.range(span.start..).next().is_some_and(is_overlapping)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::RuleLevel;
    use crate::fixtures::diagnostic;

    fn diag_with_fix(kind: FixKind, replacements: &[(u32, u32, &'static str)]) -> LintDiagnostic {
        let edits: Box<[TextEdit]> = replacements
            .iter()
            .map(|&(start, end, replacement)| TextEdit {
                span: Span::new(start, end),
                replacement: replacement.into(),
            })
            .collect();
        LintDiagnostic {
            fix: Some(Fix { kind, message: "test fix".into(), edits }),
            ..diagnostic("test", Span::new(0, 0))
        }
    }

    fn diag_with_edit(start: u32, end: u32, replacement: &'static str) -> LintDiagnostic {
        diag_with_fix(FixKind::Fix, &[(start, end, replacement)])
    }

    fn enable(names: &[&str]) -> LintConfig {
        let mut config = LintConfig::default();
        for name in names {
            config.rules.set_level(name, RuleLevel::Warn);
        }
        config
    }

    /// # Errors
    ///
    /// As for [`fix_within`].
    fn fixed_within(source: &str, config: &LintConfig, budget: usize) -> Result<String, FixError> {
        let allocator = Allocator::new();
        fix_within(&allocator, source, LuaTarget::Lua54, config, FixKind::Fix, budget)
            .map(|fixed| fixed.parse.source.to_owned())
    }

    /// Every finding and diagnostic holds one, built or not.
    #[test]
    #[cfg(target_pointer_width = "64")]
    fn fix_is_six_words() {
        assert_eq!(size_of::<Option<Fix>>(), 48);
    }

    #[test]
    fn applies_every_non_overlapping_edit() {
        let source = "\
local foo = 1
local bar = 2";
        let diags = [diag_with_edit(6, 9, "_foo"), diag_with_edit(20, 23, "_bar")];
        assert_eq!(
            apply(source, &diags, FixKind::Fix).as_deref(),
            Some(
                "\
local _foo = 1
local _bar = 2"
            )
        );
    }

    #[test]
    fn overlapping_edit_that_starts_later_wins() {
        let diags = [diag_with_edit(2, 6, "XX"), diag_with_edit(4, 8, "YY")];
        assert_eq!(apply("abcdefghij", &diags, FixKind::Fix).as_deref(), Some("abcdYYij"));
    }

    #[test]
    fn a_fix_overlapped_by_a_later_one_drops_whole() {
        let diags =
            [diag_with_fix(FixKind::Fix, &[(0, 1, "X"), (2, 6, "Y")]), diag_with_edit(5, 8, "Z")];
        assert_eq!(apply("abcdefghij", &diags, FixKind::Fix).as_deref(), Some("abcdeZij"));
    }

    #[test]
    fn applies_only_the_kinds_asked_for() {
        let diags = [
            diag_with_fix(FixKind::Fix, &[(0, 1, "F")]),
            diag_with_fix(FixKind::Suggestion, &[(2, 3, "S")]),
        ];
        assert_eq!(apply("abcd", &diags, FixKind::Fix).as_deref(), Some("Fbcd"));
        assert_eq!(apply("abcd", &diags, FixKind::Suggestion).as_deref(), Some("abSd"));
        assert_eq!(apply("abcd", &diags, FixKind::all()).as_deref(), Some("FbSd"));
        assert_eq!(apply("abcd", &diags, FixKind::empty()), None);
    }

    #[test]
    fn edits_that_change_nothing_apply_nothing() {
        let diags = [diag_with_edit(0, 1, "a")];
        assert_eq!(apply("abcd", &diags, FixKind::Fix), None);
        assert_eq!(apply("abcd", &[diagnostic("test", Span::new(0, 1))], FixKind::Fix), None);
    }

    #[test]
    fn fixes_from_two_rules_land_in_one_round() {
        // Their edits don't overlap, so the first round applies both, and
        // the second only confirms that none is left.
        let config = enable(&["redundant_nil_init", "string_index_to_field"]);
        let source = "\
local a = t[\"key\"]
local b = nil
return a, b";
        assert_eq!(
            fixed_within(source, &config, 2),
            Ok("\
local a = t.key
local b
return a, b"
                .to_owned())
        );
    }

    #[test]
    fn spends_a_round_confirming_that_no_fix_is_left() {
        // The inner negation wins the first round, since the outer fix
        // overlaps it, and the outer one applies in the second.
        let config = enable(&["unnecessary_negation"]);
        let source = "return not (a == (not (b == c)))";
        assert_eq!(fixed_within(source, &config, 3), Ok("return (a ~= ((b ~= c)))".to_owned()));
        assert_eq!(
            fixed_within(source, &config, 2),
            Err(FixError::BudgetExhausted { rounds: 2 }),
            "two rounds apply both fixes, and only a third confirms that none is left"
        );
    }
}
