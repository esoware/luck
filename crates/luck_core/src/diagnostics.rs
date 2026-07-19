use std::ops::Range;

/// A rich diagnostic (error or warning) with source location and labels.
#[derive(Debug, Clone)]
pub struct Diagnostic {
    pub code: String,
    pub message: String,
    pub severity: DiagnosticSeverity,
    pub file_path: String,
    pub span: Range<usize>,
    pub labels: Vec<(Range<usize>, String)>,
    pub help: Option<String>,
}

/// Whether a diagnostic is an error (blocks the build) or a warning (informational).
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum DiagnosticSeverity {
    Error,
    Warning,
}

/// Category of a lint rule. Owned here so the whole luck.json config (including
/// the typed `lint` section) deserializes without depending on the linter crate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Category {
    /// Outright wrong or useless code.
    Correctness,
    /// Most likely wrong or useless.
    Suspicious,
    /// Idiomatic code style.
    Style,
    /// Performance improvements.
    #[serde(alias = "perf")]
    Performance,
}

impl Diagnostic {
    pub fn error(code: &str, message: String, file_path: String, span: Range<usize>) -> Self {
        Self {
            code: code.to_string(),
            message,
            severity: DiagnosticSeverity::Error,
            file_path,
            span,
            labels: Vec::new(),
            help: None,
        }
    }

    pub fn warning(code: &str, message: String, file_path: String, span: Range<usize>) -> Self {
        Self {
            code: code.to_string(),
            message,
            severity: DiagnosticSeverity::Warning,
            file_path,
            span,
            labels: Vec::new(),
            help: None,
        }
    }

    pub fn error_at(
        code: &str,
        message: String,
        file_path: String,
        span: luck_token::Span,
    ) -> Self {
        Self::error(code, message, file_path, span.into())
    }

    pub fn warning_at(
        code: &str,
        message: String,
        file_path: String,
        span: luck_token::Span,
    ) -> Self {
        Self::warning(code, message, file_path, span.into())
    }

    pub fn with_label(mut self, span: Range<usize>, message: String) -> Self {
        self.labels.push((span, message));
        self
    }

    pub fn with_help(mut self, help: String) -> Self {
        self.help = Some(help);
        self
    }

    pub fn is_error(&self) -> bool {
        self.severity == DiagnosticSeverity::Error
    }
}

pub mod errors {
    use super::*;

    pub fn e001(file_path: &str, span: Range<usize>) -> Diagnostic {
        Diagnostic::error(
            "E001",
            "require must appear at the top of the file".to_string(),
            file_path.to_string(),
            span,
        )
        .with_help(
            "Move all require statements to the top of the file, before any other code."
                .to_string(),
        )
    }

    pub fn e002(file_path: &str, span: Range<usize>) -> Diagnostic {
        Diagnostic::error(
            "E002",
            "require argument must be a string literal".to_string(),
            file_path.to_string(),
            span,
        )
        .with_help("Use a string literal: require(\"module_name\")".to_string())
    }

    pub fn e003(file_path: &str, span: Range<usize>) -> Diagnostic {
        Diagnostic::error(
            "E003",
            "require must be assigned to a local variable".to_string(),
            file_path.to_string(),
            span,
        )
        .with_help("Assign the result: local mod = require(\"module\")".to_string())
    }

    pub fn e004(
        file_path: &str,
        span: Range<usize>,
        module_name: &str,
        tried_paths: &[String],
    ) -> Diagnostic {
        let paths_list = tried_paths.join("\n  - ");
        Diagnostic::error(
            "E004",
            format!("module not found: \"{module_name}\""),
            file_path.to_string(),
            span,
        )
        .with_help(format!("Searched paths:\n  - {paths_list}"))
    }

    pub fn e005(file_path: &str, span: Range<usize>, cycle_path: &[String]) -> Diagnostic {
        let cycle_str = cycle_path.join(" -> ");
        Diagnostic::error(
            "E005",
            "circular dependency detected".to_string(),
            file_path.to_string(),
            span,
        )
        .with_help(format!("Cycle: {cycle_str}"))
    }

    pub fn e006(file_path: &str, span: Range<usize>) -> Diagnostic {
        Diagnostic::error(
            "E006",
            "package.loaded manipulation is not allowed".to_string(),
            file_path.to_string(),
            span,
        )
        .with_help(
            "luck does not support runtime module caching. Remove package.loaded manipulation."
                .to_string(),
        )
    }

