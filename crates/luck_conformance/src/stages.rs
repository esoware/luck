//! The checks that run in this process: each stage over one file, and over
//! its own output.

use std::any::Any;
use std::panic::{self, AssertUnwindSafe};
use std::path::Path;
use std::slice;

use luck_allocator::Allocator;
use luck_codegen::LiteralSpelling;
use luck_diagnostics::Diagnostic;
use luck_formatter::FormatOptions;
use luck_linter::{FixKind, Linter};
use luck_minifier::TransformConfig;
use luck_parser::ParseResult;
use luck_semantic::SemanticBuilder;
use luck_stdlib::library_for;
use luck_testgen::tree_shape;
use luck_token::{LineIndex, Variant};

use crate::snapshot::{LintLine, TestResult};
use crate::suite::SuiteFile;
use crate::{execute, runtime};

/// What every check found on one file. Only the parser checks a file the
/// reference interpreter rejects, so the other stages are `None` for one.
///
/// The parser's check takes in the errors semantic analysis finds beyond the
/// grammar, as `luck check` does.
#[derive(Debug)]
pub(crate) struct FileReport {
    pub parser: TestResult,
    pub codegen: Option<TestResult>,
    pub formatter: Option<TestResult>,
    pub minifier: Option<TestResult>,
    /// Each diagnostic, as its line, or why the linter didn't run.
    pub linter: Option<Result<Vec<LintLine>, TestResult>>,
    /// Also `None` for a file the harness can't run.
    pub runtime: Option<TestResult>,
}

impl FileReport {
    /// A report of a file every stage stopped on where the parser did.
    fn stopped(file: &SuiteFile, result: TestResult) -> Self {
        Self {
            parser: result.clone(),
            codegen: Some(result.clone()),
            formatter: Some(result.clone()),
            minifier: Some(result.clone()),
            linter: Some(Err(result.clone())),
            runtime: runtime::is_runnable(file).then_some(result),
        }
    }
}

/// `root` is the suite's directory, which the programs run in.
pub(crate) fn check(root: &Path, file: &SuiteFile, linter: &Linter) -> FileReport {
    let source = file.source.as_slice();
    let allocator = Allocator::new();
    let parse = match guard(|| luck_parser::parse_with_tokens(&allocator, source)) {
        Ok(parse) => parse,
        Err(panicked) => return FileReport::stopped(file, panicked),
    };
    let lines = LineIndex::new(source);
    let semantic_errors = if parse.errors.is_empty() {
        match guard(|| SemanticBuilder::new().with_check(true).build(&parse.chunk).errors) {
            Ok(errors) => errors,
            Err(panicked) => return FileReport::stopped(file, panicked),
        }
    } else {
        Vec::new()
    };
    if !execute::compiles(source) {
        let first_error = parse.errors.first().or_else(|| semantic_errors.first());
        let parser = first_error.map_or(TestResult::IncorrectlyPassed, |error| {
            TestResult::CorrectError(render_errors(slice::from_ref(error), &lines))
        });
        return FileReport {
            parser,
            codegen: None,
            formatter: None,
            minifier: None,
            linter: None,
            runtime: None,
        };
    }
    if !parse.errors.is_empty() {
        let result = TestResult::ParseError(render_errors(&parse.errors, &lines));
        return FileReport::stopped(file, result);
    }
    // The tree is whole, so every other stage still has something to check.
    let parser = if semantic_errors.is_empty() {
        TestResult::Passed
    } else {
        TestResult::ParseError(render_errors(&semantic_errors, &lines))
    };
    let codegen = guard(|| codegen(&allocator, &parse)).unwrap_or_else(|panicked| panicked);
    let (formatter, format_output) =
        guard(|| formatter(&allocator, &parse)).unwrap_or_else(|panicked| (panicked, None));
    let (minifier, minify_output) =
        guard(|| minifier(&allocator, source)).unwrap_or_else(|panicked| (panicked, None));
    let linted = guard(|| lint(linter, &parse, &lines));
    let runtime = runtime::is_runnable(file)
        .then(|| runtime::check(root, file, format_output.as_deref(), minify_output.as_deref()));
    FileReport {
        parser,
        codegen: Some(codegen),
        formatter: Some(formatter),
        minifier: Some(minifier),
        linter: Some(linted),
        runtime,
    }
}

