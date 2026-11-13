//! Spans, comments, and trivia are ignored.
//!
//! Shape, names, and literal values have to match, apart from the rewrites the formatter is allowed
//! to make, such as dropping redundant parens or swapping quote style.
//!
//! A divergence starts with an empty path, and each level it unwinds through
//! prepends its own segment, so a comparison that matches builds no path.

mod expr;
mod function;
mod stmt;
mod types;

use core::ops::ControlFlow::{self, Break, Continue};

use luck_ast::{Block, Punctuated};
use luck_token::{LuaVersion, Token, decode_string};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AstDiff {
    /// Slash-separated path into the tree, e.g.
    /// `block/stmts[3]/FunctionDecl/body`.
    pub path: String,
    pub reason: String,
}

impl AstDiff {
    pub(crate) fn new(path: impl Into<String>, reason: impl Into<String>) -> Self {
        Self { path: path.into(), reason: reason.into() }
    }

    fn nest_within(&mut self, segment: &str) {
        self.path = if self.path.is_empty() {
            segment.to_owned()
        } else {
            format!("{segment}/{}", self.path)
        };
    }
}

/// A divergence travels boxed, so a comparison that matches moves one word
/// through each level of the tree instead of two strings.
type Check = ControlFlow<Box<AstDiff>>;

/// String literals compare by the bytes they decode to in that dialect.
///
/// # Errors
///
/// Returns the first point where the two blocks diverge.
pub fn blocks_equivalent(left: &Block, right: &Block, version: LuaVersion) -> Result<(), AstDiff> {
    match at("block", stmt::block_eq(left, right, version)) {
        Continue(()) => Ok(()),
        Break(diff) => Err(*diff),
    }
}

fn differ(reason: impl Into<String>) -> Check {
    Break(Box::new(AstDiff::new(String::new(), reason)))
}

fn at(segment: &str, check: Check) -> Check {
    check.map_break(|mut diff| {
        diff.nest_within(segment);
        diff
    })
}

fn at_index(name: &str, index: usize, check: Check) -> Check {
    check.map_break(|mut diff| {
        diff.nest_within(&format!("{name}[{index}]"));
        diff
    })
}

fn require(is_satisfied: bool, reason: &str) -> Check {
    if is_satisfied { Continue(()) } else { differ(reason) }
}

fn presence_eq<T>(
    left: Option<&T>,
    right: Option<&T>,
    presence_reason: &str,
    item_eq: impl FnOnce(&T, &T) -> Check,
) -> Check
where
    T: ?Sized,
{
    match (left, right) {
        (None, None) => Continue(()),
        (Some(lhs), Some(rhs)) => item_eq(lhs, rhs),
        (Some(_), None) | (None, Some(_)) => differ(presence_reason),
    }
}

fn punctuated_eq<T>(
    left: &Punctuated<T>,
    right: &Punctuated<T>,
    version: LuaVersion,
    item_eq: fn(&T, &T, LuaVersion) -> Check,
) -> Check {
    let left_count = left.len();
    let right_count = right.len();
    if left_count != right_count {
        return differ(format!("punctuated length differs: {left_count} vs {right_count}"));
    }
    for (i, (lhs, rhs)) in left.iter().zip(right.iter()).enumerate() {
        at_index("", i, item_eq(lhs, rhs, version))?;
    }
    Continue(())
}

fn token_text_eq(left: &Token, right: &Token) -> Check {
    // `TokenKind::Identifier` carries the name, so comparing kinds compares
    // spelling.
    require(left.kind == right.kind, "token kind differs")
}

/// Compare two raw string literals (quotes and escapes included) by the
/// bytes they decode to under `version`, so a quote-style swap (`'x'` ->
/// `"x"`, re-escaped as needed) still matches.
fn string_raw_eq(left_raw: &str, right_raw: &str, version: LuaVersion) -> bool {
    match (decode_string(left_raw, version), decode_string(right_raw, version)) {
        (Some(left_value), Some(right_value)) => left_value == right_value,
        // A literal the decoder doesn't model on either side falls back to
        // raw equality.
        _ => left_raw == right_raw,
    }
}
