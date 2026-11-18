//! The parent side of a run: the original, formatted, and minified programs
//! each in a child process, which must end the same way.

use core::time::Duration;
use std::io::{self, Write as _};
use std::path::Path;
use std::process::{Child, Command, ExitStatus, Stdio};
use std::time::Instant;
use std::{env, fs, thread};

use crate::execute::{FAILED, MODE, RETURNED};
use crate::snapshot::TestResult;
use crate::status::{self, Variant};
use crate::suite::{Suite, SuiteFile};

/// Far past the second the slowest test takes, so only a program that never
/// ends hits it.
const TIMEOUT: Duration = Duration::from_secs(60);

/// How a run ended.
#[derive(Debug, PartialEq, Eq)]
enum Verdict {
    /// What the program returned, one value per line.
    Returned(String),
    Failed(String),
    /// The process ended without writing a verdict.
    Crashed(Option<i32>),
    TimedOut,
}

pub(crate) fn is_runnable(suite: &Suite, file: &SuiteFile) -> bool {
    status::not_runnable(suite, &file.name).is_none()
}

/// The original must return, and each program luck wrote for it must return
/// the same, unless [`status::known_difference`] says it can't. A stage that
/// wrote nothing has its failure in its own snapshot.
pub(crate) fn check(
    suite: &Suite,
    root: &Path,
    file: &SuiteFile,
    formatted: Option<&str>,
    minified: Option<&str>,
) -> TestResult {
    let original = match execute(root, file, &file.bytes) {
        Ok(verdict) => verdict,
        Err(error) => return TestResult::GenericError("Harness", error.to_string()),
    };
    if !matches!(original, Verdict::Returned(_)) {
        return TestResult::GenericError("Original", describe(&original));
    }
    for (variant, written) in Variant::ALL.into_iter().zip([formatted, minified]) {
        let Some(program) = written else {
            continue;
        };
        let verdict = match execute(root, file, program.as_bytes()) {
            Ok(verdict) => verdict,
            Err(error) => return TestResult::GenericError("Harness", error.to_string()),
        };
        match (verdict == original, status::known_difference(suite, &file.name, variant)) {
            (true, None) | (false, Some(_)) => {}
            (true, Some(_)) => {
                return TestResult::GenericError(
                    variant.name(),
                    "ends as the original does, so it's no longer a known difference".to_owned(),
                );
            }
            (false, None) => return TestResult::GenericError(variant.name(), describe(&verdict)),
        }
    }
    TestResult::Passed
}

fn describe(verdict: &Verdict) -> String {
    match verdict {
        Verdict::Returned(values) => format!("returned {values:?}"),
        Verdict::Failed(message) => message.clone(),
        Verdict::Crashed(Some(code)) => format!("exited with code {code} before it finished"),
        Verdict::Crashed(None) => "was killed before it finished".to_owned(),
        Verdict::TimedOut => format!("ran past {} seconds", TIMEOUT.as_secs()),
    }
}

/// Runs `program` as `file` in a child process, in the suite's directory.
///
/// # Errors
///
/// Fails when the child can't start, take its input, or be waited on.
fn execute(root: &Path, file: &SuiteFile, program: &[u8]) -> io::Result<Verdict> {
    let directory = tempfile::tempdir()?;
    let verdict_path = directory.path().join("verdict");
    let mut command = Command::new(env::current_exe()?);
    command
        .arg(MODE)
        .arg(format!("@{}", file.name))
        .arg(&verdict_path)
        .current_dir(root)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    // The reference interpreter reads its search paths from these.
    for (key, _value) in env::vars_os() {
        if key.to_string_lossy().starts_with("LUA_") {
            command.env_remove(key);
        }
    }
    let mut child = command.spawn()?;
    if let Some(mut stdin) = child.stdin.take() {
        stdin.write_all(program)?;
    }
    let Some(status) = wait(&mut child)? else {
        return Ok(Verdict::TimedOut);
    };
    Ok(match fs::read_to_string(&verdict_path) {
        Ok(verdict) => parse_verdict(&verdict, status),
        Err(_missing) => Verdict::Crashed(status.code()),
    })
}

/// The child's status, or `None` after it ran past [`TIMEOUT`] and was
/// killed.
///
/// # Errors
///
/// Fails when the child can't be waited on or killed.
fn wait(child: &mut Child) -> io::Result<Option<ExitStatus>> {
    let started = Instant::now();
    loop {
        if let Some(status) = child.try_wait()? {
            return Ok(Some(status));
        }
        if started.elapsed() >= TIMEOUT {
            child.kill()?;
            child.wait()?;
            return Ok(None);
        }
        thread::sleep(Duration::from_millis(10));
    }
}

fn parse_verdict(verdict: &str, status: ExitStatus) -> Verdict {
    let (kind, payload) = verdict.split_once('\n').unwrap_or((verdict, ""));
    match kind {
        RETURNED => Verdict::Returned(payload.to_owned()),
        FAILED => Verdict::Failed(payload.to_owned()),
        _ => Verdict::Crashed(status.code()),
    }
}
