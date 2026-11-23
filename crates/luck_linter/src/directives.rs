//! The lint directives of one file, read once before any rule runs.
//!
//! Each rule a directive names is an entry, which covers the whole file, a
//! region, or a statement. A rule's diagnostics meet its entries' ranges, and
//! those of `*`, in one walk in source order as they're reported, so matching
//! is linear in both.

use core::cmp::Reverse;
use core::ops::Range;

use luck_ast::{GetSpan as _, Statement};
use luck_token::{
    Comment, CommentContent, DirectiveError, LintDirective, LintExtent, LintVerb, LuaVersion, Span,
    has_line_break,
};

use crate::nodes::{NodeKind, Nodes};
use crate::rules::RULES;

/// Past the last offset, so a range to the end of the file also holds a
/// diagnostic there.
const FILE_END: u32 = u32::MAX;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum Target {
    /// The rule at this index of `RULES`.
    Rule(usize),
    /// `*`.
    Every,
    /// A name no rule has, which covers nothing.
    Unknown,
}

/// What's wrong with a `luck:` comment, which `invalid_lint_filter` reports.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DirectiveProblem {
    Malformed(DirectiveError),
    /// A region's `end` that closes no `start` of its verb and rule, at the
    /// rule's name.
    UnopenedEnd {
        verb: LintVerb,
        name: Span,
    },
}

#[derive(Debug)]
pub(crate) struct Entry {
    pub target: Target,
    pub name: Span,
    /// The index of the directive that lists it.
    pub directive: usize,
    /// For a region's `start`, the entry of the `end` that closes it.
    pub end: Option<usize>,
}

#[derive(Debug)]
pub(crate) struct Directive {
    /// The comment it's written in.
    pub span: Span,
    pub verb: LintVerb,
    pub extent: LintExtent,
    /// Its rule list, between the parentheses.
    pub list: Span,
    pub entries: Range<usize>,
}

/// The half-open range an entry covers.
#[derive(Debug, Clone, Copy)]
struct Interval {
    target: Target,
    start: u32,
    end: u32,
    entry: usize,
    verb: LintVerb,
}

impl Interval {
    /// Of the ranges holding an offset, the greatest wins: the one that
    /// starts last, then the narrowest, then the one written last. So a
    /// statement's beats a region's around it, and a region's the file's.
    const fn rank(self) -> (u32, Reverse<u32>, usize) {
        (self.start, Reverse(self.end), self.entry)
    }
}

#[derive(Debug, Default)]
pub(crate) struct Directives {
    list: Vec<Directive>,
    entries: Vec<Entry>,
    /// By target, then by rank.
    intervals: Vec<Interval>,
    /// What's wrong with each malformed `luck:` comment, and with each region
    /// `end` that closes none.
    errors: Vec<DirectiveProblem>,
    /// By rule, whether a `deny` or `warn` names it, which runs it here
    /// whatever config says. Empty when none does.
    enabled: Vec<bool>,
}

impl Directives {
    /// The directives in `comments`. Those on a statement cover nothing until
    /// [`Self::cover_statements`] finds it.
    pub(crate) fn new(comments: &[Comment], source: &[u8]) -> Self {
        let mut directives = Self::default();
        let mut open_regions: Vec<OpenRegion<'_>> = Vec::new();
        for comment in comments {
            if !matches!(
                comment.content,
                CommentContent::LintDirective | CommentContent::InvalidDirective
            ) {
                continue;
            }
            match LintDirective::parse(comment, source) {
                Ok(Some(directive)) => {
                    directives.add(comment.span, directive, source, &mut open_regions);
                }
                Ok(None) => {}
                Err(error) => directives.errors.push(DirectiveProblem::Malformed(error)),
            }
        }
        for region in open_regions {
            directives.cover(region.entry, region.from, FILE_END);
        }
        directives
    }

