//! `tower_lsp::LanguageServer` implementation backed by `luck_formatter`
//! and `luck_linter`.
//!
//! The backend keeps each open document fully in-memory because both the
//! formatter and the linter operate on a whole-source string. Incremental
//! syncs are folded into the document on every change so we never re-fetch
//! from disk after `did_open`.

use std::collections::HashMap;
use std::sync::Arc;

use luck_core::LuaTarget;
use tokio::sync::RwLock;
use tower_lsp::jsonrpc::Result as JsonRpcResult;
use tower_lsp::lsp_types::{
    CodeActionOptions, CodeActionParams, CodeActionProviderCapability, CodeActionResponse,
    CompletionOptions, CompletionParams, CompletionResponse, Diagnostic,
    DidChangeTextDocumentParams, DidChangeWatchedFilesParams,
    DidChangeWatchedFilesRegistrationOptions, DidCloseTextDocumentParams,
    DidOpenTextDocumentParams, DidSaveTextDocumentParams, DocumentFormattingParams,
    DocumentHighlight, DocumentHighlightParams, DocumentLink, DocumentLinkOptions,
    DocumentLinkParams, DocumentRangeFormattingParams, DocumentSymbolParams,
    DocumentSymbolResponse, FileSystemWatcher, FoldingRange, FoldingRangeParams,
    FoldingRangeProviderCapability, GlobPattern, GotoDefinitionParams, GotoDefinitionResponse,
    Hover, HoverParams, HoverProviderCapability, InitializeParams, InitializeResult,
    InitializedParams, Location, MessageType, OneOf, Position, PrepareRenameResponse,
    ReferenceParams, Registration, RenameOptions, RenameParams, SaveOptions, SelectionRange,
    SelectionRangeParams, SelectionRangeProviderCapability, SemanticTokensFullOptions,
    SemanticTokensOptions, SemanticTokensRangeParams, SemanticTokensRangeResult,
    SemanticTokensRegistrationOptions, SemanticTokensResult, SemanticTokensServerCapabilities,
    ServerCapabilities, ServerInfo, SignatureHelp, SignatureHelpOptions, SignatureHelpParams,
    StaticRegistrationOptions, SymbolInformation, TextDocumentPositionParams,
    TextDocumentRegistrationOptions, TextDocumentSyncCapability, TextDocumentSyncKind,
    TextDocumentSyncOptions, TextDocumentSyncSaveOptions, TextEdit, Url, WorkDoneProgressOptions,
    WorkspaceEdit, WorkspaceSymbolParams,
};
use tower_lsp::{Client, LanguageServer};

use crate::config::{ConfigCache, ProjectSettings, resolved_format_options, target_from_settings};
use crate::diagnostics::to_lsp_diagnostics;
use crate::line_index::LineIndex;
use crate::providers::{
    code_action, completion, definition, document_link, document_symbol, folding, highlights,
    hover, references, rename, selection_range, semantic_tokens, signature_help, syntax_tree,
    workspace_symbol,
};

/// In-memory snapshot of a single open document.
///
/// The parsed AST is computed eagerly on construction / update and shared
/// across providers via `Arc`. Every did_change already triggers a lint
/// (which also parses) so the cost is paid exactly once per change rather
/// than once per LSP request.
#[derive(Debug, Clone)]
pub struct DocumentState {
    pub text: String,
    pub version: i32,
    pub target: LuaTarget,
    /// Cached line index, rebuilt on every change. Computing it is cheap
    /// (single pass), but every request needs one so we keep it around.
    pub line_index: LineIndex,
    /// Eagerly-parsed AST + comments + parse errors for the current text.
    pub parsed: Arc<luck_parser::ParseResult>,
    /// Semantic analysis over `parsed`, shared by every provider -
    /// recomputing it per request made document_highlight and friends
    /// re-walk the whole file on cursor idle.
    pub semantic: Arc<luck_semantic::SemanticAnalysis>,
}

