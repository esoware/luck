//! Matching records every (step, position) pair it has been at, so a section
//! costs at most program size times path length, however its alternatives
//! nest.

use core::mem;

/// Each runs and then falls through to the next, unless it says otherwise.
#[derive(Debug)]
enum Step {
    Literal(char),
    /// Matches `?`.
    AnyCharacter,
    /// Matches `*`, or `**` when it crosses `/`. Either consumes one character
    /// and stays put, or falls through.
    AnySequence {
        can_cross_separators: bool,
    },
    /// Matches `[seq]` or `[!seq]`.
    Class {
        ranges: Box<[(char, char)]>,
        is_negated: bool,
    },
    /// Matches `{num1..num2}`, with its bounds in order.
    Integer {
        low: i64,
        high: i64,
    },
    /// The start of every alternative of `{s1,s2}`.
    Branch(Box<[usize]>),
    Jump(usize),
    Accept,
}

/// An opening `{` not closed yet. Its step and its commas stay literal unless
/// a `}` turns them into alternation.
struct OpenBrace {
    brace_step: usize,
    comma_steps: Vec<usize>,
}

#[derive(Debug)]
pub(super) struct SectionGlob {
    steps: Vec<Step>,
}

impl SectionGlob {
    /// Subjects start with `/`, so a name holding one is anchored to the file's
    /// directory and any other matches at any depth.
    pub(super) fn new(section_name: &str) -> Self {
        let prefix = if !section_name.contains('/') {
            "/**/"
        } else if section_name.starts_with('/') {
            ""
        } else {
            "/"
        };
        let pattern: Vec<char> = prefix.chars().chain(section_name.chars()).collect();

        let mut steps = Vec::with_capacity(pattern.len().saturating_add(1));
        let mut open_braces: Vec<OpenBrace> = Vec::new();
        let mut rest = pattern.as_slice();
        while let Some((&character, after_character)) = rest.split_first() {
            rest = after_character;
            match character {
                '\\' => {
                    if let Some((&escaped, after_escape)) = rest.split_first() {
                        steps.push(Step::Literal(escaped));
                        rest = after_escape;
                    }
                }
                '/' => {
                    steps.push(Step::Literal('/'));
                    // `/**/` also matches a lone `/`.
                    if let Some(after_stars) = rest.strip_prefix(['*', '*', '/'].as_slice()) {
                        rest = after_stars;
                        let branch_step = steps.len();
                        steps.push(Step::Branch(Box::new([
                            branch_step.saturating_add(1),
                            branch_step.saturating_add(3),
                        ])));
                        steps.push(Step::AnySequence { can_cross_separators: true });
                        steps.push(Step::Literal('/'));
                    }
                }
                '*' => {
                    let can_cross_separators = rest.first() == Some(&'*');
                    if can_cross_separators {
                        rest = rest.get(1..).unwrap_or_default();
                    }
                    steps.push(Step::AnySequence { can_cross_separators });
                }
                '?' => steps.push(Step::AnyCharacter),
                '[' => match parse_class(rest) {
                    Some((class, after_class)) => {
                        steps.push(class);
                        rest = after_class;
                    }
                    None => steps.push(Step::Literal('[')),
                },
                '{' => {
                    if let Some((first, second, after_range)) = parse_integer_range(rest) {
                        steps.push(Step::Integer {
                            low: first.min(second),
                            high: first.max(second),
                        });
                        rest = after_range;
                    } else {
                        open_braces
                            .push(OpenBrace { brace_step: steps.len(), comma_steps: Vec::new() });
                        steps.push(Step::Literal('{'));
                    }
                }
                ',' => {
                    if let Some(open_brace) = open_braces.last_mut() {
                        open_brace.comma_steps.push(steps.len());
                    }
                    steps.push(Step::Literal(','));
                }
                '}' => match open_braces.pop() {
                    Some(open_brace) if !open_brace.comma_steps.is_empty() => {
                        let end = steps.len();
                        let mut alternatives = vec![open_brace.brace_step.saturating_add(1)];
                        for comma_step in open_brace.comma_steps {
                            alternatives.push(comma_step.saturating_add(1));
                            if let Some(step) = steps.get_mut(comma_step) {
                                *step = Step::Jump(end);
                            }
                        }
                        if let Some(step) = steps.get_mut(open_brace.brace_step) {
                            *step = Step::Branch(alternatives.into_boxed_slice());
                        }
                    }
                    _ => steps.push(Step::Literal('}')),
                },
                literal => steps.push(Step::Literal(literal)),
            }
        }
        steps.push(Step::Accept);
        Self { steps }
    }

