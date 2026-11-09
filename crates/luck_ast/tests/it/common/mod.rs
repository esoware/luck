use std::env;
use std::process::{Command, Stdio};
use std::thread;

const CHILD_MARKER: &str = "LUCK_AST_DEEP_CHAIN_CHILD";
/// `luck_cli`'s `WORKER_STACK_BYTES`.
const SCENARIO_STACK_BYTES: usize = 16 * 1024 * 1024;

/// Runs `scenario` in a re-executed copy of this test binary, so that a stack
/// overflow, which aborts the process, shows up as a failed assertion.
pub(crate) fn assert_runs_without_stack_overflow(test_path: &str, scenario: fn()) {
    if env::var_os(CHILD_MARKER).is_some() {
        let worker = thread::Builder::new()
            .stack_size(SCENARIO_STACK_BYTES)
            .spawn(scenario)
            .expect("spawn scenario thread");
        worker.join().expect("scenario must not panic");
        return;
    }
    let status = Command::new(env::current_exe().expect("test binary path"))
        .args(["--exact", test_path, "--nocapture"])
        .env(CHILD_MARKER, "1")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .expect("spawn test binary");
    assert!(
        status.success(),
        "{test_path}: must finish on a long chain instead of overflowing the stack ({status})"
    );
}
