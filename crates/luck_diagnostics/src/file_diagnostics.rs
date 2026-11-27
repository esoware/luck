use std::error::Error;
use std::fmt;
use std::path::PathBuf;

use crate::diagnostic::Diagnostic;

/// The diagnostics in the file at `path`, which name no file of their own.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileDiagnostics {
    pub path: PathBuf,
    pub diags: Vec<Diagnostic>,
}

impl fmt::Display for FileDiagnostics {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (index, diag) in self.diags.iter().enumerate() {
            if index > 0 {
                f.write_str("\n")?;
            }
            write!(f, "{}: {diag}", self.path.display())?;
        }
        Ok(())
    }
}

impl Error for FileDiagnostics {}

#[cfg(test)]
mod tests {
    use luck_token::Span;

    use super::*;

    #[test]
    fn displays_a_line_for_each_diagnostic() {
        let span = Span::new(0, 0);
        let file = FileDiagnostics {
            path: PathBuf::from("main.luau"),
            diags: vec![Diagnostic::e008(span, "first"), Diagnostic::e008(span, "second")],
        };
        assert_eq!(
            file.to_string(),
            "\
main.luau: first
main.luau: second"
        );
    }
}
