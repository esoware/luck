//! # `luck_allocs`
//!
//! Counts the heap and arena allocations each pipeline stage makes on every
//! benchmark file, and writes them to the committed `allocs_<stage>.yaml`
//! snapshots beside this crate's `Cargo.toml`, so a change's effect on
//! allocations shows up as a diff. `luck_allocs rules` writes the gitignored
//! `allocs_linter_rules.yaml` instead, with each lint rule measured alone.
//!
//! Ported from oxc's `tasks/track_memory_allocations`.

mod heap;
mod rules;
mod snapshot;
mod stats;

use std::path::Path;
use std::{env, io, panic, thread};

use luck_allocator::Allocator;
use luck_ast::MIN_STACK_BYTES;
use luck_benchmark_files::{TestFile, test_files};
use luck_formatter::FormatOptions;
use luck_linter::{CategoryLevels, FixKind, LintConfig, RuleLevel};
use luck_minifier::TransformConfig;
use rayon::ThreadPoolBuilder;

use crate::heap::{HEAP, TrackedAllocator};
use crate::snapshot::write_snapshot;
use crate::stats::{StageStats, record_stats_in};

#[global_allocator]
static GLOBAL: TrackedAllocator = TrackedAllocator;

const STAGES: [&str; 7] =
    ["lexer", "parser", "semantic", "linter", "codegen", "formatter", "minifier"];

/// The stages run on a thread with the stack every stage needs, as the CLI's
/// do.
///
/// # Errors
///
/// Fails on an argument other than `rules`, or when the thread can't start
/// or a snapshot can't be written.
fn main() -> io::Result<()> {
    let is_rules = match env::args().skip(1).collect::<Vec<_>>().as_slice() {
        [] => false,
        [mode] if mode == "rules" => true,
        _ => {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "expected no arguments, or `rules` for the per-rule linter report",
            ));
        }
    };
    thread::Builder::new()
        .stack_size(MIN_STACK_BYTES)
        .spawn(move || run(is_rules))?
        .join()
        .unwrap_or_else(|payload| panic::resume_unwind(payload))
}

/// # Errors
///
/// Fails when a snapshot can't be written or the thread pool can't start.
fn run(is_rules: bool) -> io::Result<()> {
    // The linter splits a large file's work across rayon's pool, and a pool
    // of only this thread runs it inline, so the counts don't depend on how
    // the work was stolen.
    ThreadPoolBuilder::new()
        .num_threads(1)
        .use_current_thread()
        .build_global()
        .map_err(io::Error::other)?;
    let files = test_files();

    // Running everything once first grows the arena to the size the largest
    // file needs and initializes anything lazy, so the measured run doesn't
    // depend on the order of the files.
    let mut allocator = Allocator::new();
    for file in &files {
        allocator.reset();
        measure(&allocator, file);
    }
    if is_rules {
        return rules::run(&mut allocator, &files);
    }

    let mut entries: [Vec<(&TestFile, StageStats)>; STAGES.len()] = Default::default();
    for file in &files {
        allocator.reset();
        for (stage_entries, stats) in entries.iter_mut().zip(measure(&allocator, file)) {
            stage_entries.push((file, stats));
        }
    }
    let directory = Path::new(env!("CARGO_MANIFEST_DIR"));
    for (stage, stage_entries) in STAGES.iter().zip(&entries) {
        write_snapshot(&directory.join(format!("allocs_{stage}.yaml")), stage_entries)?;
    }
    Ok(())
}

/// Every stage on `file`, in `STAGES`' order, in a freshly reset
/// `allocator`.
///
/// Each one's output is dropped inside its measurement, except the parse,
/// which the stages after it read. The minifier rewrites the tree, so it runs
/// last.
///
/// # Panics
///
/// Panics when `file` doesn't parse cleanly.
fn measure(allocator: &Allocator, file: &TestFile) -> [StageStats; STAGES.len()] {
    HEAP.reset();
    let (source, target) = (file.source_text.as_str(), file.target);
    let version = target.lua_version();

    let ((), lexer) = record_stats_in(allocator, || drop(luck_lexer::lex(source, version)));
    let (mut parse, parser) =
        record_stats_in(allocator, || luck_parser::parse(allocator, source, version));
    assert!(parse.errors.is_empty(), "{}: {:?}", file.file_name, parse.errors);
    let environment = target.stdlib_environment();
    let ((), semantic) = record_stats_in(allocator, || {
        drop(luck_semantic::analyze(&parse.chunk, parse.counts, environment));
    });
    let config = every_rule();
    let ((), linter) = record_stats_in(allocator, || {
        drop(luck_linter::lint_parsed(&parse, environment, &config, FixKind::empty()));
    });
    let ((), codegen) = record_stats_in(allocator, || drop(luck_codegen::compact(&parse.chunk)));
    let options = FormatOptions::default();
    let ((), formatter) = record_stats_in(allocator, || {
        drop(luck_formatter::format_parsed(allocator, &parse, &options));
    });
    let transforms = TransformConfig::default();
    let ((), minifier) = record_stats_in(allocator, || {
        drop(luck_minifier::minify_chunk(allocator, &mut parse.chunk, &transforms));
    });
    [lexer, parser, semantic, linter, codegen, formatter, minifier]
}

/// Every category on, as the linter bench runs it.
fn every_rule() -> LintConfig {
    LintConfig {
        categories: CategoryLevels {
            correctness: Some(RuleLevel::Warn),
            suspicious: Some(RuleLevel::Warn),
            style: Some(RuleLevel::Warn),
            performance: Some(RuleLevel::Warn),
        },
        ..LintConfig::default()
    }
}
