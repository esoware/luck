//! The checks that run in this process: each stage over one file, and over
//! its own output.

use core::any::Any;
use core::panic::AssertUnwindSafe;
use core::slice;
use std::panic;
use std::path::Path;

use luck_allocator::Allocator;
use luck_codegen::LiteralSpelling;
use luck_diagnostics::Diagnostic;
use luck_formatter::FormatOptions;
use luck_linter::{CategoryLevels, FixKind, LintConfig, Linter, RuleLevel};
use luck_minifier::TransformConfig;
use luck_parser::ParseResult;
use luck_semantic::SemanticBuilder;
use luck_stdlib::library_for;
use luck_testgen::tree_shape;
use luck_token::{LineIndex, LuaVersion};

use crate::snapshot::{LintLine, TestResult};
use crate::suite::{Suite, SuiteFile};
use crate::{execute, runtime};

/// What every check found on one file. Only the parser checks a file the
/// reference interpreter rejects, so the other stages are `None` for one.
///
/// The parser's check takes in the errors semantic analysis finds beyond the
/// grammar, as `luck check` does.
#[derive(Debug)]
pub(crate) struct FileReport {
    /// The reference interpreter rejects the file.
    pub should_fail: bool,
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
    fn stopped(suite: &Suite, file: &SuiteFile, result: &TestResult) -> Self {
        Self {
            should_fail: false,
            parser: result.clone(),
            codegen: Some(result.clone()),
            formatter: Some(result.clone()),
            minifier: Some(result.clone()),
            linter: Some(Err(result.clone())),
            runtime: runtime::is_runnable(suite, file).then(|| result.clone()),
        }
    }
}

/// The linter the `linter` stage runs, with every rule on.
pub(crate) fn every_rule() -> Linter {
    Linter::new(LintConfig {
        categories: CategoryLevels {
            correctness: Some(RuleLevel::Warn),
            suspicious: Some(RuleLevel::Warn),
            style: Some(RuleLevel::Warn),
            performance: Some(RuleLevel::Warn),
        },
        ..LintConfig::default()
    })
}

/// `root` is the suite's directory, which the programs run in.
pub(crate) fn check(suite: &Suite, root: &Path, file: &SuiteFile, linter: &Linter) -> FileReport {
    let source = file.bytes.as_slice();
    let version = suite.target.version();
    let allocator = Allocator::new();
    let parse = match guard(|| luck_parser::parse_with_tokens(&allocator, source, version)) {
        Ok(parse) => parse,
        Err(panicked) => return FileReport::stopped(suite, file, &panicked),
    };
    let lines = LineIndex::new(source, version);
    let compile_errors = if parse.errors.is_empty() {
        match guard(|| SemanticBuilder::new().with_check(true).build(&parse.chunk).diagnostics) {
            Ok(errors) => errors,
            Err(panicked) => return FileReport::stopped(suite, file, &panicked),
        }
    } else {
        parse.errors.clone()
    };
    if !execute::compiles(&file.bytes) {
        let parser = compile_errors.first().map_or(TestResult::IncorrectlyPassed, |error| {
            TestResult::CorrectError(render_errors(slice::from_ref(error), &lines))
        });
        return FileReport {
            should_fail: true,
            parser,
            codegen: None,
            formatter: None,
            minifier: None,
            linter: None,
            runtime: None,
        };
    }
    if !parse.errors.is_empty() {
        return FileReport::stopped(
            suite,
            file,
            &TestResult::ParseError(render_errors(&parse.errors, &lines)),
        );
    }
    // The tree is whole, so every other stage still has something to check.
    let parser = if compile_errors.is_empty() {
        TestResult::Passed
    } else {
        TestResult::ParseError(render_errors(&compile_errors, &lines))
    };
    let codegen = guard(|| codegen(&allocator, &parse)).unwrap_or_else(|error| error);
    let (formatter, format_output) =
        guard(|| formatter(&allocator, &parse)).unwrap_or_else(|error| (error, None));
    let (minifier, minify_output) =
        guard(|| minifier(&allocator, source, version)).unwrap_or_else(|error| (error, None));
    let linted = guard(|| lint(suite, linter, &parse, &lines));
    let runtime = runtime::is_runnable(suite, file).then(|| {
        runtime::check(suite, root, file, format_output.as_deref(), minify_output.as_deref())
    });
    FileReport {
        should_fail: false,
        parser,
        codegen: Some(codegen),
        formatter: Some(formatter),
        minifier: Some(minifier),
        linter: Some(linted),
        runtime,
    }
}

