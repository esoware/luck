use core::fmt::Display;
use std::process::ExitCode;

use luck_core::Diagnostic;
use luck_token::{LuaVersion, SourceError};

use crate::render::{print_diags, print_source_errors};

/// How a command failed, once it has reported why. A command that ran and
/// found no problems returns `Ok` and exits 0.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Exit {
    /// Exits 1: the command ran but failed or found problems: diagnostics,
    /// lint findings, files `fmt --check` would change, or a file or directory
    /// it couldn't read or write.
    Failure,
    /// Exits 2: bad arguments, a missing or invalid config, or a path that
    /// doesn't exist.
    Usage,
}

impl From<Exit> for ExitCode {
    fn from(exit: Exit) -> Self {
        match exit {
            Exit::Failure => Self::from(1u8),
            Exit::Usage => Self::from(2u8),
        }
    }
}

fn report_error(message: impl Display, exit: Exit) -> Exit {
    eprintln!("error: {message}");
    exit
}

pub(crate) fn usage_error(message: impl Display) -> Exit {
    report_error(message, Exit::Usage)
}

pub(crate) fn operation_error(message: impl Display) -> Exit {
    report_error(message, Exit::Failure)
}

pub(crate) fn fail_with_diags(diags: &[Diagnostic], version: LuaVersion) -> Exit {
    print_diags(diags, version);
    Exit::Failure
}

/// `source` is a document that isn't on disk, such as a bundle being minified
/// or stdin.
pub(crate) fn fail_with_source_errors(
    path: String,
    source: &str,
    errors: &[SourceError],
    version: LuaVersion,
) -> Exit {
    print_source_errors(path, source, errors, version);
    Exit::Failure
}