/// `run`'s result, or the message it panicked with.
fn guard<T>(run: impl FnOnce() -> T) -> Result<T, TestResult> {
    panic::catch_unwind(AssertUnwindSafe(run))
        .map_err(|payload| TestResult::GenericError("Panic", panic_message(payload.as_ref())))
}

fn panic_message(payload: &(dyn Any + Send)) -> String {
    payload
        .downcast_ref::<&str>()
        .map(|message| (*message).to_owned())
        .or_else(|| payload.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "a panic without a message".to_owned())
}

fn render_errors(errors: &[Diagnostic], lines: &LineIndex) -> String {
    errors
        .iter()
        .map(|error| {
            let (line, column) = lines.line_column(error.span.unwrap_or_default().start);
            format!("{line}:{column} {}", error.message)
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// The output reparses, to the same tree, and prints as itself.
fn codegen(allocator: &Allocator, parse: &ParseResult<'_>) -> TestResult {
    let output = luck_codegen::compact(&parse.chunk, LiteralSpelling::Shortest);
    let reparsed = luck_parser::parse(allocator, &output);
    round_trip(parse, &reparsed).unwrap_or_else(|| {
        idempotency(&luck_codegen::compact(&reparsed.chunk, LiteralSpelling::Shortest), &output)
    })
}

/// The output reparses, to the same tree, and formats as itself.
///
/// The parse has no errors, so formatting reports none.
fn formatter<'ast>(
    allocator: &'ast Allocator,
    parse: &'ast ParseResult<'ast>,
) -> (TestResult, Option<Vec<u8>>) {
    let options = FormatOptions::default();
    let output = luck_formatter::format_parsed(allocator, parse, &options).output;
    let result = {
        let reparsed = luck_parser::parse_with_tokens(allocator, &output);
        round_trip(parse, &reparsed).unwrap_or_else(|| {
            let again = luck_formatter::format_parsed(allocator, &reparsed, &options);
            idempotency(&again.output, &output)
        })
    };
    (result, Some(output))
}

/// The output reparses and minifies as itself.
fn minifier(allocator: &Allocator, source: &[u8]) -> (TestResult, Option<Vec<u8>>) {
    let config = TransformConfig::default();
    let output = match luck_minifier::minify(allocator, source, &config) {
        Ok(output) => output,
        Err(errors) => {
            let lines = LineIndex::new(source);
            return (TestResult::GenericError("Minify", render_errors(&errors, &lines)), None);
        }
    };
    let result = match luck_minifier::minify(allocator, &output, &config) {
        Ok(again) => idempotency(&again, &output),
        Err(errors) => {
            let lines = LineIndex::new(&output);
            TestResult::GenericError("Reparse", render_errors(&errors, &lines))
        }
    };
    (result, Some(output))
}

/// Why `reparsed`, the parse of a stage's output, isn't `parse`'s tree, if it
/// isn't.
fn round_trip(parse: &ParseResult<'_>, reparsed: &ParseResult<'_>) -> Option<TestResult> {
    if !reparsed.errors.is_empty() {
        let lines = LineIndex::new(reparsed.source);
        return Some(TestResult::GenericError("Reparse", render_errors(&reparsed.errors, &lines)));
    }
    let (expected, actual) = (tree_shape(&parse.chunk), tree_shape(&reparsed.chunk));
    (actual != expected).then_some(TestResult::Mismatch { check: "Round trip", actual, expected })
}

/// Whether a stage given its own `output` writes it `again`, with each shown
/// as a snapshot shows bytes.
fn idempotency(again: &[u8], output: &[u8]) -> TestResult {
    if again == output {
        TestResult::Passed
    } else {
        TestResult::Mismatch {
            check: "Idempotency",
            actual: String::from_utf8_lossy(again).into_owned(),
            expected: String::from_utf8_lossy(output).into_owned(),
        }
    }
}

/// `linter`'s diagnostics against Luau's stdlib.
fn lint(linter: &Linter, parse: &ParseResult<'_>, lines: &LineIndex) -> Vec<LintLine> {
    linter
        .run(parse, library_for(Variant::Standalone), FixKind::empty(), None)
        .iter()
        .map(|message| {
            let (line, column) = lines.line_column(message.span().start);
            LintLine {
                rule: message.code,
                text: format!("{line}:{column} {}: {}", message.code, message.message),
            }
        })
        .collect()
}