    fn add<'source>(
        &mut self,
        span: Span,
        directive: LintDirective,
        source: &'source [u8],
        open_regions: &mut Vec<OpenRegion<'source>>,
    ) {
        let LintDirective { verb, extent, list } = directive;
        let index = self.list.len();
        let first = self.entries.len();
        for name in directive.rules(source) {
            let target = match name.text(source) {
                b"*" => Target::Every,
                text => RULES
                    .iter()
                    .position(|entry| entry.rule().name().as_bytes() == text)
                    .map_or(Target::Unknown, Target::Rule),
            };
            if verb != LintVerb::Allow {
                self.enable(target);
            }
            self.entries.push(Entry { target, name, directive: index, end: None });
        }
        let entries = first..self.entries.len();
        self.list.push(Directive { span, verb, extent, list, entries: entries.clone() });
        for entry in entries {
            let Some(name) = self.entries.get(entry).map(|found| found.name) else {
                continue;
            };
            let name_text = name.text(source);
            match extent {
                LintExtent::File => self.cover(entry, 0, FILE_END),
                LintExtent::Statement => {}
                LintExtent::RegionStart => {
                    open_regions.push(OpenRegion { entry, name: name_text, verb, from: span.end });
                }
                LintExtent::RegionEnd => {
                    let opened = open_regions
                        .iter()
                        .rposition(|region| region.name == name_text && region.verb == verb);
                    match opened.map(|position| open_regions.remove(position)) {
                        Some(region) => {
                            if let Some(start) = self.entries.get_mut(region.entry) {
                                start.end = Some(entry);
                            }
                            self.cover(region.entry, region.from, span.start);
                        }
                        None => self.errors.push(DirectiveProblem::UnopenedEnd { verb, name }),
                    }
                }
            }
        }
    }

    fn enable(&mut self, target: Target) {
        if target != Target::Unknown && self.enabled.is_empty() {
            self.enabled = vec![false; RULES.len()];
        }
        match target {
            Target::Rule(slot) => {
                if let Some(is_enabled) = self.enabled.get_mut(slot) {
                    *is_enabled = true;
                }
            }
            Target::Every => self.enabled.fill(true),
            Target::Unknown => {}
        }
    }

    fn cover(&mut self, entry: usize, start: u32, end: u32) {
        let Some(found) = self.entries.get(entry) else {
            return;
        };
        let Some(verb) = self.list.get(found.directive).map(|directive| directive.verb) else {
            return;
        };
        if found.target != Target::Unknown {
            self.intervals.push(Interval { target: found.target, start, end, entry, verb });
        }
    }

    /// Finishes the table once the node table is built.
    ///
    /// Each directive on a statement covers the one it trails on its line, or
    /// else the next to start, or else the one it's inside. A statement that
    /// isn't inside the innermost one around the directive is neither of the
    /// first two, so a directive never reaches past the statement it's
    /// written in.
    pub(crate) fn cover_statements(
        &mut self,
        nodes: &Nodes<'_>,
        source: &[u8],
        version: LuaVersion,
    ) {
        if self.list.iter().any(|directive| directive.extent == LintExtent::Statement) {
            let mut stmts: Vec<Span> = nodes
                .as_slice()
                .iter()
                .filter_map(|node| match node.kind {
                    NodeKind::Statement(Statement::Error(_))
                    | NodeKind::Expression(_)
                    | NodeKind::Table(_)
                    | NodeKind::Chain(..)
                    | NodeKind::Call { .. } => None,
                    NodeKind::Statement(stmt) => Some(stmt.span()),
                })
                .collect();
            stmts.sort_unstable_by_key(|stmt| stmt.start);
            let mut remaining = stmts.into_iter().peekable();
            // The statements around the directive, innermost last, and the
            // one that ended last before it.
            let mut open: Vec<Span> = Vec::new();
            let mut last_closed: Option<Span> = None;
            let mut covered: Vec<(Range<usize>, Span)> = Vec::new();
            for directive in &self.list {
                if directive.extent != LintExtent::Statement {
                    continue;
                }
                let at = directive.span.start;
                while let Some(stmt) = remaining.next_if(|stmt| stmt.start < at) {
                    close_before(&mut open, &mut last_closed, stmt.start);
                    open.push(stmt);
                }
                close_before(&mut open, &mut last_closed, at);
                let around = open.last().copied();
                let is_inside = |stmt: &Span| {
                    around.is_none_or(|outer| (outer.start..outer.end).contains(&stmt.start))
                };
                let trailed = last_closed.filter(|stmt| {
                    is_inside(stmt)
                        && !has_line_break(Span::new(stmt.end, at).text(source), version)
                });
                let next = remaining.peek().copied().filter(is_inside);
                if let Some(stmt) = trailed.or(next).or(around) {
                    covered.push((directive.entries.clone(), stmt));
                }
            }
            for (entries, stmt) in covered {
                for entry in entries {
                    self.cover(entry, stmt.start, stmt.end);
                }
            }
        }
        self.intervals.sort_unstable_by_key(|interval| (interval.target, interval.rank()));
    }

    pub(crate) fn directives(&self) -> &[Directive] {
        &self.list
    }

    pub(crate) fn entries(&self) -> &[Entry] {
        &self.entries
    }

    pub(crate) fn errors(&self) -> &[DirectiveProblem] {
        &self.errors
    }

    /// Whether a `deny` or `warn` names the rule at `slot`, or `*`.
    pub(crate) fn enables(&self, slot: usize) -> bool {
        self.enabled.get(slot).copied().unwrap_or(false)
    }

    /// The walk over the ranges that can cover the diagnostics of the rule at
    /// `slot`.
    pub(crate) fn cover_of(&self, slot: usize) -> Cover<'_> {
        Cover {
            own: self.intervals_of(Target::Rule(slot)),
            every: self.intervals_of(Target::Every),
            open: Vec::new(),
        }
    }

    fn intervals_of(&self, target: Target) -> &[Interval] {
        let first = self.intervals.partition_point(|interval| interval.target < target);
        let end = self.intervals.partition_point(|interval| interval.target <= target);
        self.intervals.get(first..end).unwrap_or_default()
    }
}

