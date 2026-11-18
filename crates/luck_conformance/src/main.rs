//! # `luck_conformance`
//!
//! Runs the official test suite of the dialect this binary's interpreter
//! speaks through every stage, and writes what it finds to the committed
//! snapshots in `snapshots/` beside this crate's `Cargo.toml`, one per stage:
//!
//! - `parser`: luck parses every file the reference interpreter compiles, and
//!   rejects every file it rejects.
//! - `codegen`: the compact output parses back to the same tree and prints as
//!   itself.
//! - `formatter`: the formatted output parses back to the same tree and
//!   formats as itself.
//! - `minifier`: the minified output parses and minifies as itself.
//! - `runtime`: the original, formatted, and minified programs end the same
//!   way in the reference interpreter.
//! - `linter`: every rule's diagnostics.
//!
//! The reference interpreter is mlua's vendored build of the dialect this
//! crate's one feature picks. Ported from oxc's `tasks/coverage`.

extern crate alloc;

mod execute;
mod runtime;
mod snapshot;
mod status;
mod suite;
mod tools;

use std::path::Path;
use std::{env, io};

use luck_ast::MIN_STACK_BYTES;
use rayon::ThreadPoolBuilder;
use rayon::prelude::*;
use similar::TextDiff;

use crate::snapshot::{CoverageResult, LintLine, TestResult};
use crate::suite::Suite;
use crate::tools::FileReport;

/// With `--filter` and part of a path, checks only the files whose path holds
/// it, prints each failure in full, and writes no snapshot.
///
/// # Errors
///
/// Fails on any other arguments, when the suite can't be fetched or read, or
/// when a snapshot can't be written.
fn main() -> io::Result<()> {
    let suite = Suite::linked()?;
    execute::start(suite)?;
    match env::args().skip(1).collect::<Vec<_>>().as_slice() {
        [mode, chunk_name, verdict_path] if mode == execute::MODE => {
            execute::run(suite, chunk_name, Path::new(verdict_path))
        }
        [] => run(suite, None),
        [flag, filter] if flag == "--filter" => run(suite, Some(filter)),
        _ => Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "expected no arguments, or `--filter` and part of a file's path",
        )),
    }
}

type StageResult = fn(&FileReport) -> Option<&TestResult>;

/// # Errors
///
/// Fails when the suite can't be fetched or read, or a snapshot can't be
/// written.
fn run(suite: &'static Suite, filter: Option<&str>) -> io::Result<()> {
    let root = suite.fetch()?;
    let files = suite.files(&root, filter)?;
    let pool =
        ThreadPoolBuilder::new().stack_size(MIN_STACK_BYTES).build().map_err(io::Error::other)?;
    let reports: Vec<FileReport> =
        pool.install(|| files.par_iter().map(|file| tools::check(suite, &root, file)).collect());

    let stages: [(&str, StageResult); 5] = [
        ("parser", |report| Some(&report.parser)),
        ("codegen", |report| report.codegen.as_ref()),
        ("formatter", |report| report.formatter.as_ref()),
        ("minifier", |report| report.minifier.as_ref()),
        ("runtime", |report| report.runtime.as_ref()),
    ];
    for (stage, result_of) in stages {
        let name = format!("{stage}_{}", suite.name);
        let results: Vec<CoverageResult> = files
            .iter()
            .zip(&reports)
            .filter_map(|(file, report)| {
                result_of(report).map(|result| CoverageResult {
                    path: file.path.clone(),
                    should_fail: report.should_fail,
                    result: result.clone(),
                })
            })
            .collect();
        let summary = snapshot::summary(&name, &results)?;
        let failures = snapshot::failures(&results);
        report(&name, &summary, &failures, filter.is_some().then_some(results.as_slice()))?;
    }

    let name = format!("linter_{}", suite.name);
    let linted: Vec<(&str, &Result<Vec<LintLine>, TestResult>)> = files
        .iter()
        .zip(&reports)
        .filter_map(|(file, report)| report.linter.as_ref().map(|lint| (file.path.as_str(), lint)))
        .collect();
    let (summary, diagnostics) = snapshot::lint(&name, &linted)?;
    report(&name, &summary, &diagnostics, filter.is_some().then_some(&[]))
}

/// Prints a stage's summary, and with `filtered`, its failures and a diff for
/// each mismatch, or else writes its snapshot.
///
/// # Errors
///
/// Fails when the snapshot can't be written.
#[expect(clippy::print_stdout, reason = "the console report is this tool's output")]
fn report(
    name: &str,
    summary: &str,
    failures: &str,
    filtered: Option<&[CoverageResult]>,
) -> io::Result<()> {
    let Some(results) = filtered else {
        println!("{summary}");
        return snapshot::save(name, &format!("{summary}\n{failures}"));
    };
    println!("{summary}\n{failures}");
    for CoverageResult { path, result, .. } in results {
        if let TestResult::Mismatch(case, actual, expected) = result {
            let diff = TextDiff::from_lines(expected, actual);
            println!("{case}: {path}\n{}", diff.unified_diff().header("expected", "actual"));
        }
    }
    Ok(())
}
