use std::ops;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};

use ls_types::{DocumentSymbol, Position, Range, TextDocumentContentChangeEvent};
use luck_linter::{Analysis, FixKind, Message};
use luck_parser::ParseResult;
use luck_resolver::{Resolution, ResolveError};
use luck_semantic::{AstNodes, Scoping};
use luck_stdlib::SemanticAnalysis;
use luck_token::{Span, Token};

use crate::analyzed::{Analyzed, Arena};
use crate::line_index::{LineIndex, PositionEncoding, Positions};
use crate::module_record::{ModuleRecord, Require};
use crate::providers::document_symbol;
use crate::providers::semantic_tokens::{self, Highlights};
use crate::session::Session;
use crate::settings::DocumentSettings;

/// An open document's path and the settings its versions share until the
/// settings change, which a worker finds the first time one is analyzed,
/// since that reads the disk.
#[derive(Debug)]
struct LazySettings {
    /// As the client spells it, or `None` for a document with no file.
    path: Option<PathBuf>,
    resolved: OnceLock<Arc<DocumentSettings>>,
}

impl LazySettings {
    const fn new(path: Option<PathBuf>) -> Self {
        Self { path, resolved: OnceLock::new() }
    }

    fn resolve(&self, session: &Session) -> &Arc<DocumentSettings> {
        self.resolved.get_or_init(|| Arc::new(session.document_settings(self.path.as_deref())))
    }
}

/// One version of an open document, which every job that reads it shares.
///
/// Its analysis and its lints are built the first time a job asks, on that
/// job's thread, and a job that asks meanwhile waits for them.
#[derive(Debug)]
pub(crate) struct Document {
    version: i32,
    text: Arc<str>,
    settings: Arc<LazySettings>,
    line_index: OnceLock<Arc<LineIndex>>,
    state: OnceLock<DocumentState>,
    /// `None` when the version went stale while its lint ran.
    lints: OnceLock<Option<Vec<Message>>>,
    /// Set once a newer version or a close replaces this one, which stops
    /// its lint.
    is_stale: AtomicBool,
}

impl Document {
    /// Version `version` of the document at `path`, as the client spells it,
    /// or of a document with no file.
    pub(crate) fn new(version: i32, text: Arc<str>, path: Option<PathBuf>) -> Self {
        Self::with_settings(version, text, Arc::new(LazySettings::new(path)))
    }

    const fn with_settings(version: i32, text: Arc<str>, settings: Arc<LazySettings>) -> Self {
        Self {
            version,
            text,
            settings,
            line_index: OnceLock::new(),
            state: OnceLock::new(),
            lints: OnceLock::new(),
            is_stale: AtomicBool::new(false),
        }
    }

    pub(crate) const fn version(&self) -> i32 {
        self.version
    }

    pub(crate) const fn text(&self) -> &Arc<str> {
        &self.text
    }

    /// This version again, under settings found afresh.
    pub(crate) fn reloaded(&self) -> Self {
        Self::new(self.version, Arc::clone(&self.text), self.settings.path.clone())
    }

    /// The canonical path of its file, or `None` when it has none.
    pub(crate) fn path(&self, session: &Session) -> Option<&Path> {
        self.settings.resolve(session).path.as_deref()
    }

    pub(crate) fn mark_stale(&self) {
        self.is_stale.store(true, Ordering::Relaxed);
    }

    pub(crate) fn is_stale(&self) -> bool {
        self.is_stale.load(Ordering::Relaxed)
    }

    fn line_index(&self, encoding: PositionEncoding) -> &Arc<LineIndex> {
        self.line_index.get_or_init(|| Arc::new(LineIndex::new(&self.text, encoding)))
    }

    pub(crate) fn state(&self, session: &Session) -> &DocumentState {
        self.state.get_or_init(|| {
            DocumentState::new(
                Arc::clone(&self.text),
                Arc::clone(self.settings.resolve(session)),
                Arc::clone(self.line_index(session.encoding())),
                Arena::from_spare(session.spare_arena()),
            )
        })
    }

    /// This version's lints, which publishing, code actions, and fix-all
    /// share, or `None` when it went stale before they were done.
    pub(crate) fn lints(&self, session: &Session) -> Option<&[Message]> {
        self.lints.get_or_init(|| self.state(session).lint(&|| self.is_stale())).as_deref()
    }

