use core::mem::ManuallyDrop;
use std::process::Command;
use std::{env, thread};

use luck_token::LuaVersion;

/// The stack `luck_cli` gives its workers.
const WORKER_STACK_BYTES: usize = 16 * 1024 * 1024;

const STACK_PROBE_VARIABLE: &str = "LUCK_CODEGEN_STACK_PROBE";

/// Asserts that `source` compacts to exactly `expected`, which re-parses
/// clean and compacts to itself.
///
/// A second compaction that changes the output catches output that re-parses
/// with a different meaning.
pub(crate) fn assert_compacts_to(source: &str, expected: &str, version: LuaVersion) {
    let parsed = luck_parser::parse(source, version);
    assert!(parsed.errors.is_empty(), "parse errors for {source:?}: {:?}", parsed.errors);
    let output = luck_codegen::compact(&parsed.block);
    assert_eq!(output, expected, "input: {source:?}");
    let reparsed = luck_parser::parse(&output, version);
    assert!(
        reparsed.errors.is_empty(),
        "compact output {output:?} doesn't re-parse: {:?}",
        reparsed.errors
    );
    assert_eq!(
        luck_codegen::compact(&reparsed.block),
        output,
        "compact output of {source:?} changed when compacted again"
    );
}

/// Asserts that `compact` returns for `source` on a worker-sized stack.
///
/// A stack overflow aborts the process, so a child copy of the test at
/// `test_path` compacts and the parent asserts on how it exited. The AST is
/// never dropped, since dropping a deep one recurses outside this crate.
#[expect(clippy::expect_used, reason = "a test helper panics to fail")]
pub(crate) fn assert_compact_fits_worker_stack(test_path: &str, source: &str, version: LuaVersion) {
    if env::var_os(STACK_PROBE_VARIABLE).is_some() {
        let parsed = ManuallyDrop::new(luck_parser::parse(source, version));
        assert!(parsed.errors.is_empty(), "parse errors: {:?}", parsed.errors);
        let output = thread::scope(|scope| {
            thread::Builder::new()
                .stack_size(WORKER_STACK_BYTES)
                .spawn_scoped(scope, || luck_codegen::compact(&parsed.block))
                .expect("worker thread starts")
                .join()
                .expect("compact doesn't panic")
        });
        assert!(!output.is_empty(), "compact printed nothing");
        return;
    }
    let child = Command::new(env::current_exe().expect("test binary path"))
        .args(["--exact", test_path])
        .env(STACK_PROBE_VARIABLE, "1")
        .output()
        .expect("test binary starts");
    assert!(
        child.status.success() && String::from_utf8_lossy(&child.stdout).contains("1 passed"),
        "compact must print a source the parser accepted without exhausting a \
         {WORKER_STACK_BYTES}-byte stack, but the process running it exited with {}",
        child.status
    );
}
