//! Each stage's results over the suite, summarized and listed the way oxc's
//! `tasks/coverage` snapshots them.

use std::collections::BTreeMap;
use std::fmt::{self, Write as _};
use std::path::Path;
use std::{fs, io};

/// How a stage took one file.
///
/// Only the parser checks a file the reference interpreter rejects, and it
/// ends in [`Self::IncorrectlyPassed`] or [`Self::CorrectError`], which
/// nothing else ends in.
#[derive(Debug, Clone)]
pub(crate) enum TestResult {
    Passed,
    /// luck parsed a file the reference interpreter rejects.
    IncorrectlyPassed,
    /// Every error, one per line.
    ParseError(String),
    /// luck rejected a file the reference interpreter rejects too, with its
    /// first error.
    CorrectError(String),
    Mismatch {
        check: &'static str,
        actual: String,
        expected: String,
    },
    /// A check and what went wrong.
    GenericError(&'static str, String),
}

impl TestResult {
    /// Whether it's of a file the reference interpreter rejects, which the
    /// summary counts apart.
    const fn is_negative(&self) -> bool {
        match self {
            Self::IncorrectlyPassed | Self::CorrectError(_) => true,
            Self::Passed | Self::ParseError(_) | Self::Mismatch { .. } | Self::GenericError(..) => {
                false
            }
        }
    }

    const fn has_passed(&self) -> bool {
        match self {
            Self::Passed | Self::CorrectError(_) => true,
            Self::IncorrectlyPassed
            | Self::ParseError(_)
            | Self::Mismatch { .. }
            | Self::GenericError(..) => false,
        }
    }

    const fn has_parsed(&self) -> bool {
        match self {
            Self::Passed
            | Self::IncorrectlyPassed
            | Self::Mismatch { .. }
            | Self::GenericError(..) => true,
            Self::ParseError(_) | Self::CorrectError(_) => false,
        }
    }
}

/// A stage's result on the file at `path`.
#[derive(Debug, Clone, Copy)]
pub(crate) struct CoverageResult<'a> {
    pub path: &'a str,
    pub result: &'a TestResult,
}

/// A stage's snapshot of the suite: the summary the console shows, then the
/// details.
#[derive(Debug)]
pub(crate) struct Snapshot {
    /// The stage, such as `parser`, which names the file.
    name: &'static str,
    pub summary: String,
    details: String,
}

impl Snapshot {
    /// Writes `snapshots/<name>.snap` beside this crate's `Cargo.toml`.
    pub(crate) fn save(&self) -> io::Result<()> {
        let directory = Path::new(env!("CARGO_MANIFEST_DIR")).join("snapshots");
        fs::create_dir_all(&directory)?;
        fs::write(directory.join(format!("{}.snap", self.name)), self.to_string())
    }
}

impl fmt::Display for Snapshot {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}\n{}", self.summary, self.details)
    }
}

/// A stage's snapshot: how many of the files the reference interpreter
/// compiles luck parsed and passed, and how many of the ones it rejects luck
/// rejected, then the [`failures`].
pub(crate) fn coverage(name: &'static str, results: &[CoverageResult<'_>]) -> Snapshot {
    let (negatives, positives): (Vec<&TestResult>, Vec<&TestResult>) =
        results.iter().map(|coverage| coverage.result).partition(|result| result.is_negative());
    let mut summary = format!("{name} Summary:\n");
    let mut tally = |label: &str, group: &[&TestResult], is_counted: fn(&TestResult) -> bool| {
        let count = group.iter().filter(|result| is_counted(result)).count();
        let all = group.len();
        let _ = writeln!(summary, "{label}: {count}/{all} ({:.2}%)", percent(count, all));
    };
    tally("AST Parsed     ", &positives, TestResult::has_parsed);
    tally("Positive Passed", &positives, TestResult::has_passed);
    if !negatives.is_empty() {
        tally("Negative Passed", &negatives, TestResult::has_passed);
    }
    Snapshot { name, summary, details: failures(results) }
}

#[expect(clippy::cast_precision_loss, reason = "f64 rounds only a count past 2^53")]
fn percent(count: usize, all: usize) -> f64 {
    count as f64 / all.max(1) as f64 * 100.0
}

/// Every result that didn't pass, then every file luck rejected as it should
/// have, with its first error, each in the order given, which is path order.
fn failures(results: &[CoverageResult<'_>]) -> String {
    let mut listed = String::new();
    for &CoverageResult { path, result } in results {
        let (label, detail) = match result {
            TestResult::Passed | TestResult::CorrectError(_) => continue,
            TestResult::IncorrectlyPassed => ("Expect Syntax Error: ", None),
            TestResult::ParseError(errors) => ("Expect to Parse: ", Some(errors)),
            TestResult::Mismatch { check, .. } => {
                listed.push_str(check);
                (": ", None)
            }
            TestResult::GenericError(check, error) => {
                listed.push_str(check);
                (" Error: ", Some(error))
            }
        };
        listed.push_str(label);
        listed.push_str(path);
        listed.push('\n');
        if let Some(text) = detail {
            listed.push_str(text);
            listed.push('\n');
        }
        listed.push('\n');
    }
    for &CoverageResult { path, result } in results {
        if let TestResult::CorrectError(error) = result {
            listed.push_str("Correct Error: ");
            listed.push_str(path);
            listed.push('\n');
            listed.push_str(error);
            listed.push_str("\n\n");
        }
    }
    listed
}

/// A diagnostic as the linter's snapshot lists it.
#[derive(Debug)]
pub(crate) struct LintLine {
    pub rule: &'static str,
    /// `line:column rule: message`.
    pub text: String,
}

/// The linter's snapshot: how many files it ran on and each rule's count, then
/// the files it couldn't run on, and every diagnostic of the others, by file in
/// the order given.
pub(crate) fn lint(
    name: &'static str,
    files: &[(&str, &Result<Vec<LintLine>, TestResult>)],
) -> Snapshot {
    let mut counts: BTreeMap<&str, usize> = BTreeMap::new();
    let mut not_linted = Vec::new();
    let mut listed = String::new();
    for &(path, linted) in files {
        match linted {
            Err(result) => not_linted.push(CoverageResult { path, result }),
            Ok(diagnostics) if diagnostics.is_empty() => {}
            Ok(diagnostics) => {
                let _ = writeln!(listed, "{path}");
                for LintLine { rule, text } in diagnostics {
                    *counts.entry(rule).or_default() += 1;
                    let _ = writeln!(listed, "{text}");
                }
                listed.push('\n');
            }
        }
    }
    let mut summary = format!("{name} Summary:\n");
    let _ =
        writeln!(summary, "Files Linted   : {}/{}", files.len() - not_linted.len(), files.len());
    let _ = writeln!(summary, "Diagnostics    : {}", counts.values().sum::<usize>());
    let mut by_count: Vec<(&str, usize)> = counts.into_iter().collect();
    by_count.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(b.0)));
    for (rule, count) in by_count {
        let _ = writeln!(summary, "{count:>6} {rule}");
    }
    Snapshot { name, summary, details: format!("{}{listed}", failures(&not_linted)) }
}