    /// Version `version`: this one's text with `changes` applied in order,
    /// or `None` when a range doesn't fit the text it was meant for.
    pub(crate) fn changed(
        &self,
        version: i32,
        changes: Vec<TextDocumentContentChangeEvent>,
        encoding: PositionEncoding,
    ) -> Option<Self> {
        // A change with no range replaces the whole text, so the ones before
        // the last of those don't count.
        let last_whole = changes.iter().rposition(|change| change.range.is_none());
        let mut remaining = changes.into_iter().skip(last_whole.unwrap_or(0));
        // The lines before this one still start where the index has them.
        let (mut text, mut valid_lines) = match last_whole {
            Some(_) => (remaining.next()?.text, 0),
            None => (String::from(&*self.text), u32::MAX),
        };
        // Each change's range is in the text the one before it made. Clients
        // such as VS Code send a batch last first, so this keeps one index
        // for as long as every change ends on a line before the last one
        // began, as rust-analyzer does.
        let mut rebuilt: Option<LineIndex> = None;
        for change in remaining {
            let range = change.range?;
            if valid_lines <= range.end.line {
                rebuilt = Some(LineIndex::new(&text, encoding));
            }
            valid_lines = range.start.line;
            let index: &LineIndex = match &rebuilt {
                Some(index) => index,
                None => self.line_index(encoding),
            };
            let byte_range = index.byte_range(&text, range)?;
            text.replace_range(byte_range, &change.text);
        }
        Some(Self::with_settings(version, Arc::from(text), Arc::clone(&self.settings)))
    }
}

/// A version of a document, parsed and analyzed, which is what every
/// provider reads.
#[derive(Debug)]
pub(crate) struct DocumentState {
    settings: Arc<DocumentSettings>,
    line_index: Arc<LineIndex>,
    analyzed: Analyzed,
    /// Classified on first use, so a client asking for a range and then the
    /// whole of a version classifies it once.
    highlights: OnceLock<Highlights>,
    /// The outline, built on first use, which the outline and every
    /// workspace symbol query read.
    symbols: OnceLock<Vec<DocumentSymbol>>,
    /// Built on first use, which links, diagnostics, and every cross-file
    /// request read.
    record: OnceLock<ModuleRecord>,
}

impl DocumentState {
    pub(crate) fn new(
        text: Arc<str>,
        settings: Arc<DocumentSettings>,
        line_index: Arc<LineIndex>,
        arena: Arena,
    ) -> Self {
        let stdlib = settings.project.project().stdlib().clone();
        let analyzed = Analyzed::new(text, stdlib, &settings.linter, arena);
        Self {
            settings,
            line_index,
            analyzed,
            highlights: OnceLock::new(),
            symbols: OnceLock::new(),
            record: OnceLock::new(),
        }
    }

    pub(crate) fn settings(&self) -> &DocumentSettings {
        &self.settings
    }

    pub(crate) fn text(&self) -> &str {
        self.analyzed.text()
    }

    /// [`Self::text`], which the type checker reads while the version is
    /// open.
    pub(crate) fn shared_text(&self) -> &Arc<str> {
        self.analyzed.text()
    }

    pub(crate) fn parse(&self) -> &ParseResult<'_> {
        self.analyzed.parse()
    }

    fn analysis(&self) -> &Analysis<'_> {
        self.analyzed.analysis()
    }

    pub(crate) fn semantic(&self) -> &SemanticAnalysis<'_> {
        self.analysis().semantic()
    }

    pub(crate) fn scoping(&self) -> &Scoping {
        self.semantic().scoping()
    }

    /// Every node of the parse, in source preorder with its parent, which
    /// each provider finds the node at the cursor in.
    pub(crate) fn nodes(&self) -> &AstNodes<'_> {
        self.semantic().semantic().nodes()
    }

    /// What the parse read, which the cursor's surroundings are read from
    /// where the tree has nothing, as in a comment, a string, or a member
    /// still being typed, and which the formatter reads too.
    pub(crate) fn tokens(&self) -> &[Token] {
        &self.parse().tokens
    }

    pub(crate) fn highlights(&self) -> &Highlights {
        self.highlights.get_or_init(|| semantic_tokens::classify(self))
    }

    pub(crate) fn symbols(&self) -> &[DocumentSymbol] {
        self.symbols.get_or_init(|| document_symbol::document_symbols(self))
    }

    /// What the other files see of this one: its requires and where it
    /// names a member of a module's table.
    pub(crate) fn record(&self) -> &ModuleRecord {
        self.record.get_or_init(|| {
            let mut positions = self.positions();
            let semantic = self.semantic().semantic();
            ModuleRecord::new(&self.parse().chunk, semantic, |span| positions.range(span))
        })
    }

    /// What `require` loads, through the project's resolver, or `None` for
    /// a document with no file, which no relative path starts from.
    pub(crate) fn resolve(&self, require: &Require) -> Option<Result<Resolution, ResolveError>> {
        let path = self.settings.path.as_deref()?;
        Some(self.settings.project.resolver().resolve(path, &require.module))
    }

    pub(crate) fn position(&self, offset: u32) -> Position {
        self.line_index.position(self.text(), offset)
    }

    /// A cursor for the positions of offsets taken in order.
    pub(crate) fn positions(&self) -> Positions<'_> {
        self.line_index.positions(self.text())
    }

    pub(crate) fn offset(&self, position: Position) -> u32 {
        self.line_index.offset(self.text(), position)
    }

    pub(crate) fn range(&self, span: Span) -> Range {
        self.line_index.range(self.text(), span)
    }

    pub(crate) fn encoding(&self) -> PositionEncoding {
        self.line_index.encoding()
    }

    /// How many of the session's units the text at `span` takes.
    pub(crate) fn length(&self, span: Span) -> u32 {
        self.line_index.length(span)
    }

    pub(crate) fn byte_range(&self, range: Range) -> Option<ops::Range<usize>> {
        self.line_index.byte_range(self.text(), range)
    }

    pub(crate) fn full_range(&self) -> Range {
        self.line_index.full_document_range(self.text())
    }

    /// Lints with the linter that built the analysis, building every kind
    /// of fix, since a suggestion is still a quick fix, or `None` once
    /// `is_cancelled` stops it. A document the project leaves out has none.
    pub(crate) fn lint(&self, is_cancelled: &dyn Fn() -> bool) -> Option<Vec<Message>> {
        if !self.settings.is_included {
            return Some(Vec::new());
        }
        self.settings.linter.run_analyzed(self.analysis(), FixKind::all(), None, is_cancelled)
    }
}

