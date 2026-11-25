//! The parent side of a run: the original, formatted, and minified programs
//! each in a child process, which must end the same way.

use std::io::{self, Write as _};
use std::path::Path;
use std::process::{Child, Command, ExitStatus, Stdio};
use std::time::{Duration, Instant};
use std::{env, thread};

use crate::execute::{MODE, Verdict};
use crate::snapshot::TestResult;
use crate::status::{self, Rewrite};
use crate::suite::SuiteFile;

/// Far past the second the slowest test takes, so only a program that never
/// ends hits it.
const TIMEOUT: Duration = Duration::from_secs(60);

/// How a run ended.
#[derive(Debug, PartialEq, Eq)]
enum Ending {
    Verdict(Verdict),
    /// The process ended without writing a verdict.
    Crashed(Option<i32>),
    TimedOut,
}

pub(crate) fn is_runnable(file: &SuiteFile) -> bool {
    status::not_runnable(&file.name).is_none()
}

/// The original must return, and each program luck wrote for it must return
/// the same, unless [`status::known_difference`] says it can't. A stage that
/// wrote nothing has its failure in its own snapshot.
pub(crate) fn check(
    root: &Path,
    file: &SuiteFile,
    formatted: Option<&[u8]>,
    minified: Option<&[u8]>,
) -> TestResult {
    let original = match run_in_child(root, file, &file.source) {
        Ok(ending) => ending,
        Err(error) => return TestResult::GenericError("Harness", error.to_string()),
    };
    if !matches!(original, Ending::Verdict(Verdict::Returned(_))) {
        return TestResult::GenericError("Original", describe(&original));
    }
    for (rewrite, written) in [(Rewrite::Formatted, formatted), (Rewrite::Minified, minified)] {
        let Some(program) = written else {
            continue;
        };
        let ending = match run_in_child(root, file, program) {
            Ok(ending) => ending,
            Err(error) => return TestResult::GenericError("Harness", error.to_string()),
        };
        match (ending == original, status::known_difference(&file.name, rewrite)) {
            (true, None) | (false, Some(_)) => {}
            (true, Some(_)) => {
                return TestResult::GenericError(
                    rewrite.name(),
                    "ends as the original does, so it's no longer a known difference".to_owned(),
                );
            }
            (false, None) => return TestResult::GenericError(rewrite.name(), describe(&ending)),
        }
    }
    TestResult::Passed
}

fn describe(ending: &Ending) -> String {
    match ending {
        Ending::Verdict(Verdict::Returned(values)) => format!("returned {values:?}"),
        Ending::Verdict(Verdict::Failed(error)) => error.clone(),
        Ending::Crashed(Some(code)) => format!("exited with code {code} before it finished"),
        Ending::Crashed(None) => "was killed before it finished".to_owned(),
        Ending::TimedOut => format!("ran past {} seconds", TIMEOUT.as_secs()),
    }
}

/// Runs `program` as `file` in a child process, in the suite's directory.
fn run_in_child(root: &Path, file: &SuiteFile, program: &[u8]) -> io::Result<Ending> {
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
    let mut child = command.spawn()?;
    let ended = child
        .stdin
        .take()
        .map_or(Ok(()), |mut stdin| stdin.write_all(program))
        .and_then(|()| wait(&mut child));
    let status = match ended {
        Ok(Some(status)) => status,
        Ok(None) => return Ok(Ending::TimedOut),
        Err(error) => {
            // The run failed already, so only its own error is worth reporting.
            let _ = child.kill();
            let _ = child.wait();
            return Err(error);
        }
    };
    Ok(Verdict::read(&verdict_path).map_or_else(|| Ending::Crashed(status.code()), Ending::Verdict))
}

/// The child's status, or `None` after it ran past [`TIMEOUT`] and was
/// killed.
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