    /// Whether the whole of `subject`, a `/`-rooted path relative to the
    /// section's `.editorconfig`, matches.
    pub(super) fn matches(&self, subject: &[char]) -> bool {
        let position_count = subject.len().saturating_add(1);
        let mut visited = vec![false; self.steps.len().saturating_mul(position_count)];
        let mut pending = vec![(0usize, 0usize)];
        while let Some((step_index, position)) = pending.pop() {
            let visit_slot = step_index.saturating_mul(position_count).saturating_add(position);
            if visited.get_mut(visit_slot).is_none_or(|is_visited| mem::replace(is_visited, true)) {
                continue;
            }
            let Some(step) = self.steps.get(step_index) else {
                continue;
            };
            let current = subject.get(position).copied();
            let next_step = step_index.saturating_add(1);
            let advanced = (next_step, position.saturating_add(1));
            match step {
                Step::Literal(expected) => {
                    if current == Some(*expected) {
                        pending.push(advanced);
                    }
                }
                Step::AnyCharacter => {
                    if current.is_some_and(|c| c != '/') {
                        pending.push(advanced);
                    }
                }
                Step::AnySequence { can_cross_separators } => {
                    pending.push((next_step, position));
                    if current.is_some_and(|c| *can_cross_separators || c != '/') {
                        pending.push((step_index, position.saturating_add(1)));
                    }
                }
                Step::Class { ranges, is_negated } => {
                    let is_in_class =
                        |c: char| ranges.iter().any(|(low, high)| (*low..=*high).contains(&c));
                    if current.is_some_and(|c| is_in_class(c) != *is_negated) {
                        pending.push(advanced);
                    }
                }
                Step::Integer { low, high } => {
                    for end in integer_ends(subject, position, *low, *high) {
                        pending.push((next_step, end));
                    }
                }
                Step::Branch(targets) => {
                    pending.extend(targets.iter().map(|target| (*target, position)));
                }
                Step::Jump(target) => pending.push((*target, position)),
                Step::Accept => {
                    if current.is_none() {
                        return true;
                    }
                }
            }
        }
        false
    }
}

/// The class after a `[`, with what follows it. `None` when the `[` is
/// literal: unclosed, holding a `/`, or empty.
fn parse_class(rest: &[char]) -> Option<(Step, &[char])> {
    let (is_negated, mut remaining) =
        rest.strip_prefix(['!'].as_slice()).map_or((false, rest), |after_bang| (true, after_bang));
    let mut ranges = Vec::new();
    loop {
        let (&character, after_character) = remaining.split_first()?;
        remaining = after_character;
        let low = match character {
            ']' => break,
            '/' => return None,
            '\\' => {
                let (&escaped, after_escape) = remaining.split_first()?;
                remaining = after_escape;
                escaped
            }
            plain => plain,
        };
        let mut high = low;
        if let ['-', after_hyphen, after_upper @ ..] = remaining
            && *after_hyphen != ']'
        {
            high = *after_hyphen;
            remaining = after_upper;
            if high == '\\' {
                let (&escaped, after_escape) = remaining.split_first()?;
                high = escaped;
                remaining = after_escape;
            }
        }
        ranges.push((low, high));
    }
    if ranges.is_empty() && !is_negated {
        return None;
    }
    Some((Step::Class { ranges: ranges.into_boxed_slice(), is_negated }, remaining))
}

/// The `num1..num2}` after a `{`, in written order, with what follows it.
fn parse_integer_range(rest: &[char]) -> Option<(i64, i64, &[char])> {
    let body_length = rest.iter().position(|c| !matches!(c, '0'..='9' | '.' | '-'))?;
    let (body, closer) = rest.split_at_checked(body_length)?;
    let after_range = closer.strip_prefix(['}'].as_slice())?;
    let body_text: String = body.iter().collect();
    let (first, second) = body_text.split_once("..")?;
    Some((first.parse().ok()?, second.parse().ok()?, after_range))
}

/// Where each integer in `low..=high` starting at `position` ends. A leading
/// zero is only the integer `0` itself.
fn integer_ends(subject: &[char], position: usize, low: i64, high: i64) -> Vec<usize> {
    let tail = subject.get(position..).unwrap_or_default();
    let (is_negative, digits) =
        tail.strip_prefix(['-'].as_slice()).map_or((false, tail), |after_sign| (true, after_sign));
    let digits_start = position.saturating_add(usize::from(is_negative));
    let mut ends = Vec::new();
    let mut magnitude: i64 = 0;
    for (digit_index, character) in digits.iter().enumerate() {
        let Some(digit) = character.to_digit(10) else {
            break;
        };
        if digit_index > 0 && digits.first() == Some(&'0') {
            break;
        }
        magnitude = magnitude.saturating_mul(10).saturating_add(i64::from(digit));
        let value = if is_negative { magnitude.saturating_neg() } else { magnitude };
        if (low..=high).contains(&value) {
            ends.push(digits_start.saturating_add(digit_index).saturating_add(1));
        }
    }
    ends
}

