use std::process::ExitCode;

/// How a command failed, once it has reported why. A command that ran and
/// found no problems returns `Ok` and exits 0.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Exit {
    /// Exits 1: the command ran but failed or found problems: diagnostics,
    /// lint findings, files `fmt --check` would change, or a file it couldn't
    /// read or write.
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
