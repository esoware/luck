//! The `tower_lsp::LanguageServer` implementation.
//!
//! Open documents live in memory and take incremental changes, so the server
//! never reads an open file from disk.

use alloc::collections::BTreeMap;
use alloc::sync::Arc;

use luck_core::LuaTarget;
use luck_linter::LintDiagnostic;
use tokio::sync::RwLock;
use tower_lsp::jsonrpc::{Error as JsonRpcError, Result as JsonRpcResult};
use tower_lsp::lsp_types::{
    CodeActionKind, CodeActionOptions, CodeActionParams, CodeActionProviderCapability,
    CodeActionResponse, CompletionOptions, CompletionParams, CompletionResponse, Diagnostic,
    DidChangeTextDocumentParams, DidChangeWatchedFilesParams,
    DidChangeWatchedFilesRegistrationOptions, DidCloseTextDocumentParams,
    DidOpenTextDocumentParams, DidSaveTextDocumentParams, DocumentFormattingParams,
    DocumentHighlight, DocumentHighlightParams, DocumentLink, DocumentLinkOptions,
    DocumentLinkParams, DocumentRangeFormattingParams, DocumentSymbolParams,
    DocumentSymbolResponse, FileSystemWatcher, FoldingRange, FoldingRangeParams,
    FoldingRangeProviderCapability, GlobPattern, GotoDefinitionParams, GotoDefinitionResponse,
    Hover, HoverParams, HoverProviderCapability, InitializeParams, InitializeResult,
    InitializedParams, Location, MessageType, OneOf, PrepareRenameResponse, Range, ReferenceParams,
    Registration, RenameOptions, RenameParams, SaveOptions, SelectionRange, SelectionRangeParams,
    SelectionRangeProviderCapability, SemanticTokensFullOptions, SemanticTokensOptions,
    SemanticTokensParams, SemanticTokensRangeParams, SemanticTokensRangeResult,
    SemanticTokensRegistrationOptions, SemanticTokensResult, SemanticTokensServerCapabilities,
    ServerCapabilities, ServerInfo, SignatureHelp, SignatureHelpOptions, SignatureHelpParams,
    StaticRegistrationOptions, SymbolInformation, TextDocumentPositionParams,
    TextDocumentRegistrationOptions, TextDocumentSyncCapability, TextDocumentSyncKind,
    TextDocumentSyncOptions, TextDocumentSyncSaveOptions, TextEdit, Url, WorkDoneProgressOptions,
    WorkspaceEdit, WorkspaceSymbolParams,
};
use tower_lsp::{Client, LanguageServer};

use crate::document::DocumentState;
use crate::providers::{
    code_action, completion, definition, diagnostics, document_highlight, document_link,
    document_symbol, folding_range, hover, references, rename, selection_range, semantic_tokens,
    signature_help, syntax_tree, workspace_symbol,
};
use crate::settings::SettingsCache;

/// The parts of `tower_lsp::Client` the backend uses, so tests can record
/// what the server sends instead.
pub trait Notifier: Send + Sync + 'static {
    fn publish_diagnostics(
        &self,
        uri: Url,
        diagnostics: Vec<Diagnostic>,
        version: Option<i32>,
    ) -> impl Future<Output = ()> + Send;
    fn show_message(
        &self,
        message_type: MessageType,
        message: String,
    ) -> impl Future<Output = ()> + Send;
    fn log_message(
        &self,
        message_type: MessageType,
        message: String,
    ) -> impl Future<Output = ()> + Send;
    fn register_capability(
        &self,
        registrations: Vec<Registration>,
    ) -> impl Future<Output = ()> + Send;
}

impl Notifier for Client {
    async fn publish_diagnostics(
        &self,
        uri: Url,
        diagnostics: Vec<Diagnostic>,
        version: Option<i32>,
    ) {
        Self::publish_diagnostics(self, uri, diagnostics, version).await;
    }

    async fn show_message(&self, message_type: MessageType, message: String) {
        Self::show_message(self, message_type, message).await;
    }

    async fn log_message(&self, message_type: MessageType, message: String) {
        Self::log_message(self, message_type, message).await;
    }

    async fn register_capability(&self, registrations: Vec<Registration>) {
        let _unused = Self::register_capability(self, registrations).await;
    }
}

#[derive(Debug)]
struct OpenDocument {
    state: Arc<DocumentState>,
    /// The lints last published for `state`. VS Code asks for code actions
    /// on nearly every cursor move, and without this each request would
    /// rerun every rule.
    published_lints: Option<Arc<[LintDiagnostic]>>,
}

