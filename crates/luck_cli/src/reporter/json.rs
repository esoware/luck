use luck_diagnostics::{Diagnostic, Tag};
use luck_token::{LineIndex, Span};
use serde_json::{Value, json};

use super::{Reporter, SourceFile, line_and_column};

/// One JSON object per diagnostic, a line each, for tools to read on stdout.
#[derive(Debug, Clone, Copy)]
pub(crate) struct JsonReporter;

impl Reporter for JsonReporter {
    fn render(&self, file: SourceFile<'_>, diags: &[Diagnostic], output: &mut Vec<u8>) {
        let text = file.text.unwrap_or_default();
        let lines = LineIndex::new(text);
        let place = |span: Option<Span>| Place::of(span, &lines, text);
        for diag in diags {
            let labels: Vec<Value> = diag
                .labels
                .iter()
                .map(|label| {
                    let at = place(Some(label.span));
                    json!({
                        "message": label.message,
                        "start": at.start,
                        "end": at.end,
                        "line": at.line,
                        "column": at.column,
                        "end_line": at.end_line,
                        "end_column": at.end_column,
                    })
                })
                .collect();
            let at = place(diag.span);
            let entry = json!({
                "file": file.label,
                "code": diag.code,
                "severity": diag.severity.name(),
                "message": diag.message,
                "help": diag.help,
                "note": diag.note,
                "url": diag.url,
                "tag": diag.tag.map(Tag::name),
                "start": at.start,
                "end": at.end,
                "line": at.line,
                "column": at.column,
                "end_line": at.end_line,
                "end_column": at.end_column,
                "labels": labels,
            });
            output.extend(entry.to_string().as_bytes());
            output.push(b'\n');
        }
    }
}

/// A span's offsets and 1-based lines and character columns, each `None`
/// for a diagnostic with no place in its file.
#[derive(Default)]
struct Place {
    start: Option<u32>,
    end: Option<u32>,
    line: Option<u32>,
    column: Option<usize>,
    end_line: Option<u32>,
    end_column: Option<usize>,
}

impl Place {
    fn of(span: Option<Span>, lines: &LineIndex, text: &[u8]) -> Self {
        let Some(found) = span else {
            return Self::default();
        };
        let (line, column) = line_and_column(lines, text, found.start);
        let (end_line, end_column) = line_and_column(lines, text, found.end);
        Self {
            start: Some(found.start),
            end: Some(found.end),
            line: Some(line),
            column: Some(column),
            end_line: Some(end_line),
            end_column: Some(end_column),
        }
    }
}

#[cfg(test)]
mod tests {
    use luck_diagnostics::Severity;

    use super::*;

    fn rendered(text: &str, diags: &[Diagnostic]) -> Vec<Value> {
        let mut output = Vec::new();
        let file = SourceFile { label: "src/a.luau", text: Some(text.as_bytes()) };
        JsonReporter.render(file, diags, &mut output);
        let lines = String::from_utf8(output).expect("JSON is UTF-8");
        lines.lines().map(|line| serde_json::from_str(line).expect("a JSON object")).collect()
    }

    #[test]
    fn gives_the_line_and_column_of_a_diagnostic_and_its_labels() {
        let text = "\
local a = 1
local unused = 2";
        let diag =
            Diagnostic::lint("unused_variable", Severity::Warning, Span::new(18, 24), "unused")
                .with_label(Span::new(6, 7), "here")
                .with_tag(Tag::Unnecessary);
        let [entry] = <[Value; 1]>::try_from(rendered(text, &[diag])).expect("one line");
        assert_eq!(entry["file"], "src/a.luau");
        assert_eq!(entry["code"], "unused_variable");
        assert_eq!(entry["tag"], "unnecessary");
        let number = |value: &Value, key: &str| value[key].as_u64().expect("a number");
        assert_eq!((number(&entry, "start"), number(&entry, "end")), (18, 24));
        assert_eq!((number(&entry, "line"), number(&entry, "column")), (2, 7));
        assert_eq!((number(&entry, "end_line"), number(&entry, "end_column")), (2, 13));
        let label = &entry["labels"][0];
        assert_eq!(label["message"], "here");
        assert_eq!((number(label, "line"), number(label, "column")), (1, 7));
    }

    #[test]
    fn a_diagnostic_with_no_place_has_null_positions() {
        let [entry] = <[Value; 1]>::try_from(rendered("", &[Diagnostic::e014(None, "bad")]))
            .expect("one line");
        assert!(entry["line"].is_null() && entry["start"].is_null(), "{entry}");
        assert!(entry["help"].is_null() && entry["url"].is_null(), "{entry}");
    }
}