    pub fn e007(file_path: &str, span: Range<usize>, details: &str) -> Diagnostic {
        Diagnostic::error(
            "E007",
            "ambiguous module resolution".to_string(),
            file_path.to_string(),
            span,
        )
        .with_help(format!(
            "Ambiguity: {details}. Remove one of the conflicting files."
        ))
    }

    pub fn e008(file_path: &str, span: Range<usize>, parse_error: &str) -> Diagnostic {
        Diagnostic::error(
            "E008",
            format!("parse error: {parse_error}"),
            file_path.to_string(),
            span,
        )
    }
    pub fn e009(file_path: &str, span: Range<usize>, limit: usize) -> Diagnostic {
        Diagnostic::error(
            "E009",
            format!("too many modules: exceeded {limit} module limit"),
            file_path.to_string(),
            span,
        )
        .with_help(format!(
            "A bundle may contain at most {limit} modules. Reduce the dependency graph or split the build."
        ))
    }

    pub fn e010(file_path: &str, span: Range<usize>, io_error: &str) -> Diagnostic {
        Diagnostic::error(
            "E010",
            format!("cannot read file \"{file_path}\": {io_error}"),
            file_path.to_string(),
            span,
        )
        .with_help("Check that the file exists and is readable.".to_string())
    }

    pub fn e011(entry_path: &str, span: Range<usize>) -> Diagnostic {
        Diagnostic::error(
            "E011",
            format!("entry file not found: \"{entry_path}\""),
            entry_path.to_string(),
            span,
        )
        .with_help(
            "Verify the entry path passed to the bundler points at an existing file.".to_string(),
        )
    }

    pub fn e012(file_path: &str, span: Range<usize>, byte_len: usize, limit: usize) -> Diagnostic {
        Diagnostic::error(
            "E012",
            format!("file too large: {byte_len} bytes (limit: {limit})"),
            file_path.to_string(),
            span,
        )
        .with_help(format!("Files must be at most {limit} bytes."))
    }

    pub fn w001(file_path: &str, span: Range<usize>, module_name: &str) -> Diagnostic {
        Diagnostic::warning(
            "W001",
            format!("module \"{module_name}\" required multiple times"),
            file_path.to_string(),
            span,
        )
    }

    pub fn w002(file_path: &str, span: Range<usize>, module_name: &str) -> Diagnostic {
        Diagnostic::warning(
            "W002",
            format!(
                "module \"{module_name}\" uses top-level vararg (...) which will be empty in bundled output"
            ),
            file_path.to_string(),
            span,
        )
    }

    pub fn w004(file_path: &str, span: Range<usize>) -> Diagnostic {
        Diagnostic::warning(
            "W004",
            "alias \"self\" defined in .luaurc is shadowed by built-in @self".to_string(),
            file_path.to_string(),
            span,
        )
    }

    pub fn w003(file_path: &str, span: Range<usize>, cycle_path: &[String]) -> Diagnostic {
        let cycle_str = cycle_path.join(" -> ");
        Diagnostic::warning(
            "W003",
            "circular dependency between modules".to_string(),
            file_path.to_string(),
            span,
        )
        .with_help(format!(
            "Cycle: {cycle_str}\nModules load lazily, so cycles deferred into function bodies work; \
             a cycle hit while a module is still loading raises at runtime."
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_diagnostic_creation() {
        let diag = Diagnostic::error(
            "E001",
            "test message".to_string(),
            "test.lua".to_string(),
            0..10,
        );
        assert!(diag.is_error());
        assert_eq!(diag.code, "E001");
    }

    #[test]
    fn test_diagnostic_warning() {
        let diag = Diagnostic::warning(
            "W001",
            "test warning".to_string(),
            "test.lua".to_string(),
            0..5,
        );
        assert!(!diag.is_error());
        assert_eq!(diag.severity, DiagnosticSeverity::Warning);
    }

    #[test]
    fn test_diagnostic_with_labels() {
        let diag = Diagnostic::error("E001", "msg".to_string(), "f.lua".to_string(), 0..10)
            .with_label(5..8, "here".to_string())
            .with_help("do this".to_string());
        assert_eq!(diag.labels.len(), 1);
        assert!(diag.help.is_some());
    }

    #[test]
    fn test_error_constructors() {
        let e1 = errors::e001("f.lua", 0..10);
        assert_eq!(e1.code, "E001");
        assert!(e1.is_error());

        let e4 = errors::e004("f.lua", 0..10, "mymod", &["src/mymod.lua".to_string()]);
        assert!(
            e4.help
                .expect("e004 always sets help")
                .contains("src/mymod.lua")
        );

        let w1 = errors::w001("f.lua", 0..5, "utils");
        assert!(!w1.is_error());
    }
}