impl OpenDocument {
    fn new(text: String, version: i32, target: LuaTarget) -> Self {
        Self { state: Arc::new(DocumentState::new(text, version, target)), published_lints: None }
    }
}

/// The LSP server state shared across requests.
#[derive(Debug)]
pub struct Backend<N: Notifier = Client> {
    notifier: N,
    documents: RwLock<BTreeMap<Url, OpenDocument>>,
    settings_cache: SettingsCache,
}

impl<N: Notifier> Backend<N> {
    pub fn new(notifier: N) -> Self {
        Self {
            notifier,
            documents: RwLock::new(BTreeMap::new()),
            settings_cache: SettingsCache::default(),
        }
    }

    async fn snapshot(&self, uri: &Url) -> Option<Arc<DocumentState>> {
        let documents = self.documents.read().await;
        documents.get(uri).map(|open| Arc::clone(&open.state))
    }

    async fn open_documents(&self) -> Vec<(Url, Arc<DocumentState>)> {
        let documents = self.documents.read().await;
        documents.iter().map(|(uri, open)| (uri.clone(), Arc::clone(&open.state))).collect()
    }

    async fn target_for(&self, uri: &Url) -> LuaTarget {
        let file_path = uri.to_file_path().ok();
        self.settings_cache
            .settings_for(file_path.as_deref())
            .await
            .target_for(file_path.as_deref())
    }

    async fn lint_and_publish(&self, uri: Url) {
        let Some(document) = self.snapshot(&uri).await else {
            return;
        };
        let file_path = uri.to_file_path().ok();
        let settings = self.settings_cache.settings_for(file_path.as_deref()).await;
        // Files outside the project's include/exclude set get no diagnostics.
        if let Some(path) = &file_path
            && !settings.filter.is_included(path)
        {
            self.notifier.publish_diagnostics(uri, Vec::new(), Some(document.version)).await;
            return;
        }
        let lints: Arc<[LintDiagnostic]> = document.lint(&settings.lint_config).into();
        {
            let mut documents = self.documents.write().await;
            // A change that landed while linting has already replaced the
            // state these lints belong to.
            if let Some(open) = documents.get_mut(&uri)
                && Arc::ptr_eq(&open.state, &document)
            {
                open.published_lints = Some(Arc::clone(&lints));
            }
        }
        let diagnostics = diagnostics::to_lsp(&document, &lints);
        self.notifier.publish_diagnostics(uri, diagnostics, Some(document.version)).await;
    }

    /// Formats the document at `uri`, or only `range` of it, as one
    /// whole-document edit. `None` when it isn't open, `range` doesn't fit
    /// it, or it doesn't parse, which also shows a warning.
    async fn format_edits(&self, uri: &Url, range: Option<Range>) -> Option<Vec<TextEdit>> {
        let document = self.snapshot(uri).await?;
        let byte_range = match range {
            Some(requested) => Some(document.byte_range(requested)?),
            None => None,
        };
        let file_path = uri.to_file_path().ok();
        let settings = self.settings_cache.settings_for(file_path.as_deref()).await;
        let options = settings.format_options(file_path.as_deref());
        let lua_version = document.target.lua_version();
        let (result, action) = byte_range.map_or_else(
            || (luck_formatter::format(document.text(), lua_version, &options), "format"),
            // `format_range` returns the whole file with text outside the
            // range kept byte-for-byte, so a whole-file edit only changes the
            // range.
            |bytes| {
                (
                    luck_formatter::format_range(document.text(), lua_version, &options, bytes),
                    "range-format",
                )
            },
        );
        if let Some(first) = result.errors.first() {
            self.notifier
                .show_message(
                    MessageType::WARNING,
                    format!("luck: can't {action} file (parse error): {}", first.message),
                )
                .await;
            return None;
        }
        if result.output == document.text() {
            return Some(Vec::new());
        }
        Some(vec![TextEdit { range: document.full_range(), new_text: result.output }])
    }