/// A region's `start` that no `end` has closed yet.
struct OpenRegion<'source> {
    entry: usize,
    name: &'source [u8],
    verb: LintVerb,
    /// Where it starts covering.
    from: u32,
}

fn close_before(open: &mut Vec<Span>, last_closed: &mut Option<Span>, at: u32) {
    while let Some(&stmt) = open.last()
        && stmt.end <= at
    {
        open.pop();
        *last_closed = Some(stmt);
    }
}

/// One rule's walk over the ranges that can cover its diagnostics, which it
/// asks about in source order.
#[derive(Debug)]
pub(crate) struct Cover<'directives> {
    own: &'directives [Interval],
    every: &'directives [Interval],
    /// The ranges begun so far, with the winner on top once those that ended
    /// are popped, since each begins with a greater rank than all before it.
    open: Vec<Interval>,
}

impl Cover<'_> {
    pub(crate) const fn is_empty(&self) -> bool {
        self.own.is_empty() && self.every.is_empty() && self.open.is_empty()
    }

    /// The verb and entry of the directive that wins at `offset`, which is
    /// never before the offset asked about last.
    pub(crate) fn at(&mut self, offset: u32) -> Option<(LintVerb, usize)> {
        loop {
            let ranges = match (self.own.first(), self.every.first()) {
                (Some(own), Some(every)) if every.rank() < own.rank() => &mut self.every,
                (Some(_), _) => &mut self.own,
                (None, Some(_)) => &mut self.every,
                (None, None) => break,
            };
            let Some((&next, rest)) =
                ranges.split_first().filter(|&(next, _)| next.start <= offset)
            else {
                break;
            };
            *ranges = rest;
            self.open.push(next);
        }
        while self.open.last().is_some_and(|interval| interval.end <= offset) {
            self.open.pop();
        }
        self.open.last().map(|interval| (interval.verb, interval.entry))
    }
}

#[cfg(test)]
mod tests {
    use luck_allocator::Allocator;
    use luck_token::StdlibEnvironment;

    use super::*;
    use crate::config::LintConfig;
    use crate::driver::analyze;

    /// What the directives in `source` set at each offset `needles` start
    /// at, for `rule`.
    fn verbs_at(source: &str, rule: &str, needles: &[&str]) -> Vec<Option<LintVerb>> {
        let allocator = Allocator::new();
        let parse = luck_parser::parse(&allocator, source.as_bytes(), LuaVersion::Lua54);
        assert!(parse.errors.is_empty(), "{:?}", parse.errors);
        let analysis = analyze(&parse, StdlibEnvironment::Standalone, &LintConfig::default());
        let slot = RULES.iter().position(|entry| entry.rule().name() == rule).expect("a rule");
        let mut cover = analysis.directives().cover_of(slot);
        let mut offsets: Vec<u32> = needles
            .iter()
            .map(|needle| u32::try_from(source.find(needle).expect("in the source")).expect("fits"))
            .collect();
        offsets.sort_unstable();
        offsets.into_iter().map(|offset| cover.at(offset).map(|(verb, _)| verb)).collect()
    }

    #[test]
    fn a_statement_directive_covers_the_statement_it_trails() {
        let source = "\
local a = 1 local b = 2 -- luck: allow(shadowing)
local c = 3";
        assert_eq!(
            verbs_at(source, "shadowing", &["a =", "b =", "c ="]),
            [None, Some(LintVerb::Allow), None]
        );
    }

