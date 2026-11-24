//! Each stage's results over a suite, summarized and listed the way oxc's
//! `tasks/coverage` snapshots them.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::Path;
use std::{fs, io};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum TestResult {
    Passed,
    /// luck parsed a file the reference interpreter rejects.
    IncorrectlyPassed,
    /// Every error, one per line.
    ParseError(String),
    /// luck rejected a file the reference interpreter rejects too, with its
    /// first error.
    CorrectError(String),
    /// A check, what it got, and what it expected.
    Mismatch(&'static str, String, String),
    /// A check and what went wrong.
    GenericError(&'static str, String),
}

#[derive(Debug)]
pub(crate) struct CoverageResult {
    pub path: String,
    /// The reference interpreter rejects the file.
    pub should_fail: bool,
    pub result: TestResult,
}

impl CoverageResult {
    const fn passed(&self) -> bool {
        match self.result {
            TestResult::Passed => !self.should_fail,
            TestResult::CorrectError(_) => true,
            TestResult::IncorrectlyPassed
            | TestResult::ParseError(_)
            | TestResult::Mismatch(..)
            | TestResult::GenericError(..) => false,
        }
    }

    const fn parsed(&self) -> bool {
        match self.result {
            TestResult::Passed
            | TestResult::IncorrectlyPassed
            | TestResult::Mismatch(..)
            | TestResult::GenericError(..) => true,
            TestResult::ParseError(_) | TestResult::CorrectError(_) => false,
        }
    }
}

/// The summary that opens a snapshot and that the console shows: how many of
/// the files the reference interpreter compiles luck parsed and passed, and
/// how many of the ones it rejects luck rejected.
pub(crate) fn summary(name: &str, results: &[CoverageResult]) -> String {
    let (negatives, positives): (Vec<&CoverageResult>, Vec<&CoverageResult>) =
        results.iter().partition(|result| result.should_fail);
    let tally = |group: &[&CoverageResult], is_counted: fn(&CoverageResult) -> bool| {
        group.iter().filter(|result| is_counted(result)).count()
    };
    let mut summary = format!("{name} Summary:\n");
    let mut lines = vec![
        ("AST Parsed     ", tally(&positives, CoverageResult::parsed), positives.len()),
        ("Positive Passed", tally(&positives, CoverageResult::passed), positives.len()),
    ];
    if !negatives.is_empty() {
        lines.push(("Negative Passed", tally(&negatives, CoverageResult::passed), negatives.len()));
    }
    for (label, count, all) in lines {
        let _ = writeln!(summary, "{label}: {count}/{all} ({:.2}%)", percent(count, all));
    }
    summary
}

#[expect(clippy::cast_precision_loss, reason = "f64 rounds only a count past 2^53")]
fn percent(count: usize, all: usize) -> f64 {
    count as f64 / all.max(1) as f64 * 100.0
}

/// Every result that didn't pass, in path order, then every file luck
/// rejected as it should have, with its first error.
pub(crate) fn failures(results: &[CoverageResult]) -> String {
    let mut sorted: Vec<&CoverageResult> = results.iter().collect();
    sorted.sort_by(|a, b| a.path.cmp(&b.path));
    let mut listed = String::new();
    for CoverageResult { path, result, .. } in &sorted {
        let (label, detail) = match result {
            TestResult::Passed | TestResult::CorrectError(_) => continue,
            TestResult::IncorrectlyPassed => ("Expect Syntax Error: ", None),
            TestResult::ParseError(errors) => ("Expect to Parse: ", Some(errors)),
            TestResult::Mismatch(case, _actual, _expected) => {
                listed.push_str(case);
                (": ", None)
            }
            TestResult::GenericError(case, error) => {
                listed.push_str(case);
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
    for CoverageResult { path, result, .. } in &sorted {
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
/// path order.
pub(crate) fn lint(
    name: &str,
    files: &[(&str, &Result<Vec<LintLine>, TestResult>)],
) -> (String, String) {
    let mut counts: BTreeMap<&str, usize> = BTreeMap::new();
    let mut not_linted = Vec::new();
    let mut listed = String::new();
    for (path, linted) in files {
        match linted {
            Err(result) => not_linted.push(CoverageResult {
                path: (*path).to_owned(),
                should_fail: false,
                result: result.clone(),
            }),
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
    (summary, format!("{}{listed}", failures(&not_linted)))
}

/// Writes `snapshots/<name>.snap` beside this crate's `Cargo.toml`.
pub(crate) fn save(name: &str, content: &str) -> io::Result<()> {
    let directory = Path::new(env!("CARGO_MANIFEST_DIR")).join("snapshots");
    fs::create_dir_all(&directory)?;
    fs::write(directory.join(format!("{name}.snap")), content)
}