    /// Serves `luck/syntaxTree` with a pretty-printed AST dump of the
    /// requested document, or an empty string when it isn't open.
    ///
    /// # Errors
    ///
    /// Returns an invalid-params error when `params` carries no parseable
    /// `textDocument.uri`.
    pub async fn syntax_tree_request(
        &self,
        params: serde_json::Value,
    ) -> JsonRpcResult<serde_json::Value> {
        let uri: Url = params
            .get("textDocument")
            .and_then(|text_document| text_document.get("uri"))
            .and_then(|uri| uri.as_str())
            .and_then(|uri| uri.parse().ok())
            .ok_or_else(|| JsonRpcError::invalid_params("missing `textDocument.uri`"))?;
        let Some(document) = self.snapshot(&uri).await else {
            return Ok(serde_json::Value::String(String::new()));
        };
        Ok(serde_json::Value::String(syntax_tree::syntax_tree(&document)))
    }

    /// Serves `luck/fixAllWorkspace` with one `WorkspaceEdit` holding every
    /// available fix in every open document, for the client to apply.
    ///
    /// # Errors
    ///
    /// Never fails; `custom_method` requires a `Result`.
    pub async fn fix_all_workspace_request(
        &self,
        _params: serde_json::Value,
    ) -> JsonRpcResult<WorkspaceEdit> {
        let mut documents = Vec::new();
        for (uri, document) in self.open_documents().await {
            let settings =
                self.settings_cache.settings_for(uri.to_file_path().ok().as_deref()).await;
            documents.push((uri, document, settings));
        }
        Ok(code_action::fix_all_open(&documents))
    }
}