#[cfg(test)]
mod tests {
    use super::*;

    fn glob_matches(section_name: &str, path: &str) -> bool {
        let subject: Vec<char> = format!("/{path}").chars().collect();
        SectionGlob::new(section_name).matches(&subject)
    }

    #[test]
    fn a_name_without_a_separator_matches_at_any_depth() {
        assert!(glob_matches("*.lua", "main.lua"));
        assert!(glob_matches("*.lua", "src/deep/main.lua"));
        assert!(!glob_matches("*.lua", "main.luau"));
    }

    #[test]
    fn a_name_with_a_separator_is_anchored() {
        for name in ["src/*.lua", "/src/*.lua"] {
            assert!(glob_matches(name, "src/a.lua"), "{name}");
            assert!(!glob_matches(name, "other/src/a.lua"), "{name}");
            assert!(!glob_matches(name, "src/nested/a.lua"), "{name}");
        }
    }

    #[test]
    fn star_star_crosses_separators_and_slash_star_star_slash_may_be_empty() {
        assert!(glob_matches("a**z.c", "a/m/z.c"));
        assert!(glob_matches("d/**/z.c", "d/z.c"));
        assert!(glob_matches("d/**/z.c", "d/m/n/z.c"));
        assert!(!glob_matches("d/**/z.c", "dm/z.c"));
        assert!(!glob_matches("d/**/z.c", "d/mz.c"));
    }

    #[test]
    fn question_mark_stays_inside_one_path_component() {
        assert!(glob_matches("a?c", "abc"));
        assert!(!glob_matches("src/a?c", "src/a/c"));
    }

    #[test]
    fn classes_support_ranges_negation_and_escapes() {
        assert!(glob_matches("[a-c].lua", "b.lua"));
        assert!(!glob_matches("[a-c].lua", "d.lua"));
        assert!(glob_matches("[!a-c].lua", "d.lua"));
        assert!(glob_matches(r"[\]].lua", "].lua"));
        assert!(glob_matches("[a-].lua", "-.lua"));
    }

    #[test]
    fn a_bracket_that_does_not_close_or_holds_a_separator_is_literal() {
        assert!(glob_matches("[a.lua", "[a.lua"));
        assert!(glob_matches("ab[e/]cd", "ab[e/]cd"));
        assert!(!glob_matches("ab[e/]cd", "abecd"));
    }

    #[test]
    fn alternatives_may_nest_and_each_alternative_is_tried() {
        assert!(glob_matches("{a,{b,c}}.lua", "c.lua"));
        assert!(glob_matches("{a*b,xb}.lua", "xb.lua"));
        assert!(glob_matches("{a*b,xb}.lua", "ab.lua"));
        assert!(glob_matches("{,a}.lua", ".lua"));
    }

    #[test]
    fn braces_without_a_comma_or_a_closer_are_literal() {
        assert!(glob_matches("{a}.lua", "{a}.lua"));
        assert!(!glob_matches("{a}.lua", "a.lua"));
        assert!(glob_matches("{a,b.lua", "{a,b.lua"));
        assert!(glob_matches("a,b}", "a,b}"));
    }

    #[test]
    fn integer_ranges_reject_leading_zeros_and_order_their_bounds() {
        assert!(glob_matches("{3..120}", "15"));
        assert!(glob_matches("{120..3}", "15"));
        assert!(!glob_matches("{3..120}", "121"));
        assert!(!glob_matches("{3..120}", "060"));
        assert!(glob_matches("{-3..3}", "-2"));
        assert!(glob_matches("{0..5}", "0"));
    }

    #[test]
    fn backslash_escapes_a_special_character() {
        assert!(glob_matches(r"\*.lua", "*.lua"));
        assert!(!glob_matches(r"\*.lua", "a.lua"));
    }

    #[test]
    fn exponential_alternation_finishes() {
        let name = format!("b{}.lua", "{a,aa}".repeat(40));
        assert!(!glob_matches(&name, &format!("{}.lua", "a".repeat(60))));
    }
}
