use alloc::borrow::Cow;
use alloc::sync::Arc;
use core::ops;
use std::path::PathBuf;
use std::sync::OnceLock;

use luck_allocator::Allocator;
use luck_core::LuaTarget;
use luck_formatter::FormatResult;
use luck_linter::{Analysis, LintDiagnostic};
use luck_parser::ParseResult;
use luck_semantic::{ScopeTree, SemanticAnalysis};
use luck_token::{LuaVersion, Span, Token};
use tower_lsp_server::ls_types::{Position, Range, TextDocumentContentChangeEvent, Uri};

use crate::analyzed::Analyzed;
use crate::line_index::{LineIndex, Positions};
use crate::providers::semantic_tokens::{self, Highlights};
use crate::settings::ProjectSettings;

/// What a document's URI and the project around it fix, which every version
/// of the document shares until the settings change.
#[derive(Debug)]
pub(crate) struct DocumentSettings {
    pub path: Option<PathBuf>,
    pub project: Arc<ProjectSettings>,
    pub target: LuaTarget,
    /// Whether the project's `include` and `exclude` take the document in.
    /// One they leave out gets no diagnostics and no fixes.
    pub is_included: bool,
}

/// One version of an open document, parsed and analyzed once, which the
/// backend shares through an `Arc`, so no request parses or copies the text.
#[derive(Debug)]
pub(crate) struct DocumentState {
    version: i32,
    settings: Arc<DocumentSettings>,
    analyzed: Analyzed,
    line_index: LineIndex,
    /// What the parse read, which highlighting and the cursor's surroundings
    /// look at where the tree may have none.
    tokens: Vec<Token>,
    /// Classified on first use, so a client asking for a range and then the
    /// whole of a version classifies it once.
    highlights: OnceLock<Highlights>,
}

impl DocumentState {
    /// Parses and analyzes `text` in `arena`, which this version then owns.
    pub(crate) fn new(
        text: &str,
        version: i32,
        settings: Arc<DocumentSettings>,
        arena: Allocator,
    ) -> Self {
        let (analyzed, tokens) =
            Analyzed::new(text, settings.target, &settings.project.lint_config, arena);
        let line_index = LineIndex::new(text);
        Self { version, settings, analyzed, line_index, tokens, highlights: OnceLock::new() }
    }

    pub(crate) fn with_text(&self, text: &str, version: i32, arena: Allocator) -> Self {
        Self::new(text, version, Arc::clone(&self.settings), arena)
    }

    /// This version's arena, emptied, for the next to parse into.
    pub(crate) fn into_arena(self) -> Allocator {
        self.analyzed.into_arena()
    }

    pub(crate) const fn version(&self) -> i32 {
        self.version
    }

    pub(crate) fn settings(&self) -> &DocumentSettings {
        &self.settings
    }

    pub(crate) const fn text(&self) -> &str {
        self.parse().source
    }

