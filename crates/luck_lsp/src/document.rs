//! An open document's parse and analysis, and the conversions between its
//! byte offsets and LSP positions.

use core::ops;

use luck_core::LuaTarget;
use luck_linter::{LintConfig, LintDiagnostic};
use luck_parser::ParseResult;
use luck_semantic::SemanticAnalysis;
use luck_token::Span;
use tower_lsp::lsp_types::{Position, Range, TextDocumentContentChangeEvent};

use crate::line_index::LineIndex;

/// An open document with its parse and semantic analysis.
///
/// Both are built once per change, and the backend shares the whole state
/// through an `Arc`, so no request reparses or copies the text.
#[derive(Debug)]
pub(crate) struct DocumentState {
    pub version: i32,
    pub target: LuaTarget,
    pub parsed: ParseResult,
    pub semantic: SemanticAnalysis,
    pub line_index: LineIndex,
}

impl DocumentState {
    pub(crate) fn new(text: String, version: i32, target: LuaTarget) -> Self {
        let line_index = LineIndex::new(&text);
        let parsed = luck_parser::parse(text, target.lua_version());
        let semantic = luck_semantic::analyze_with_environment(
            &parsed.block,
            target.lua_version(),
            target.stdlib_environment(),
        );
        Self { version, target, parsed, semantic, line_index }
    }

    pub(crate) fn text(&self) -> &str {
        &self.parsed.source
    }

    pub(crate) fn position(&self, offset: u32) -> Position {
        self.line_index.position(self.text(), offset)
    }

    pub(crate) fn offset(&self, position: Position) -> u32 {
        self.line_index.offset(self.text(), position)
    }

    pub(crate) fn range(&self, span: Span) -> Range {
        self.line_index.range(self.text(), span)
    }

    /// The byte range an LSP `Range` covers, or `None` when it's inverted.
    pub(crate) fn byte_range(&self, range: Range) -> Option<ops::Range<usize>> {
        self.line_index.byte_range(self.text(), range)
    }

    pub(crate) fn full_range(&self) -> Range {
        self.line_index.full_document_range(self.text())
    }

    pub(crate) fn lint(&self, config: &LintConfig) -> Vec<LintDiagnostic> {
        luck_linter::lint_parsed(
            &self.parsed,
            self.target.lua_version(),
            self.target.stdlib_environment(),
            config,
        )
    }

    /// The text once `changes` are applied in order, or `None` when a range
    /// doesn't fit the text it was meant for.
    pub(crate) fn changed_text(
        &self,
        changes: Vec<TextDocumentContentChangeEvent>,
    ) -> Option<String> {
        let mut text = self.text().to_owned();
        let mut rebuilt_index = None;
        let mut remaining_changes = changes.into_iter().peekable();
        while let Some(change) = remaining_changes.next() {
            if let Some(range) = change.range {
                let line_index = rebuilt_index.as_ref().unwrap_or(&self.line_index);
                let byte_range = line_index.byte_range(&text, range)?;
                text.replace_range(byte_range, &change.text);
            } else {
                text = change.text;
            }
            // The next change's range is relative to this change's result.
            // `DocumentState::new` rebuilds the final index, so the last
            // change skips it.
            if remaining_changes.peek().is_some() {
                rebuilt_index = Some(LineIndex::new(&text));
            }
        }
        Some(text)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn replace(range: Range, text: &str) -> TextDocumentContentChangeEvent {
        TextDocumentContentChangeEvent {
            range: Some(range),
            range_length: None,
            text: text.to_owned(),
        }
    }

    #[test]
    fn changed_text_applies_each_range_to_the_previous_result() {
        let document = DocumentState::new(
            "local a = 1\nlocal b = 2\nlocal c = 3".to_owned(),
            1,
            LuaTarget::Lua54,
        );
        // The first edit inserts a line, and the second edit's range is
        // expressed against the text the first one produced, so the line
        // index must be refreshed between them.
        let text = document.changed_text(vec![
            replace(
                Range::new(Position::new(0, 0), Position::new(0, 11)),
                "local a = 10\nlocal aa = 11",
            ),
            replace(Range::new(Position::new(3, 10), Position::new(3, 11)), "30"),
        ]);
        assert_eq!(
            text.as_deref(),
            Some("local a = 10\nlocal aa = 11\nlocal b = 2\nlocal c = 30"),
            "second edit must land on the line shifted by the first"
        );
    }

    #[test]
    fn changed_text_refuses_an_inverted_range() {
        let document = DocumentState::new("local a = 1".to_owned(), 1, LuaTarget::Lua54);
        let text = document
            .changed_text(vec![replace(Range::new(Position::new(0, 5), Position::new(0, 2)), "x")]);
        assert_eq!(text, None);
    }
}
