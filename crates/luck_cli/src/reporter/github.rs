use std::fmt::Write as _;
use std::path::{self, Path};

use luck_diagnostics::Diagnostic;
use luck_resolver::{FileSystem as _, FileSystemOs, to_slash_lossy};
use luck_token::LineIndex;

use super::{Reporter, SourceFile, line_and_column};

/// GitHub Actions workflow commands, an annotation per diagnostic, for the
/// runner to read on stdout.
#[derive(Debug, Clone, Copy)]
pub(crate) struct GithubReporter;

impl Reporter for GithubReporter {
    fn render(&self, file: SourceFile<'_>, diags: &[Diagnostic], output: &mut Vec<u8>) {
        let text = file.text.unwrap_or_default();
        let lines = LineIndex::new(text, file.version);
        // The runner resolves a relative path from the workspace root, not
        // the working directory, and makes an absolute one relative to the
        // workspace.
        let label = Path::new(file.label);
        let absolute = path::absolute(label).unwrap_or_else(|_| label.to_path_buf());
        let canonical = FileSystemOs.canonicalize(&absolute).unwrap_or(absolute);
        let path = annotation_property(&to_slash_lossy(&canonical));
        let mut annotations = String::new();
        for diag in diags {
            // An annotation has one place, so the others go in its message.
            let mut message = diag.message.to_string();
            for other in &diag.labels {
                let (line, column) = line_and_column(&lines, text, other.span.start);
                let _ = write!(message, "\n{line}:{column}: {}", other.message);
            }
            for (heading, extra) in [("help", &diag.help), ("note", &diag.note)] {
                if let Some(added) = extra {
                    let _ = write!(message, "\n{heading}: {added}");
                }
            }
            let place = diag.span.map_or_else(String::new, |span| {
                let (line, column) = line_and_column(&lines, text, span.start);
                let (end_line, end_column) = line_and_column(&lines, text, span.end);
                format!(",line={line},endLine={end_line},col={column},endColumn={end_column}")
            });
            let _ = writeln!(
                annotations,
                "::{} file={path}{place},title={}::{}",
                diag.severity.name(),
                annotation_property(diag.code),
                annotation_data(&message),
            );
        }
        output.extend(annotations.as_bytes());
    }
}

/// The message of a GitHub workflow command, escaped the way the runner
/// unescapes it.
fn annotation_data(text: &str) -> String {
    text.replace('%', "%25").replace('\r', "%0D").replace('\n', "%0A")
}

/// A property value of a GitHub workflow command, where `:` and `,` delimit
/// too.
fn annotation_property(text: &str) -> String {
    annotation_data(text).replace(':', "%3A").replace(',', "%2C")
}

#[cfg(test)]
mod tests {
    use std::env;

    use luck_diagnostics::Severity;
    use luck_token::{LuaVersion, Span};

    use super::*;

    /// The path is absolute, since the runner would resolve a relative one
    /// from the workspace root even when lint runs in a subdirectory.
    #[test]
    fn writes_one_annotation_per_diagnostic() {
        let text = "\
local a = 1
local unused = 2
print(a)";
        let diag = Diagnostic::lint(
            "unused_variable",
            Severity::Warning,
            Span::new(18, 24),
            "unused variable `unused`",
        )
        .with_label(Span::new(6, 7), "a label")
        .with_help("prefix with `_` to suppress this warning");
        let mut output = Vec::new();
        let source = SourceFile {
            label: r"src\a.lua",
            text: Some(text.as_bytes()),
            version: LuaVersion::Lua54,
        };
        GithubReporter.render(source, &[diag], &mut output);
        let lines = String::from_utf8(output).expect("annotations are UTF-8");
        let (file, rest) = lines
            .strip_prefix("::warning file=")
            .and_then(|annotation| annotation.split_once(','))
            .expect("one warning annotation");
        let working_directory = env::current_dir().expect("current directory");
        assert_eq!(
            file,
            annotation_property(&to_slash_lossy(&working_directory.join(r"src\a.lua")))
        );
        assert_eq!(
            rest,
            "line=2,endLine=2,col=7,endColumn=13,title=unused_variable::unused variable \
             `unused`%0A1:7: a label%0Ahelp: prefix with `_` to suppress this warning\n"
        );
    }

    #[test]
    fn escapes_what_the_runner_would_parse() {
        assert_eq!(annotation_data("50% of a\r\nb"), "50%25 of a%0D%0Ab");
        assert_eq!(annotation_property("C:/a,b.lua"), "C%3A/a%2Cb.lua");
    }
}