/// What `span` covers of `text`, a document, whose spans all start and end
/// on a character, since an editor's text is UTF-8.
pub(crate) fn span_text(text: &str, span: Span) -> &str {
    &text[ops::Range::<usize>::from(span)]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn version(text: &str) -> Document {
        Document::new(1, Arc::from(text), None)
    }

    fn replace(range: Range, text: &str) -> TextDocumentContentChangeEvent {
        TextDocumentContentChangeEvent {
            range: Some(range),
            range_length: None,
            text: text.to_owned(),
        }
    }

    fn replace_all(text: &str) -> TextDocumentContentChangeEvent {
        TextDocumentContentChangeEvent { range: None, range_length: None, text: text.to_owned() }
    }

    fn changed_text(
        document: &Document,
        changes: Vec<TextDocumentContentChangeEvent>,
    ) -> Option<String> {
        let next = document.changed(2, changes, PositionEncoding::Utf16)?;
        assert_eq!(next.version(), 2);
        Some(String::from(&*next.text))
    }

    #[test]
    fn each_change_applies_to_the_text_the_last_made() {
        let document = version(
            "\
local a = 1
local b = 2
local c = 3",
        );
        // The first edit inserts a line, and the second edit's range is
        // expressed against the text the first one produced, so the line
        // index must be refreshed between them.
        let text = changed_text(
            &document,
            vec![
                replace(
                    Range::new(Position::new(0, 0), Position::new(0, 11)),
                    "\
local a = 10
local aa = 11",
                ),
                replace(Range::new(Position::new(3, 10), Position::new(3, 11)), "30"),
            ],
        );
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
    fn changes_sent_last_first_land_where_they_were_meant() {
        let document = version(
            "\
local a = 1
local b = 2
local c = 3",
        );
        let text = changed_text(
            &document,
            vec![
                replace(Range::new(Position::new(2, 6), Position::new(2, 7)), "\u{e4}\u{e4}"),
                replace(Range::new(Position::new(0, 6), Position::new(0, 7)), "first\nline"),
            ],
        );
        assert_eq!(
            text.as_deref(),
            Some(
                "\
local first
line = 1
local b = 2
local \u{e4}\u{e4} = 3"
            )
        );
    }

    #[test]
    fn a_whole_text_change_replaces_what_came_before_it() {
        let document = version("local a = 1");
        let text = changed_text(
            &document,
            vec![
                replace(Range::new(Position::new(0, 0), Position::new(0, 5)), "ignored"),
                replace_all("local b = 2"),
                replace(Range::new(Position::new(0, 6), Position::new(0, 7)), "c"),
            ],
        );
        assert_eq!(text.as_deref(), Some("local c = 2"));
    }

    #[test]
    fn no_change_keeps_the_text() {
        let document = version("local a = 1");
        assert_eq!(changed_text(&document, Vec::new()).as_deref(), Some("local a = 1"));
    }

    #[test]
    fn a_change_refuses_an_inverted_range() {
        let document = version("local a = 1");
        let text = changed_text(
            &document,
            vec![replace(Range::new(Position::new(0, 5), Position::new(0, 2)), "x")],
        );
        assert_eq!(text, None);
    }
}