impl DocumentState {
    fn new(text: String, version: i32, target: LuaTarget) -> Self {
        let line_index = LineIndex::new(&text);
        let parsed = Arc::new(luck_parser::parse(&text, target.lua_version()));
        let semantic = Arc::new(luck_semantic::analyze_with_environment(
            &parsed.block,
            target.lua_version(),
            target.stdlib_environment(),
        ));
        Self {
            text,
            version,
            target,
            line_index,
            parsed,
            semantic,
        }
    }

    fn update(&mut self, text: String, version: i32) {
        self.text = text;
        self.version = version;
        self.line_index = LineIndex::new(&self.text);
        self.parsed = Arc::new(luck_parser::parse(&self.text, self.target.lua_version()));
        self.semantic = Arc::new(luck_semantic::analyze_with_environment(
            &self.parsed.block,
            self.target.lua_version(),
            self.target.stdlib_environment(),
        ));
    }
}

/// Trait abstracting the bits of `tower_lsp::Client` that the backend uses.
/// Lets the integration tests drop in a no-op client while keeping the real
/// server wired to `tower_lsp::Client`.
#[async_trait::async_trait]
pub trait Notifier: Send + Sync + 'static {
    async fn publish_diagnostics(&self, uri: Url, diags: Vec<Diagnostic>, version: Option<i32>);
    async fn show_message(&self, ty: MessageType, message: String);
    async fn log_message(&self, ty: MessageType, message: String);
    /// Dynamic capability registration; the test notifier no-ops.
    async fn register_capability(&self, registrations: Vec<Registration>) {
        let _ = registrations;
    }
}

#[async_trait::async_trait]
impl Notifier for Client {
    async fn publish_diagnostics(&self, uri: Url, diags: Vec<Diagnostic>, version: Option<i32>) {
        Client::publish_diagnostics(self, uri, diags, version).await;
    }

    async fn show_message(&self, ty: MessageType, message: String) {
        Client::show_message(self, ty, message).await;
    }

    async fn log_message(&self, ty: MessageType, message: String) {
        Client::log_message(self, ty, message).await;
    }

    async fn register_capability(&self, registrations: Vec<Registration>) {
        let _ = Client::register_capability(self, registrations).await;
    }
}

/// The LSP server state shared across requests.
pub struct Backend<N: Notifier = Client> {
    notifier: Arc<N>,
    documents: Arc<RwLock<HashMap<Url, DocumentState>>>,
    config_cache: Arc<ConfigCache>,
    /// Last lint result per document, keyed by version. VS Code fires
    /// codeAction on nearly every cursor move; without this each request
    /// re-ran the full rule set.
    lint_cache: Arc<RwLock<HashMap<Url, VersionedLints>>>,
}

/// Lint results tagged with the document version they were computed for.
type VersionedLints = (i32, Arc<Vec<luck_linter::diagnostic::LintDiagnostic>>);