    #[test]
    fn a_statement_directive_skips_a_separator() {
        let source = "local a = 1; -- luck: allow(shadowing)";
        assert_eq!(verbs_at(source, "shadowing", &["a ="]), [Some(LintVerb::Allow)]);
    }

    #[test]
    fn a_statement_directive_covers_the_next_statement_inside_its_own() {
        let source = "\
local function f()
  -- luck: deny(shadowing)
  local inner = 1
end
local after = 2";
        assert_eq!(
            verbs_at(source, "shadowing", &["function", "inner", "after"]),
            [None, Some(LintVerb::Deny), None]
        );
    }

    #[test]
    fn a_statement_directive_inside_a_statement_with_none_after_covers_that_one() {
        let source = "\
local t = {
  a = 1,
  a = 2, -- luck: allow(duplicate_keys)
}
local after = 2";
        assert_eq!(
            verbs_at(source, "duplicate_keys", &["t =", "a = 2", "after"]),
            [Some(LintVerb::Allow), Some(LintVerb::Allow), None]
        );
    }

    #[test]
    fn a_statement_directive_at_the_end_of_a_block_stays_in_it() {
        let source = "\
do
  local a = 1
  -- luck: allow(shadowing)
end
local after = 2";
        assert_eq!(verbs_at(source, "shadowing", &["do", "after"]), [Some(LintVerb::Allow), None]);
    }

    #[test]
    fn the_range_that_starts_last_wins_and_then_the_narrowest() {
        let source = "\
-- luck: allow(shadowing) start
local a = 1
-- luck: deny(shadowing) start
local b = 2
-- luck: allow(shadowing) end
local c = 3
-- luck: warn(shadowing)
local d = 4
-- luck: deny(shadowing) end
local e = 5
-- #luck: warn(shadowing)";
        assert_eq!(
            verbs_at(source, "shadowing", &["a =", "b =", "c =", "d =", "e ="]),
            [
                Some(LintVerb::Allow),
                Some(LintVerb::Deny),
                Some(LintVerb::Deny),
                Some(LintVerb::Warn),
                Some(LintVerb::Warn),
            ]
        );
    }

    #[test]
    fn a_wildcard_competes_with_the_rule_s_own_entries() {
        let source = "\
-- luck: deny(*) start
local a = 1
-- luck: allow(shadowing)
local b = 2
local c = 3";
        assert_eq!(
            verbs_at(source, "shadowing", &["a =", "b =", "c ="]),
            [Some(LintVerb::Deny), Some(LintVerb::Allow), Some(LintVerb::Deny)]
        );
        assert_eq!(verbs_at(source, "empty_block", &["b ="]), [Some(LintVerb::Deny)]);
    }

    fn directives_of(source: &str) -> Directives {
        let allocator = Allocator::new();
        let parse = luck_parser::parse(&allocator, source.as_bytes(), LuaVersion::Lua54);
        Directives::new(&parse.comments, source.as_bytes())
    }

    #[test]
    fn deny_and_warn_enable_what_they_name() {
        let directives = directives_of(
            "\
-- luck: warn(shadowing)
-- luck: allow(empty_block)
local a = 1",
        );
        let slot = |rule: &str| RULES.iter().position(|entry| entry.rule().name() == rule);
        assert!(slot("shadowing").is_some_and(|found| directives.enables(found)));
        assert!(slot("empty_block").is_some_and(|found| !directives.enables(found)));
        let every = directives_of("-- luck: deny(*)");
        assert!((0..RULES.len()).all(|found| every.enables(found)));
    }

    #[test]
    fn an_end_closes_the_latest_region_of_its_verb_and_rule() {
        let source = "\
-- luck: allow(shadowing) start
-- luck: deny(shadowing) start
-- luck: allow(shadowing, unused_variable) end
local a = 1";
        let directives = directives_of(source);
        let ends: Vec<Option<usize>> = directives.entries().iter().map(|entry| entry.end).collect();
        assert_eq!(ends, [Some(2), None, None, None]);
        let Some(&DirectiveProblem::UnopenedEnd { verb, name }) = directives.errors().first()
        else {
            panic!("{:?}", directives.errors());
        };
        assert_eq!(directives.errors().len(), 1, "{:?}", directives.errors());
        assert_eq!(
            (verb, name.text(source.as_bytes())),
            (LintVerb::Allow, &b"unused_variable"[..])
        );
    }
}
