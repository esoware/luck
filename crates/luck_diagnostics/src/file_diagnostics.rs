use std::path::PathBuf;

use crate::diagnostic::Diagnostic;

/// The diagnostics in the file at `path`, which name no file of their own.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileDiagnostics {
    pub path: PathBuf,
    pub diags: Vec<Diagnostic>,
}
