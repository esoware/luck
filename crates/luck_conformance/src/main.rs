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

mod execute;
mod runtime;
mod snapshot;
mod stages;
mod status;
mod suite;

use std::path::Path;
use std::{env, io};

use luck_ast::MIN_STACK_BYTES;
use luck_linter::{CategoryLevels, LintConfig, Linter, RuleLevel};
use rayon::ThreadPoolBuilder;
use rayon::prelude::*;
use similar::TextDiff;

use crate::snapshot::{CoverageResult, LintLine, Snapshot, TestResult};
use crate::stages::FileReport;
use crate::suite::Suite;

/// With `--filter` and part of a path, checks only the files whose path holds
/// it, prints each failure in full, and writes no snapshot.
fn main() -> io::Result<()> {
    let suite = Suite::linked()?;
    execute::start(suite)?;
    match env::args().skip(1).collect::<Vec<_>>().as_slice() {
        [mode, chunk_name, verdict_path] if mode == execute::MODE => {
            execute::run(suite.runner, chunk_name, Path::new(verdict_path))
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

fn run(suite: &Suite, filter: Option<&str>) -> io::Result<()> {
    let root = suite.fetch()?;
    let files = suite.files(&root, filter)?;
    let pool =
        ThreadPoolBuilder::new().stack_size(MIN_STACK_BYTES).build().map_err(io::Error::other)?;
    let linter = Linter::new(LintConfig {
        categories: CategoryLevels::all(RuleLevel::Warn),
        ..LintConfig::default()
    });
    let reports: Vec<FileReport> = pool.install(|| {
        files.par_iter().map(|file| stages::check(suite, &root, file, &linter)).collect()
    });

    let stages: [(&str, StageResult); 5] = [
        ("parser", |report| Some(&report.parser)),
        ("codegen", |report| report.codegen.as_ref()),
        ("formatter", |report| report.formatter.as_ref()),
        ("minifier", |report| report.minifier.as_ref()),
        ("runtime", |report| report.runtime.as_ref()),
    ];
    for (stage, result_of) in stages {
        let results: Vec<CoverageResult<'_>> = files
            .iter()
            .zip(&reports)
            .filter_map(|(file, report)| {
                result_of(report).map(|result| CoverageResult { path: &file.path, result })
            })
            .collect();
        let snapshot = snapshot::coverage(format!("{stage}_{}", suite.name), &results);
        report(&snapshot, filter.is_some().then_some(results.as_slice()))?;
    }

    let linted: Vec<(&str, &Result<Vec<LintLine>, TestResult>)> = files
        .iter()
        .zip(&reports)
        .filter_map(|(file, report)| report.linter.as_ref().map(|lint| (file.path.as_str(), lint)))
        .collect();
    let snapshot = snapshot::lint(format!("linter_{}", suite.name), &linted);
    report(&snapshot, filter.is_some().then_some(&[]))
}

/// Prints a stage's summary and writes its snapshot, or with `filtered`, the
/// results of the files a filter kept, prints the whole snapshot and a diff
/// for each mismatch among them.
#[expect(clippy::print_stdout, reason = "the console report is this tool's output")]
fn report(snapshot: &Snapshot, filtered: Option<&[CoverageResult<'_>]>) -> io::Result<()> {
    let Some(results) = filtered else {
        println!("{}", snapshot.summary);
        return snapshot.save();
    };
    println!("{snapshot}");
    for &CoverageResult { path, result } in results {
        if let TestResult::Mismatch { check, actual, expected } = result {
            let diff = TextDiff::from_lines(expected, actual);
            println!("{check}: {path}\n{}", diff.unified_diff().header("expected", "actual"));
        }
    }
    Ok(())
}