    pub(crate) const fn parse(&self) -> &ParseResult<'_> {
        self.analyzed.parse()
    }

    pub(crate) const fn analysis(&self) -> &Analysis<'_> {
        self.analyzed.analysis()
    }

    pub(crate) const fn semantic(&self) -> &SemanticAnalysis<'_> {
        self.analysis().semantic()
    }

    pub(crate) const fn tree(&self) -> &ScopeTree<'_> {
        self.semantic().scope_tree()
    }

    pub(crate) fn lua_version(&self) -> LuaVersion {
        self.settings.target.lua_version()
    }

    pub(crate) fn tokens(&self) -> &[Token] {
        &self.tokens
    }

    pub(crate) fn highlights(&self) -> &Highlights {
        self.highlights.get_or_init(|| semantic_tokens::classify(self))
    }

    pub(crate) fn position(&self, offset: u32) -> Position {
        self.line_index.position(self.text(), offset)
    }

    /// A cursor for the positions of offsets taken in order.
    pub(crate) const fn positions(&self) -> Positions<'_> {
        self.line_index.positions(self.text())
    }

    pub(crate) fn offset(&self, position: Position) -> u32 {
        self.line_index.offset(self.text(), position)
    }

    pub(crate) fn range(&self, span: Span) -> Range {
        self.line_index.range(self.text(), span)
    }

    pub(crate) fn byte_range(&self, range: Range) -> Option<ops::Range<usize>> {
        self.line_index.byte_range(self.text(), range)
    }

    pub(crate) fn full_range(&self) -> Range {
        self.line_index.full_document_range(self.text())
    }

    /// Lints with the config the analysis was built under.
    pub(crate) fn lint(&self) -> Vec<LintDiagnostic> {
        let config = &self.settings.project.lint_config;
        luck_linter::lint_analyzed(self.parse(), self.analysis(), config)
    }

    /// Formats the whole text, or with `range`, the statements it touches.
    /// Reads the `.editorconfig` chain, which touches the disk.
    pub(crate) fn format(&self, range: Option<ops::Range<usize>>) -> FormatResult {
        let DocumentSettings { path, project, .. } = &*self.settings;
        let options = project.format_options(path.as_deref());
        // A scratch arena, so the formatting is freed as soon as it's done.
        let allocator = Allocator::new();
        let (parse, version) = (self.parse(), self.lua_version());
        range.map_or_else(
            || luck_formatter::format_parsed(&allocator, parse, version, &options),
            |bytes| luck_formatter::format_range(&allocator, parse, version, &options, bytes),
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

/// The path a `file:` URI names. `Uri::to_file_path` ignores the scheme, so
/// it would turn an `untitled:` buffer into a relative path.
pub(crate) fn file_path(uri: &Uri) -> Option<PathBuf> {
    if !uri.scheme().as_str().eq_ignore_ascii_case("file") {
        return None;
    }
    uri.to_file_path().map(Cow::into_owned)
}

#[cfg(test)]
mod tests {
    use core::str::FromStr as _;
    use std::thread;

    use super::*;
    use crate::fixtures::document;

    #[test]
    fn only_a_file_uri_has_a_file_path() {
        let untitled = Uri::from_str("untitled:Untitled-1").expect("valid URI");
        assert_eq!(file_path(&untitled), None);
        let saved = Uri::from_str("FILE:///tmp/main.lua").expect("valid URI");
        assert!(file_path(&saved).is_some_and(|path| path.ends_with("main.lua")));
    }

    fn replace(range: Range, text: &str) -> TextDocumentContentChangeEvent {
        TextDocumentContentChangeEvent {
            range: Some(range),
            range_length: None,
            text: text.to_owned(),
        }
    }

    #[test]
    fn changed_text_applies_each_range_to_the_previous_result() {
        let document = document(
            "\
local a = 1
local b = 2
local c = 3",
            LuaTarget::Lua54,
        );
        // The first edit inserts a line, and the second edit's range is
        // expressed against the text the first one produced, so the line
        // index must be refreshed between them.
        let text = document.changed_text(vec![
            replace(
                Range::new(Position::new(0, 0), Position::new(0, 11)),
                "\
local a = 10
local aa = 11",
            ),
            replace(Range::new(Position::new(3, 10), Position::new(3, 11)), "30"),
        ]);
        assert_eq!(
            text.as_deref(),
            Some(
                "\
local a = 10
local aa = 11
local b = 2
local c = 30"
            ),
            "second edit must land on the line shifted by the first"
        );
    }

    #[test]
    fn changed_text_refuses_an_inverted_range() {
        let document = document("local a = 1", LuaTarget::Lua54);
        let text = document
            .changed_text(vec![replace(Range::new(Position::new(0, 5), Position::new(0, 2)), "x")]);
        assert_eq!(text, None);
    }

    #[test]
    fn a_document_moved_to_another_thread_keeps_its_tree() {
        let document = Arc::new(document("local answer = 42", LuaTarget::Lua54));
        let shared = Arc::clone(&document);
        let names = thread::spawn(move || {
            let tree = shared.tree();
            tree.symbols()
                .iter()
                .map(|symbol| tree.name(symbol.name).to_owned())
                .collect::<Vec<_>>()
        })
        .join()
        .expect("the reader thread finishes");
        assert_eq!(names, ["answer"]);
        assert_eq!(document.text(), "local answer = 42");
    }
}
