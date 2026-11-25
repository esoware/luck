use std::fmt::Display;
use std::process::ExitCode;
use std::slice;

use luck_diagnostics::FileDiagnostics;
use luck_token::LuaVersion;

use crate::reporter::print_files;

/// How a command failed, once it has reported why. A command that ran and
/// found no problems returns `Ok` and exits 0.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Exit {
    /// Exits 1: the command ran but failed or found problems: diagnostics,
    /// lint findings, files `fmt --check` would change, or a file or directory
    /// it couldn't read or write.
    Failure,
    /// Exits 2: bad arguments, a missing or invalid config, a path that
    /// doesn't exist, or no file to work on.
    Usage,
}

impl From<Exit> for ExitCode {
    fn from(exit: Exit) -> Self {
        match exit {
            Exit::Failure => Self::from(1),
            Exit::Usage => Self::from(2),
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

pub(crate) fn fail_with_diags(files: &[FileDiagnostics], version: LuaVersion) -> Exit {
    print_files(files, version);
    Exit::Failure
}

/// A `luck.json` that doesn't load is a usage error.
pub(crate) fn config_error(file: &FileDiagnostics) -> Exit {
    // Lua 5.4 ends a line where JSON5 does, but at the rare LS and PS.
    print_files(slice::from_ref(file), LuaVersion::Lua54);
    Exit::Usage
}