/// `run`'s result.
///
/// # Errors
///
/// Returns the panic `run` raised, as a failure.
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
    let version = parse.chunk.version;
    let output = luck_codegen::compact(&parse.chunk, LiteralSpelling::Shortest);
    let reparsed = luck_parser::parse(allocator, &output, version);
    if !reparsed.errors.is_empty() {
        let lines = LineIndex::new(&output, version);
        return TestResult::GenericError("Reparse", render_errors(&reparsed.errors, &lines));
    }
    let (expected, actual) = (tree_shape(&parse.chunk), tree_shape(&reparsed.chunk));
    if actual != expected {
        return TestResult::Mismatch("Round trip", actual, expected);
    }
    let again = luck_codegen::compact(&reparsed.chunk, LiteralSpelling::Shortest);
    if again != output {
        return TestResult::Mismatch("Idempotency", lossy(&again), lossy(&output));
    }
    TestResult::Passed
}

/// A stage's output, which is bytes, as a snapshot shows it.
fn lossy(output: &[u8]) -> String {
    String::from_utf8_lossy(output).into_owned()
}

/// The output reparses, to the same tree, and formats as itself.
fn formatter<'ast>(
    allocator: &'ast Allocator,
    parse: &'ast ParseResult<'ast>,
) -> (TestResult, Option<Vec<u8>>) {
    let version = parse.chunk.version;
    let options = FormatOptions::default();
    let first = luck_formatter::format_parsed(allocator, parse, &options);
    if !first.errors.is_empty() {
        let lines = LineIndex::new(parse.source, version);
        return (TestResult::GenericError("Format", render_errors(&first.errors, &lines)), None);
    }
    let output = first.output;
    let result = {
        let reparsed = luck_parser::parse_with_tokens(allocator, &output, version);
        if reparsed.errors.is_empty() {
            let (expected, actual) = (tree_shape(&parse.chunk), tree_shape(&reparsed.chunk));
            if actual == expected {
                let again = luck_formatter::format_parsed(allocator, &reparsed, &options);
                if again.output == output {
                    TestResult::Passed
                } else {
                    TestResult::Mismatch("Idempotency", lossy(&again.output), lossy(&output))
                }
            } else {
                TestResult::Mismatch("Round trip", actual, expected)
            }
        } else {
            let lines = LineIndex::new(&output, version);
            TestResult::GenericError("Reparse", render_errors(&reparsed.errors, &lines))
        }
    };
    (result, Some(output))
}

/// The output reparses and minifies as itself.
fn minifier(
    allocator: &Allocator,
    source: &[u8],
    version: LuaVersion,
) -> (TestResult, Option<Vec<u8>>) {
    let config = TransformConfig::default();
    let output = match luck_minifier::minify(allocator, source, version, &config) {
        Ok(output) => output,
        Err(errors) => {
            let lines = LineIndex::new(source, version);
            return (TestResult::GenericError("Minify", render_errors(&errors, &lines)), None);
        }
    };
    let result = match luck_minifier::minify(allocator, &output, version, &config) {
        Ok(again) if again == output => TestResult::Passed,
        Ok(again) => TestResult::Mismatch("Idempotency", lossy(&again), lossy(&output)),
        Err(errors) => {
            let lines = LineIndex::new(&output, version);
            TestResult::GenericError("Reparse", render_errors(&errors, &lines))
        }
    };
    (result, Some(output))
}

/// `linter`'s diagnostics.
fn lint(
    suite: &Suite,
    linter: &Linter,
    parse: &ParseResult<'_>,
    lines: &LineIndex,
) -> Vec<LintLine> {
    linter
        .run(parse, library_for(suite.target), FixKind::empty())
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