impl<N: Notifier> Backend<N> {
    pub fn new(notifier: N) -> Self {
        Self {
            notifier: Arc::new(notifier),
            documents: Arc::new(RwLock::new(HashMap::new())),
            config_cache: Arc::new(ConfigCache::new()),
            lint_cache: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Snapshot the current text for a URI, if open. Used by integration tests.
    pub async fn document_text(&self, uri: &Url) -> Option<String> {
        let documents = self.documents.read().await;
        documents.get(uri).map(|doc| doc.text.clone())
    }

    fn project_settings_for(&self, uri: &Url) -> ProjectSettings {
        match uri.to_file_path() {
            Ok(path) => self.config_cache.settings_for(&path),
            Err(_) => ProjectSettings::default(),
        }
    }

    fn target_for(&self, uri: &Url, settings: &ProjectSettings) -> LuaTarget {
        match uri.to_file_path() {
            Ok(path) => target_from_settings(&path, settings),
            Err(_) => settings.lua_target,
        }
    }

    async fn lint_and_publish(&self, uri: Url) {
        let snapshot = {
            let documents = self.documents.read().await;
            documents.get(&uri).cloned()
        };
        let Some(doc) = snapshot else {
            return;
        };
        let settings = self.project_settings_for(&uri);
        // Skip files outside the project's include/exclude set entirely.
        if let Ok(path) = uri.to_file_path() {
            if !settings.filter.is_included(&path) {
                self.notifier
                    .publish_diagnostics(uri, Vec::new(), Some(doc.version))
                    .await;
                return;
            }
        }
        // Opt-in: with no `lint` section, lint with all rules off so only
        // parse errors surface (luck_linter returns parse errors first).
        let lint_config = settings.effective_lint_config();
        // The document's parse is already cached - lint it directly
        // instead of handing the linter raw text to re-parse.
        let lint_diags = Arc::new(luck_linter::lint_parsed(
            &doc.parsed,
            doc.target.lua_version(),
            doc.target.stdlib_environment(),
            &lint_config,
        ));
        {
            let mut lint_cache = self.lint_cache.write().await;
            lint_cache.insert(uri.clone(), (doc.version, Arc::clone(&lint_diags)));
        }
        let lsp_diags = to_lsp_diagnostics(&doc.text, &doc.line_index, &lint_diags);
        self.notifier
            .publish_diagnostics(uri, lsp_diags, Some(doc.version))
            .await;
    }

    async fn apply_incremental_change(
        &self,
        uri: &Url,
        params: DidChangeTextDocumentParams,
    ) -> bool {
        let mut documents = self.documents.write().await;
        let Some(doc) = documents.get_mut(uri) else {
            return false;
        };
        // tower-lsp dispatches notifications concurrently: two rapid
        // didChange batches can arrive out of order. Applying version
        // N+2's ranges against version N's text silently corrupts the
        // document FOREVER - reject stale/duplicate versions outright.
        if params.text_document.version <= doc.version {
            return false;
        }
        let mut text = doc.text.clone();
        let mut line_index = doc.line_index.clone();
        let mut dirty = false;
        let mut changes = params.content_changes.into_iter().peekable();
        while let Some(change) = changes.next() {
            match change.range {
                Some(range) => {
                    let start_byte = line_index.offset(&text, range.start) as usize;
                    let end_byte = line_index.offset(&text, range.end) as usize;
                    if start_byte > end_byte || end_byte > text.len() {
                        // An invalid range means our state has already
                        // desynced from the client's. Silently skipping
                        // masks the corruption; drop our copy and let the
                        // next full-sync or reopen restore consistency.
                        documents.remove(uri);
                        return false;
                    }
                    text.replace_range(start_byte..end_byte, &change.text);
                    dirty = true;
                }
                None => {
                    text = change.text;
                    dirty = true;
                }
            }
            // Each subsequent change's range is relative to the text this one
            // produced, so refresh the index for the next iteration only. The
            // final index the store keeps is rebuilt by `doc.update` below, so
            // rebuilding it here too would be redundant.
            if changes.peek().is_some() {
                line_index = LineIndex::new(&text);
            }
        }
        if dirty {
            doc.update(text, params.text_document.version);
        }
        true
    }
}

#[tower_lsp::async_trait]
impl<N: Notifier> LanguageServer for Backend<N> {
    async fn initialize(&self, _params: InitializeParams) -> JsonRpcResult<InitializeResult> {
        Ok(InitializeResult {
            capabilities: ServerCapabilities {
                text_document_sync: Some(TextDocumentSyncCapability::Options(
                    TextDocumentSyncOptions {
                        open_close: Some(true),
                        change: Some(TextDocumentSyncKind::INCREMENTAL),
                        save: Some(TextDocumentSyncSaveOptions::SaveOptions(SaveOptions {
                            include_text: Some(false),
                        })),
                        ..Default::default()
                    },
                )),
                document_formatting_provider: Some(OneOf::Left(true)),
                document_range_formatting_provider: Some(OneOf::Left(true)),
                hover_provider: Some(HoverProviderCapability::Simple(true)),
                completion_provider: Some(CompletionOptions {
                    resolve_provider: Some(false),
                    trigger_characters: Some(vec![".".to_string(), ":".to_string()]),
                    ..Default::default()
                }),
                signature_help_provider: Some(SignatureHelpOptions {
                    trigger_characters: Some(vec!["(".to_string(), ",".to_string()]),
                    retrigger_characters: Some(vec![",".to_string()]),
                    work_done_progress_options: WorkDoneProgressOptions::default(),
                }),
                document_symbol_provider: Some(OneOf::Left(true)),
                code_action_provider: Some(CodeActionProviderCapability::Options(
                    CodeActionOptions {
                        code_action_kinds: Some(vec![
                            tower_lsp::lsp_types::CodeActionKind::QUICKFIX,
                            tower_lsp::lsp_types::CodeActionKind::SOURCE_FIX_ALL,
                        ]),
                        resolve_provider: Some(false),
                        work_done_progress_options: WorkDoneProgressOptions::default(),
                    },
                )),
                semantic_tokens_provider: Some(
                    SemanticTokensServerCapabilities::SemanticTokensRegistrationOptions(
                        SemanticTokensRegistrationOptions {
                            text_document_registration_options: TextDocumentRegistrationOptions {
                                document_selector: None,
                            },
                            semantic_tokens_options: SemanticTokensOptions {
                                work_done_progress_options: WorkDoneProgressOptions::default(),
                                legend: semantic_tokens::legend(),
                                range: Some(true),
                                full: Some(SemanticTokensFullOptions::Bool(true)),
                            },
                            static_registration_options: StaticRegistrationOptions::default(),
                        },
                    ),
                ),
                document_highlight_provider: Some(OneOf::Left(true)),
                definition_provider: Some(OneOf::Left(true)),
                references_provider: Some(OneOf::Left(true)),
                rename_provider: Some(OneOf::Right(RenameOptions {
                    prepare_provider: Some(true),
                    work_done_progress_options: WorkDoneProgressOptions::default(),
                })),
                workspace_symbol_provider: Some(OneOf::Left(true)),
                folding_range_provider: Some(FoldingRangeProviderCapability::Simple(true)),
                selection_range_provider: Some(SelectionRangeProviderCapability::Simple(true)),
                document_link_provider: Some(DocumentLinkOptions {
                    resolve_provider: Some(false),
                    work_done_progress_options: WorkDoneProgressOptions::default(),
                }),
                ..Default::default()
            },
            server_info: Some(ServerInfo {
                name: "luck".to_string(),
                version: Some(env!("CARGO_PKG_VERSION").to_string()),
            }),
        })
    }

    async fn initialized(&self, _params: InitializedParams) {
        // Watch config files: without this, ConfigCache entries (including
        // cached "no config here" negatives) lived until server restart -
        // editing or CREATING luck.json changed nothing for open editors.
        let watcher = Registration {
            id: "luck-config-watch".to_string(),
            method: "workspace/didChangeWatchedFiles".to_string(),
            register_options: Some(
                serde_json::to_value(DidChangeWatchedFilesRegistrationOptions {
                    watchers: vec![
                        FileSystemWatcher {
                            glob_pattern: GlobPattern::String("**/luck.json".to_string()),
                            kind: None,
                        },
                        FileSystemWatcher {
                            glob_pattern: GlobPattern::String("**/.editorconfig".to_string()),
                            kind: None,
                        },
                        FileSystemWatcher {
                            glob_pattern: GlobPattern::String("**/.luaurc".to_string()),
                            kind: None,
                        },
                    ],
                })
                .unwrap_or_default(),
            ),
        };
        let _ = self.notifier.register_capability(vec![watcher]).await;
        self.notifier
            .log_message(MessageType::INFO, "luck_lsp initialized".to_string())
            .await;
    }

    async fn did_change_watched_files(&self, _params: DidChangeWatchedFilesParams) {
        self.config_cache.clear();
        // Retarget every open document (the config may have changed its
        // dialect) and republish diagnostics under the new settings.
        let uris: Vec<Url> = {
            let mut documents = self.documents.write().await;
            let uris: Vec<Url> = documents.keys().cloned().collect();
            for uri in &uris {
                let settings = self.project_settings_for(uri);
                let target = self.target_for(uri, &settings);
                if let Some(doc) = documents.get_mut(uri)
                    && doc.target != target
                {
                    let text = doc.text.clone();
                    let version = doc.version;
                    doc.target = target;
                    doc.update(text, version);
                }
            }
            uris
        };
        for uri in uris {
            self.lint_and_publish(uri).await;
        }
    }

    async fn shutdown(&self) -> JsonRpcResult<()> {
        Ok(())
    }

    async fn did_open(&self, params: DidOpenTextDocumentParams) {
        let uri = params.text_document.uri.clone();
        let settings = self.project_settings_for(&uri);
        let target = self.target_for(&uri, &settings);
        let document = DocumentState::new(
            params.text_document.text,
            params.text_document.version,
            target,
        );
        {
            let mut documents = self.documents.write().await;
            documents.insert(uri.clone(), document);
        }
        self.lint_and_publish(uri).await;
    }

    async fn did_change(&self, params: DidChangeTextDocumentParams) {
        let uri = params.text_document.uri.clone();
        let known = self.apply_incremental_change(&uri, params).await;
        if known {
            self.lint_and_publish(uri).await;
        }
    }

    async fn did_save(&self, params: DidSaveTextDocumentParams) {
        let uri = params.text_document.uri.clone();
        // `text` is optional; refresh from it if the client sent it.
        if let Some(text) = params.text {
            let mut documents = self.documents.write().await;
            if let Some(doc) = documents.get_mut(&uri) {
                // didSave is unversioned (the params carry no document version),
                // so the saved on-disk text belongs to the document's CURRENT
                // version - reusing `doc.version` is correct, not stale. Only
                // re-sync when the text actually differs from our in-memory copy.
                if doc.text != text {
                    let version = doc.version;
                    doc.update(text, version);
                }
            }
        }
        self.lint_and_publish(uri).await;
    }

    async fn did_close(&self, params: DidCloseTextDocumentParams) {
        let uri = params.text_document.uri.clone();
        {
            let mut documents = self.documents.write().await;
            documents.remove(&uri);
        }
        // Per LSP spec, clear diagnostics for closed files.
        self.notifier
            .publish_diagnostics(uri, Vec::new(), None)
            .await;
    }

    async fn formatting(
        &self,
        params: DocumentFormattingParams,
    ) -> JsonRpcResult<Option<Vec<TextEdit>>> {
        let uri = params.text_document.uri;
        let doc = {
            let documents = self.documents.read().await;
            documents.get(&uri).cloned()
        };
        let Some(doc) = doc else {
            return Ok(None);
        };
        let settings = self.project_settings_for(&uri);
        let format_options = resolved_format_options(&settings, uri.to_file_path().ok().as_deref());
        let result = luck_formatter::format(&doc.text, doc.target.lua_version(), &format_options);
        if !result.errors.is_empty() {
            let first = &result.errors[0];
            self.notifier
                .show_message(
                    MessageType::WARNING,
                    format!("luck: cannot format file (parse error): {}", first.message),
                )
                .await;
            return Ok(None);
        }
        if result.output == doc.text {
            return Ok(Some(Vec::new()));
        }
        let edit = TextEdit {
            range: doc.line_index.full_document_range(&doc.text),
            new_text: result.output,
        };
        Ok(Some(vec![edit]))
    }

    async fn range_formatting(
        &self,
        params: DocumentRangeFormattingParams,
    ) -> JsonRpcResult<Option<Vec<TextEdit>>> {
        let uri = params.text_document.uri;
        let doc = {
            let documents = self.documents.read().await;
            documents.get(&uri).cloned()
        };
        let Some(doc) = doc else {
            return Ok(None);
        };
        let start_byte = doc.line_index.offset(&doc.text, params.range.start) as usize;
        let end_byte = doc.line_index.offset(&doc.text, params.range.end) as usize;
        if start_byte > end_byte || end_byte > doc.text.len() {
            return Ok(None);
        }
        let settings = self.project_settings_for(&uri);
        let format_options = resolved_format_options(&settings, uri.to_file_path().ok().as_deref());
        let result = luck_formatter::format_range(
            &doc.text,
            doc.target.lua_version(),
            &format_options,
            start_byte..end_byte,
        );
        if !result.errors.is_empty() {
            let first = &result.errors[0];
            self.notifier
                .show_message(
                    MessageType::WARNING,
                    format!(
                        "luck: cannot range-format file (parse error): {}",
                        first.message
                    ),
                )
                .await;
            return Ok(None);
        }
        if result.output == doc.text {
            return Ok(Some(Vec::new()));
        }
        // `format_range` returns the full reformatted file (statements outside
        // the range are emitted verbatim), so we still emit a whole-file edit.
        // The "limit to range" guarantee comes from the formatter itself -
        // text outside the range is preserved byte-for-byte.
        let edit = TextEdit {
            range: doc.line_index.full_document_range(&doc.text),
            new_text: result.output,
        };
        Ok(Some(vec![edit]))
    }

    async fn hover(&self, params: HoverParams) -> JsonRpcResult<Option<Hover>> {
        let uri = params
            .text_document_position_params
            .text_document
            .uri
            .clone();
        let doc = self.snapshot(&uri).await;
        let Some(doc) = doc else {
            return Ok(None);
        };
        Ok(hover::hover(&doc, &params))
    }

    async fn completion(
        &self,
        params: CompletionParams,
    ) -> JsonRpcResult<Option<CompletionResponse>> {
        let uri = params.text_document_position.text_document.uri.clone();
        let doc = self.snapshot(&uri).await;
        let Some(doc) = doc else {
            return Ok(None);
        };
        Ok(completion::completion(&doc, &params))
    }

    async fn signature_help(
        &self,
        params: SignatureHelpParams,
    ) -> JsonRpcResult<Option<SignatureHelp>> {
        let uri = params
            .text_document_position_params
            .text_document
            .uri
            .clone();
        let doc = self.snapshot(&uri).await;
        let Some(doc) = doc else {
            return Ok(None);
        };
        Ok(signature_help::signature_help(&doc, &params))
    }

    async fn document_symbol(
        &self,
        params: DocumentSymbolParams,
    ) -> JsonRpcResult<Option<DocumentSymbolResponse>> {
        let doc = self.snapshot(&params.text_document.uri).await;
        let Some(doc) = doc else {
            return Ok(None);
        };
        Ok(Some(document_symbol::document_symbols(&doc)))
    }

    async fn code_action(
        &self,
        params: CodeActionParams,
    ) -> JsonRpcResult<Option<CodeActionResponse>> {
        let uri = params.text_document.uri.clone();
        let doc = self.snapshot(&uri).await;
        let Some(doc) = doc else {
            return Ok(None);
        };
        let settings = self.project_settings_for(&uri);
        // Serve the lint results computed by the last publish when they
        // match the document version; the provider recomputes otherwise.
        let cached_lints = {
            let lint_cache = self.lint_cache.read().await;
            lint_cache
                .get(&uri)
                .filter(|(version, _)| *version == doc.version)
                .map(|(_, diags)| Arc::clone(diags))
        };
        Ok(Some(code_action::code_action(
            &doc,
            &settings,
            &uri,
            &params,
            cached_lints,
        )))
    }

    async fn semantic_tokens_full(
        &self,
        params: tower_lsp::lsp_types::SemanticTokensParams,
    ) -> JsonRpcResult<Option<SemanticTokensResult>> {
        let doc = self.snapshot(&params.text_document.uri).await;
        let Some(doc) = doc else {
            return Ok(None);
        };
        Ok(Some(semantic_tokens::semantic_tokens(&doc)))
    }

    async fn document_highlight(
        &self,
        params: DocumentHighlightParams,
    ) -> JsonRpcResult<Option<Vec<DocumentHighlight>>> {
        let uri = params
            .text_document_position_params
            .text_document
            .uri
            .clone();
        let doc = self.snapshot(&uri).await;
        let Some(doc) = doc else {
            return Ok(None);
        };
        Ok(Some(highlights::document_highlight(&doc, &params)))
    }

    async fn goto_definition(
        &self,
        params: GotoDefinitionParams,
    ) -> JsonRpcResult<Option<GotoDefinitionResponse>> {
        let uri = params
            .text_document_position_params
            .text_document
            .uri
            .clone();
        let Some(doc) = self.snapshot(&uri).await else {
            return Ok(None);
        };
        let settings = self.project_settings_for(&uri);
        Ok(definition::goto_definition(&doc, &uri, &settings, &params))
    }

    async fn references(&self, params: ReferenceParams) -> JsonRpcResult<Option<Vec<Location>>> {
        let uri = params.text_document_position.text_document.uri.clone();
        let Some(doc) = self.snapshot(&uri).await else {
            return Ok(None);
        };
        Ok(Some(references::references(&doc, &uri, &params)))
    }

    async fn prepare_rename(
        &self,
        params: TextDocumentPositionParams,
    ) -> JsonRpcResult<Option<PrepareRenameResponse>> {
        let uri = params.text_document.uri.clone();
        let Some(doc) = self.snapshot(&uri).await else {
            return Ok(None);
        };
        Ok(rename::prepare_rename(&doc, &params))
    }

    async fn rename(&self, params: RenameParams) -> JsonRpcResult<Option<WorkspaceEdit>> {
        let uri = params.text_document_position.text_document.uri.clone();
        let Some(doc) = self.snapshot(&uri).await else {
            return Ok(None);
        };
        match rename::rename(&doc, &uri, &params) {
            Ok(edit) => Ok(edit),
            Err(message) => Err(tower_lsp::jsonrpc::Error::invalid_params(message)),
        }
    }

    async fn symbol(
        &self,
        params: WorkspaceSymbolParams,
    ) -> JsonRpcResult<Option<Vec<SymbolInformation>>> {
        let documents: Vec<(Url, DocumentState)> = {
            let documents = self.documents.read().await;
            documents
                .iter()
                .map(|(uri, doc)| (uri.clone(), doc.clone()))
                .collect()
        };
        Ok(Some(workspace_symbol::workspace_symbols(
            &documents,
            &params.query,
        )))
    }

    async fn semantic_tokens_range(
        &self,
        params: SemanticTokensRangeParams,
    ) -> JsonRpcResult<Option<SemanticTokensRangeResult>> {
        let Some(doc) = self.snapshot(&params.text_document.uri).await else {
            return Ok(None);
        };
        Ok(Some(semantic_tokens::semantic_tokens_range(
            &doc,
            &params.range,
        )))
    }

    async fn folding_range(
        &self,
        params: FoldingRangeParams,
    ) -> JsonRpcResult<Option<Vec<FoldingRange>>> {
        let doc = self.snapshot(&params.text_document.uri).await;
        let Some(doc) = doc else {
            return Ok(None);
        };
        Ok(Some(folding::folding_ranges(&doc)))
    }

    async fn selection_range(
        &self,
        params: SelectionRangeParams,
    ) -> JsonRpcResult<Option<Vec<SelectionRange>>> {
        let doc = self.snapshot(&params.text_document.uri).await;
        let Some(doc) = doc else {
            return Ok(None);
        };
        Ok(Some(selection_range::selection_ranges(&doc, &params)))
    }

    async fn document_link(
        &self,
        params: DocumentLinkParams,
    ) -> JsonRpcResult<Option<Vec<DocumentLink>>> {
        let uri = params.text_document.uri.clone();
        let doc = self.snapshot(&uri).await;
        let Some(doc) = doc else {
            return Ok(None);
        };
        let settings = self.project_settings_for(&uri);
        Ok(Some(document_link::document_links(&doc, &uri, &settings)))
    }
}

impl<N: Notifier> Backend<N> {
    async fn snapshot(&self, uri: &Url) -> Option<DocumentState> {
        let documents = self.documents.read().await;
        documents.get(uri).cloned()
    }

    /// Handler for the `luck/syntaxTree` custom request - returns a
    /// pretty-printed AST dump for the requested document.
    pub async fn syntax_tree_request(
        &self,
        params: serde_json::Value,
    ) -> JsonRpcResult<serde_json::Value> {
        let uri: Url = params
            .get("textDocument")
            .and_then(|td| td.get("uri"))
            .and_then(|u| u.as_str())
            .and_then(|s| s.parse().ok())
            .ok_or_else(|| tower_lsp::jsonrpc::Error::invalid_params("missing textDocument.uri"))?;
        let doc = self.snapshot(&uri).await;
        let Some(doc) = doc else {
            return Ok(serde_json::Value::String(String::new()));
        };
        Ok(serde_json::Value::String(syntax_tree::syntax_tree(&doc)))
    }

    /// Handler for `luck/fixAllWorkspace` - applies every available fix
    /// across every open document and returns a single WorkspaceEdit.
    pub async fn fix_all_workspace_request(
        &self,
        _params: serde_json::Value,
    ) -> JsonRpcResult<WorkspaceEdit> {
        let documents = self.documents.read().await;
        let edit = code_action::fix_all_open(
            &documents,
            |uri| self.project_settings_for(uri),
            &luck_core::config::LuckConfig::default(),
        );
        Ok(edit)
    }
}

/// A single captured `publishDiagnostics` notification.
pub type PublishedRecord = (Url, Vec<Diagnostic>, Option<i32>);

/// A single captured `showMessage` / `logMessage` notification.
pub type MessageRecord = (MessageType, String);

/// Helper for tests that don't want to wire up an actual `tower_lsp::Client`.
#[derive(Default, Clone)]
pub struct CapturedNotifier {
    pub published: Arc<RwLock<Vec<PublishedRecord>>>,
    pub messages: Arc<RwLock<Vec<MessageRecord>>>,
}

#[async_trait::async_trait]
impl Notifier for CapturedNotifier {
    async fn publish_diagnostics(&self, uri: Url, diags: Vec<Diagnostic>, version: Option<i32>) {
        let mut published = self.published.write().await;
        published.push((uri, diags, version));
    }

    async fn show_message(&self, ty: MessageType, message: String) {
        let mut messages = self.messages.write().await;
        messages.push((ty, message));
    }

    async fn log_message(&self, ty: MessageType, message: String) {
        let mut messages = self.messages.write().await;
        messages.push((ty, message));
    }
}

impl CapturedNotifier {
    pub async fn diagnostics_for(&self, uri: &Url) -> Vec<Diagnostic> {
        let published = self.published.read().await;
        published
            .iter()
            .rev()
            .find(|(other, _, _)| other == uri)
            .map(|(_, diags, _)| diags.clone())
            .unwrap_or_default()
    }
}

/// Compute the byte length of a text edit's replacement, for tests that want
/// to verify a range edit didn't touch the whole document.
#[must_use]
pub fn edit_replacement_span_lines(edits: &[TextEdit]) -> (Position, Position) {
    let start = edits
        .iter()
        .map(|edit| edit.range.start)
        .min_by(|a, b| (a.line, a.character).cmp(&(b.line, b.character)))
        .unwrap_or(Position {
            line: 0,
            character: 0,
        });
    let end = edits
        .iter()
        .map(|edit| edit.range.end)
        .max_by(|a, b| (a.line, a.character).cmp(&(b.line, b.character)))
        .unwrap_or(Position {
            line: 0,
            character: 0,
        });
    (start, end)
}

/// Build the LSP `LspService` over the real `tower_lsp::Client` notifier.
/// Registers two custom requests:
///   - `luck/syntaxTree` -> AST debug dump for the requested document
///   - `luck/fixAllWorkspace` -> server-computed WorkspaceEdit applying
///     every available fix across all open documents
#[must_use]
pub fn build_service() -> (
    tower_lsp::LspService<Backend<Client>>,
    tower_lsp::ClientSocket,
) {
    tower_lsp::LspService::build(Backend::new)
        .custom_method("luck/syntaxTree", Backend::<Client>::syntax_tree_request)
        .custom_method(
            "luck/fixAllWorkspace",
            Backend::<Client>::fix_all_workspace_request,
        )
        .finish()
}