/// Folds a `didChange` batch into the store. Returns whether the document is
/// still open and the batch was current, so diagnostics should be republished.
fn update_stored_document(
    documents: &mut BTreeMap<Url, OpenDocument>,
    params: DidChangeTextDocumentParams,
) -> bool {
    let uri = &params.text_document.uri;
    let Some(open) = documents.get_mut(uri) else {
        return false;
    };
    // tower-lsp handles notifications concurrently, so batches can arrive out
    // of order. A stale or duplicate batch's ranges don't fit the current
    // text, and applying them would corrupt it.
    let version = params.text_document.version;
    if version <= open.state.version {
        return false;
    }
    if params.content_changes.is_empty() {
        return true;
    }
    let Some(text) = open.state.changed_text(params.content_changes) else {
        // A range that doesn't fit means we've already desynced from the
        // client. Drop the document so nothing is served from corrupt text
        // until the client reopens it.
        documents.remove(uri);
        return false;
    };
    *open = OpenDocument::new(text, version, open.state.target);
    true
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
                    trigger_characters: Some(vec![".".to_owned(), ":".to_owned()]),
                    ..Default::default()
                }),
                signature_help_provider: Some(SignatureHelpOptions {
                    trigger_characters: Some(vec!["(".to_owned(), ",".to_owned()]),
                    retrigger_characters: Some(vec![",".to_owned()]),
                    work_done_progress_options: WorkDoneProgressOptions::default(),
                }),
                document_symbol_provider: Some(OneOf::Left(true)),
                code_action_provider: Some(CodeActionProviderCapability::Options(
                    CodeActionOptions {
                        code_action_kinds: Some(vec![
                            CodeActionKind::QUICKFIX,
                            CodeActionKind::SOURCE_FIX_ALL,
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
                name: "luck".to_owned(),
                version: Some(env!("CARGO_PKG_VERSION").to_owned()),
            }),
        })
    }

    async fn initialized(&self, _params: InitializedParams) {
        // Without these watchers, cached settings (cached misses included)
        // would live until a restart, so creating or editing a config file
        // would do nothing for open documents.
        let watcher = Registration {
            id: "luck-config-watch".to_owned(),
            method: "workspace/didChangeWatchedFiles".to_owned(),
            register_options: Some(
                serde_json::to_value(DidChangeWatchedFilesRegistrationOptions {
                    watchers: vec![
                        FileSystemWatcher {
                            glob_pattern: GlobPattern::String("**/luck.json".to_owned()),
                            kind: None,
                        },
                        FileSystemWatcher {
                            glob_pattern: GlobPattern::String("**/.editorconfig".to_owned()),
                            kind: None,
                        },
                        FileSystemWatcher {
                            glob_pattern: GlobPattern::String("**/.luaurc".to_owned()),
                            kind: None,
                        },
                    ],
                })
                .unwrap_or_default(),
            ),
        };
        let () = self.notifier.register_capability(vec![watcher]).await;
        self.notifier.log_message(MessageType::INFO, "luck_lsp initialized".to_owned()).await;
    }

    async fn did_change_watched_files(&self, _params: DidChangeWatchedFilesParams) {
        self.settings_cache.clear().await;
        // Retarget every open document (the config may have changed its
        // dialect) and republish diagnostics under the new settings. Each
        // target is found before taking the documents lock, so config
        // discovery doesn't stall other requests.
        let uris: Vec<Url> = self.documents.read().await.keys().cloned().collect();
        for uri in &uris {
            let target = self.target_for(uri).await;
            let mut documents = self.documents.write().await;
            if let Some(open) = documents.get_mut(uri)
                && open.state.target != target
            {
                *open = OpenDocument::new(open.state.text().to_owned(), open.state.version, target);
            }
        }
        for uri in uris {
            self.lint_and_publish(uri).await;
        }
    }

    async fn shutdown(&self) -> JsonRpcResult<()> {
        Ok(())
    }

    async fn did_open(&self, params: DidOpenTextDocumentParams) {
        let uri = params.text_document.uri;
        let target = self.target_for(&uri).await;
        let document =
            OpenDocument::new(params.text_document.text, params.text_document.version, target);
        self.documents.write().await.insert(uri.clone(), document);
        self.lint_and_publish(uri).await;
    }

    async fn did_change(&self, params: DidChangeTextDocumentParams) {
        let uri = params.text_document.uri.clone();
        let is_current = update_stored_document(&mut *self.documents.write().await, params);
        if is_current {
            self.lint_and_publish(uri).await;
        }
    }

    async fn did_save(&self, params: DidSaveTextDocumentParams) {
        let uri = params.text_document.uri;
        // We register with `include_text: false`, but a client may send the
        // text anyway.
        if let Some(text) = params.text {
            let mut documents = self.documents.write().await;
            if let Some(open) = documents.get_mut(&uri)
                // didSave carries no version, and the saved text belongs to
                // the current one.
                && open.state.text() != text
            {
                *open = OpenDocument::new(text, open.state.version, open.state.target);
            }
        }
        self.lint_and_publish(uri).await;
    }

    async fn did_close(&self, params: DidCloseTextDocumentParams) {
        let uri = params.text_document.uri;
        self.documents.write().await.remove(&uri);
        // Per LSP spec, clear diagnostics for closed files.
        self.notifier.publish_diagnostics(uri, Vec::new(), None).await;
    }

    async fn formatting(
        &self,
        params: DocumentFormattingParams,
    ) -> JsonRpcResult<Option<Vec<TextEdit>>> {
        Ok(self.format_edits(&params.text_document.uri, None).await)
    }

    async fn range_formatting(
        &self,
        params: DocumentRangeFormattingParams,
    ) -> JsonRpcResult<Option<Vec<TextEdit>>> {
        Ok(self.format_edits(&params.text_document.uri, Some(params.range)).await)
    }

    async fn hover(&self, params: HoverParams) -> JsonRpcResult<Option<Hover>> {
        let text_document_position = &params.text_document_position_params;
        let Some(document) = self.snapshot(&text_document_position.text_document.uri).await else {
            return Ok(None);
        };
        Ok(hover::hover(&document, text_document_position.position))
    }

    async fn completion(
        &self,
        params: CompletionParams,
    ) -> JsonRpcResult<Option<CompletionResponse>> {
        let text_document_position = &params.text_document_position;
        let Some(document) = self.snapshot(&text_document_position.text_document.uri).await else {
            return Ok(None);
        };
        Ok(Some(completion::completion(&document, text_document_position.position)))
    }

    async fn signature_help(
        &self,
        params: SignatureHelpParams,
    ) -> JsonRpcResult<Option<SignatureHelp>> {
        let text_document_position = &params.text_document_position_params;
        let Some(document) = self.snapshot(&text_document_position.text_document.uri).await else {
            return Ok(None);
        };
        Ok(signature_help::signature_help(&document, text_document_position.position))
    }

    async fn document_symbol(
        &self,
        params: DocumentSymbolParams,
    ) -> JsonRpcResult<Option<DocumentSymbolResponse>> {
        let Some(document) = self.snapshot(&params.text_document.uri).await else {
            return Ok(None);
        };
        Ok(Some(DocumentSymbolResponse::Nested(document_symbol::document_symbols(&document))))
    }

    async fn code_action(
        &self,
        params: CodeActionParams,
    ) -> JsonRpcResult<Option<CodeActionResponse>> {
        let uri = &params.text_document.uri;
        let Some((document, published_lints)) = self
            .documents
            .read()
            .await
            .get(uri)
            .map(|open| (Arc::clone(&open.state), open.published_lints.clone()))
        else {
            return Ok(None);
        };
        // Without published lints, they're recomputed through the same opt-in
        // gate as publishing, so no action fixes a diagnostic the user wasn't
        // shown.
        let lints = if let Some(lints) = published_lints {
            lints
        } else {
            let settings =
                self.settings_cache.settings_for(uri.to_file_path().ok().as_deref()).await;
            document.lint(&settings.lint_config).into()
        };
        Ok(Some(code_action::code_action(&document, &params, &lints)))
    }

    async fn semantic_tokens_full(
        &self,
        params: SemanticTokensParams,
    ) -> JsonRpcResult<Option<SemanticTokensResult>> {
        let Some(document) = self.snapshot(&params.text_document.uri).await else {
            return Ok(None);
        };
        Ok(Some(semantic_tokens::semantic_tokens(&document)))
    }

    async fn document_highlight(
        &self,
        params: DocumentHighlightParams,
    ) -> JsonRpcResult<Option<Vec<DocumentHighlight>>> {
        let text_document_position = &params.text_document_position_params;
        let Some(document) = self.snapshot(&text_document_position.text_document.uri).await else {
            return Ok(None);
        };
        Ok(Some(document_highlight::document_highlights(
            &document,
            text_document_position.position,
        )))
    }

    async fn goto_definition(
        &self,
        params: GotoDefinitionParams,
    ) -> JsonRpcResult<Option<GotoDefinitionResponse>> {
        let uri = &params.text_document_position_params.text_document.uri;
        let Some(document) = self.snapshot(uri).await else {
            return Ok(None);
        };
        let settings = self.settings_cache.settings_for(uri.to_file_path().ok().as_deref()).await;
        Ok(definition::goto(&document, &settings, &params))
    }

    async fn references(&self, params: ReferenceParams) -> JsonRpcResult<Option<Vec<Location>>> {
        let Some(document) = self.snapshot(&params.text_document_position.text_document.uri).await
        else {
            return Ok(None);
        };
        Ok(Some(references::references(&document, &params)))
    }

    async fn prepare_rename(
        &self,
        params: TextDocumentPositionParams,
    ) -> JsonRpcResult<Option<PrepareRenameResponse>> {
        let Some(document) = self.snapshot(&params.text_document.uri).await else {
            return Ok(None);
        };
        Ok(rename::prepare(&document, params.position))
    }

    async fn rename(&self, params: RenameParams) -> JsonRpcResult<Option<WorkspaceEdit>> {
        let Some(document) = self.snapshot(&params.text_document_position.text_document.uri).await
        else {
            return Ok(None);
        };
        rename::rename(&document, &params)
    }

    async fn symbol(
        &self,
        params: WorkspaceSymbolParams,
    ) -> JsonRpcResult<Option<Vec<SymbolInformation>>> {
        let documents = self.open_documents().await;
        Ok(Some(workspace_symbol::workspace_symbols(&documents, &params.query)))
    }

    async fn semantic_tokens_range(
        &self,
        params: SemanticTokensRangeParams,
    ) -> JsonRpcResult<Option<SemanticTokensRangeResult>> {
        let Some(document) = self.snapshot(&params.text_document.uri).await else {
            return Ok(None);
        };
        Ok(Some(semantic_tokens::in_range(&document, params.range)))
    }

    async fn folding_range(
        &self,
        params: FoldingRangeParams,
    ) -> JsonRpcResult<Option<Vec<FoldingRange>>> {
        let Some(document) = self.snapshot(&params.text_document.uri).await else {
            return Ok(None);
        };
        Ok(Some(folding_range::folding_ranges(&document)))
    }

    async fn selection_range(
        &self,
        params: SelectionRangeParams,
    ) -> JsonRpcResult<Option<Vec<SelectionRange>>> {
        let Some(document) = self.snapshot(&params.text_document.uri).await else {
            return Ok(None);
        };
        Ok(Some(selection_range::selection_ranges(&document, &params.positions)))
    }

    async fn document_link(
        &self,
        params: DocumentLinkParams,
    ) -> JsonRpcResult<Option<Vec<DocumentLink>>> {
        let uri = &params.text_document.uri;
        let Some(document) = self.snapshot(uri).await else {
            return Ok(None);
        };
        let settings = self.settings_cache.settings_for(uri.to_file_path().ok().as_deref()).await;
        Ok(Some(document_link::document_links(&document, uri, &settings)))
    }
}
