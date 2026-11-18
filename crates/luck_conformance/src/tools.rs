//! The checks that run in this process: each stage over one file, and over
//! its own output.

use core::any::Any;
use core::panic::AssertUnwindSafe;
use core::slice;
use std::panic;
use std::path::Path;

use luck_allocator::Allocator;
use luck_formatter::FormatOptions;
use luck_linter::{CategoryLevels, FixKind, LintConfig, RuleLevel};
use luck_minifier::TransformConfig;
use luck_parser::ParseResult;
use luck_testgen::tree_shape;
use luck_token::{LineIndex, LuaVersion, SourceError};

use crate::snapshot::{LintLine, TestResult};
use crate::suite::{Suite, SuiteFile};
use crate::{execute, runtime};

/// What every check found on one file. Only the parser checks a file the
/// reference interpreter rejects, so the other stages are `None` for one.
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

/// `root` is the suite's directory, which the programs run in.
pub(crate) fn check(suite: &Suite, root: &Path, file: &SuiteFile) -> FileReport {
    let Ok(source) = str::from_utf8(&file.bytes) else {
        return FileReport::stopped(suite, file, &TestResult::NotUtf8);
    };
    let version = suite.target.lua_version();
    let allocator = Allocator::new();
    let parse = match guard(|| luck_parser::parse(&allocator, source, version)) {
        Ok(parse) => parse,
        Err(panicked) => return FileReport::stopped(suite, file, &panicked),
    };
    let lines = LineIndex::new(source, version);
    if !execute::compiles(&file.bytes) {
        let parser = parse.errors.first().map_or(TestResult::IncorrectlyPassed, |error| {
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
    let codegen = guard(|| codegen(&allocator, &parse, version)).unwrap_or_else(|error| error);
    let (formatter, format_output) =
        guard(|| formatter(&allocator, &parse, version)).unwrap_or_else(|error| (error, None));
    let (minifier, minify_output) =
        guard(|| minifier(&allocator, source, version)).unwrap_or_else(|error| (error, None));
    let linter = guard(|| lint(suite, &parse, &lines));
    let runtime = runtime::is_runnable(suite, file).then(|| {
        runtime::check(suite, root, file, format_output.as_deref(), minify_output.as_deref())
    });
    FileReport {
        should_fail: false,
        parser: TestResult::Passed,
        codegen: Some(codegen),
        formatter: Some(formatter),
        minifier: Some(minifier),
        linter: Some(linter),
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

fn render_errors(errors: &[SourceError], lines: &LineIndex) -> String {
    errors
        .iter()
        .map(|error| {
            let (line, column) = lines.line_column(error.span.start);
            format!("{line}:{column} {}", error.message)
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// The output reparses, to the same tree, and prints as itself.
fn codegen(allocator: &Allocator, parse: &ParseResult<'_>, version: LuaVersion) -> TestResult {
    let output = luck_codegen::compact(&parse.block);
    let reparsed = luck_parser::parse(allocator, &output, version);
    if !reparsed.errors.is_empty() {
        let lines = LineIndex::new(&output, version);
        return TestResult::GenericError("Reparse", render_errors(&reparsed.errors, &lines));
    }
    let (expected, actual) =
        (tree_shape(&parse.block, version), tree_shape(&reparsed.block, version));
    if actual != expected {
        return TestResult::Mismatch("Round trip", actual, expected);
    }
    let again = luck_codegen::compact(&reparsed.block);
    if again != output {
        return TestResult::Mismatch("Idempotency", again, output);
    }
    TestResult::Passed
}

/// The output reparses, to the same tree, and formats as itself.
fn formatter<'ast>(
    allocator: &'ast Allocator,
    parse: &'ast ParseResult<'ast>,
    version: LuaVersion,
) -> (TestResult, Option<String>) {
    let options = FormatOptions::default();
    let first = luck_formatter::format_parsed(allocator, parse, version, &options);
    if !first.errors.is_empty() {
        let lines = LineIndex::new(parse.source, version);
        return (TestResult::GenericError("Format", render_errors(&first.errors, &lines)), None);
    }
    let output = first.output;
    let result = {
        let reparsed = luck_parser::parse(allocator, &output, version);
        if reparsed.errors.is_empty() {
            let (expected, actual) =
                (tree_shape(&parse.block, version), tree_shape(&reparsed.block, version));
            if actual == expected {
                let again = luck_formatter::format_parsed(allocator, &reparsed, version, &options);
                if again.output == output {
                    TestResult::Passed
                } else {
                    TestResult::Mismatch("Idempotency", again.output, output.clone())
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
    source: &str,
    version: LuaVersion,
) -> (TestResult, Option<String>) {
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
        Ok(again) => TestResult::Mismatch("Idempotency", again, output.clone()),
        Err(errors) => {
            let lines = LineIndex::new(&output, version);
            TestResult::GenericError("Reparse", render_errors(&errors, &lines))
        }
    };
    (result, Some(output))
}

/// Every rule's diagnostics.
fn lint(suite: &Suite, parse: &ParseResult<'_>, lines: &LineIndex) -> Vec<LintLine> {
    let every_rule = LintConfig {
        categories: CategoryLevels {
            correctness: Some(RuleLevel::Warn),
            suspicious: Some(RuleLevel::Warn),
            style: Some(RuleLevel::Warn),
            performance: Some(RuleLevel::Warn),
        },
        ..LintConfig::default()
    };
    luck_linter::lint_parsed(parse, suite.target, &every_rule, FixKind::empty())
        .iter()
        .map(|diagnostic| {
            let (line, column) = lines.line_column(diagnostic.span.start);
            LintLine {
                rule: diagnostic.rule,
                text: format!("{line}:{column} {}: {}", diagnostic.rule, diagnostic.message),
            }
        })
        .collect()
}
