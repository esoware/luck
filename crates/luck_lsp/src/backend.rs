//! Open documents live in memory and take incremental changes, so the server
//! never reads an open file from disk.

use alloc::collections::BTreeMap;
use alloc::sync::Arc;
use core::future::ready;

use luck_core::LuaTarget;
use luck_linter::LintDiagnostic;
use rustc_hash::FxHashSet;
use tokio::sync::{Mutex, RwLock};
use tokio::task::{spawn_blocking, yield_now};
use tower_lsp_server::jsonrpc::{Error as JsonRpcError, Result as JsonRpcResult};
use tower_lsp_server::ls_types::{
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
    StaticRegistrationOptions, TextDocumentPositionParams, TextDocumentRegistrationOptions,
    TextDocumentSyncCapability, TextDocumentSyncKind, TextDocumentSyncOptions,
    TextDocumentSyncSaveOptions, TextEdit, Uri, WorkDoneProgressOptions, WorkspaceEdit,
    WorkspaceSymbolParams, WorkspaceSymbolResponse,
};
use tower_lsp_server::{Client, LanguageServer};

use crate::document::{DocumentState, file_path};
use crate::providers::{
    code_action, completion, definition, diagnostics, document_highlight, document_link,
    document_symbol, folding_range, hover, references, rename, selection_range, semantic_tokens,
    signature_help, syntax_tree, workspace_symbol,
};
use crate::settings::{ProjectSettings, SettingsCache};

/// The parts of `tower_lsp_server::Client` the backend uses, so tests can
/// record what the server sends instead.
pub trait Notifier: Send + Sync + 'static {
    fn publish_diagnostics(
        &self,
        uri: Uri,
        diags: Vec<Diagnostic>,
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
    async fn publish_diagnostics(&self, uri: Uri, diags: Vec<Diagnostic>, version: Option<i32>) {
        Self::publish_diagnostics(self, uri, diags, version).await;
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

#[derive(Debug)]
pub struct Backend<N: Notifier = Client> {
    notifier: N,
    documents: RwLock<BTreeMap<Uri, OpenDocument>>,
    /// Orders what changes the open documents and what tells the client about
    /// them. tower-lsp-server polls notifications concurrently, but this
    /// lock queues them in arrival order: an edit can't overtake the open it
    /// follows, and a publish can't land after what replaced or closed its
    /// document.
    sync: Mutex<()>,
    settings_cache: SettingsCache,
}

impl<N: Notifier> Backend<N> {
    pub fn new(notifier: N) -> Self {
        Self {
            notifier,
            documents: RwLock::new(BTreeMap::new()),
            sync: Mutex::new(()),
            settings_cache: SettingsCache::default(),
        }
    }

    /// Makes `state` what's open at `uri`, which leaves any lint still
    /// running on what it replaces with nothing to publish.
    async fn store(&self, uri: &Uri, state: DocumentState) -> Arc<DocumentState> {
        let shared = Arc::new(state);
        let open = OpenDocument { state: Arc::clone(&shared), published_lints: None };
        self.documents.write().await.insert(uri.clone(), open);
        shared
    }

    async fn snapshot(&self, uri: &Uri) -> Option<Arc<DocumentState>> {
        let documents = self.documents.read().await;
        documents.get(uri).map(|open| Arc::clone(&open.state))
    }

    async fn open_documents(&self) -> Vec<(Uri, Arc<DocumentState>)> {
        let documents = self.documents.read().await;
        documents.iter().map(|(uri, open)| (uri.clone(), Arc::clone(&open.state))).collect()
    }

    async fn document_settings(&self, uri: &Uri) -> (LuaTarget, FxHashSet<String>) {
        let document_path = file_path(uri);
        let settings = self.settings_cache.settings_for(document_path.as_deref()).await;
        (
            settings.target_for(document_path.as_deref()),
            settings.lint_config.extra_globals.iter().cloned().collect(),
        )
    }

    /// The settings of the document at `uri`, or `None` when the project's
    /// include/exclude set leaves it out, so it gets no diagnostics and no
    /// fixes.
    async fn settings_if_included(&self, uri: &Uri) -> Option<Arc<ProjectSettings>> {
        let document_path = file_path(uri);
        let settings = self.settings_cache.settings_for(document_path.as_deref()).await;
        let Some(path) = document_path else {
            return Some(settings);
        };
        let filter_settings = Arc::clone(&settings);
        // `is_included` canonicalizes the path, which touches the disk. A
        // check that dies counts as excluded, so no fix lands on a guess.
        spawn_blocking(move || filter_settings.filter.is_included(&path))
            .await
            .unwrap_or(false)
            .then_some(settings)
    }

    /// Lints `document` and publishes the result, unless `document` has been
    /// replaced or closed by the time the result is ready.
    async fn lint_and_publish(&self, uri: Uri, document: Arc<DocumentState>) {
        // A change already queued behind this one replaces `document`, which
        // saves linting it, so a burst of keystrokes lints once.
        yield_now().await;
        if !self.is_open(&uri, &document).await {
            return;
        }
        let lints: Arc<[LintDiagnostic]> = match self.settings_if_included(&uri).await {
            Some(settings) => lint_off_the_serve_loop(&document, settings).await,
            None => Vec::new().into(),
        };
        let diags = diagnostics::to_lsp(&document, &lints);
        // Through the send, so nothing newer can reach the client before it.
        let _ordered = self.sync.lock().await;
        if !self.is_open(&uri, &document).await {
            return;
        }
        if let Some(open) = self.documents.write().await.get_mut(&uri) {
            open.published_lints = Some(lints);
        }
        self.notifier.publish_diagnostics(uri, diags, Some(document.version)).await;
    }

    async fn is_open(&self, uri: &Uri, document: &Arc<DocumentState>) -> bool {
        self.documents.read().await.get(uri).is_some_and(|open| Arc::ptr_eq(&open.state, document))
    }

    /// Formats the document at `uri`, or only `range` of it, as one
    /// whole-document edit. `None` when it isn't open, `range` doesn't fit
    /// it, or it doesn't parse, which also shows a warning.
    async fn format_edits(&self, uri: &Uri, range: Option<Range>) -> Option<Vec<TextEdit>> {
        let document = self.snapshot(uri).await?;
        let byte_range = match range {
            Some(requested) => Some(document.byte_range(requested)?),
            None => None,
        };
        let document_path = file_path(uri);
        let settings = self.settings_cache.settings_for(document_path.as_deref()).await;
        // Reading the `.editorconfig` chain touches the disk.
        let options =
            spawn_blocking(move || settings.format_options(document_path.as_deref())).await.ok()?;
        let lua_version = document.lua_version;
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

    /// # Errors
    ///
    /// Returns an invalid-params error when `params` carries no parseable
    /// `textDocument.uri`.
    pub async fn syntax_tree_request(
        &self,
        params: serde_json::Value,
    ) -> JsonRpcResult<serde_json::Value> {
        let uri: Uri = params
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

    /// # Errors
    ///
    /// Never fails; `custom_method` requires a `Result`.
    pub async fn fix_all_workspace_request(
        &self,
        _params: serde_json::Value,
    ) -> JsonRpcResult<WorkspaceEdit> {
        let mut documents = Vec::new();
        for (uri, document) in self.open_documents().await {
            if let Some(settings) = self.settings_if_included(&uri).await {
                documents.push((uri, document, settings));
            }
        }
        Ok(code_action::fix_all_open(&documents))
    }

    /// Folds a `didChange` batch into the store. Returns the new state when
    /// the document is still open and the batch was current, so diagnostics
    /// should be republished.
    async fn apply_change(
        &self,
        params: DidChangeTextDocumentParams,
    ) -> Option<Arc<DocumentState>> {
        let uri = &params.text_document.uri;
        let current = self.snapshot(uri).await?;
        // A repeated or older batch's ranges don't fit the current text, and
        // applying them would corrupt it.
        let version = params.text_document.version;
        if version <= current.version {
            return None;
        }
        let Some(text) = current.changed_text(params.content_changes) else {
            // A range that doesn't fit means we've already desynced from the
            // client. Drop the document so nothing is served from corrupt
            // text until the client reopens it.
            self.documents.write().await.remove(uri);
            return None;
        };
        Some(self.store(uri, current.with_text(text, version)).await)
    }
}

/// `document`'s lints, computed on the blocking pool: tower-lsp-server polls
/// every handler on one task, so a lint run inline would stall the whole
/// server. A lint that dies yields nothing.
async fn lint_off_the_serve_loop(
    document: &Arc<DocumentState>,
    settings: Arc<ProjectSettings>,
) -> Arc<[LintDiagnostic]> {
    let linted = Arc::clone(document);
    spawn_blocking(move || linted.lint(&settings.lint_config)).await.unwrap_or_default().into()
}

impl<N: Notifier> LanguageServer for Backend<N> {
    fn initialize(
        &self,
        _params: InitializeParams,
    ) -> impl Future<Output = JsonRpcResult<InitializeResult>> {
        ready(Ok(InitializeResult {
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
                            code_action::FIX_ALL_KIND,
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
            offset_encoding: None,
        }))
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
        // Rebuild every open document under the new settings, whether or not
        // its dialect or extra globals changed, so a lint running under the
        // old settings has nothing to publish. Each document's settings are
        // found before taking the sync lock, so config discovery doesn't
        // stall edits.
        let uris: Vec<Uri> = self.documents.read().await.keys().cloned().collect();
        for uri in uris {
            let (target, extra_globals) = self.document_settings(&uri).await;
            let document = {
                let _ordered = self.sync.lock().await;
                let Some(current) = self.snapshot(&uri).await else {
                    continue;
                };
                let state = DocumentState::new(
                    current.text().to_owned(),
                    current.version,
                    target,
                    extra_globals,
                );
                self.store(&uri, state).await
            };
            self.lint_and_publish(uri, document).await;
        }
    }

    fn shutdown(&self) -> impl Future<Output = JsonRpcResult<()>> {
        ready(Ok(()))
    }

    async fn did_open(&self, params: DidOpenTextDocumentParams) {
        let uri = params.text_document.uri;
        let document = {
            // Held across settings discovery, so a change arriving during the
            // open waits for the document it edits.
            let _ordered = self.sync.lock().await;
            let (target, extra_globals) = self.document_settings(&uri).await;
            let state = DocumentState::new(
                params.text_document.text,
                params.text_document.version,
                target,
                extra_globals,
            );
            self.store(&uri, state).await
        };
        self.lint_and_publish(uri, document).await;
    }

    async fn did_change(&self, params: DidChangeTextDocumentParams) {
        let uri = params.text_document.uri.clone();
        let updated = {
            let _ordered = self.sync.lock().await;
            self.apply_change(params).await
        };
        if let Some(document) = updated {
            self.lint_and_publish(uri, document).await;
        }
    }

    async fn did_save(&self, params: DidSaveTextDocumentParams) {
        let uri = params.text_document.uri;
        let document = {
            let _ordered = self.sync.lock().await;
            let Some(current) = self.snapshot(&uri).await else {
                return;
            };
            // We register with `include_text: false`, but a client may send
            // the text anyway. didSave carries no version, and the saved
            // text belongs to the current one.
            if let Some(text) = params.text
                && current.text() != text
            {
                self.store(&uri, current.with_text(text, current.version)).await
            } else {
                current
            }
        };
        self.lint_and_publish(uri, document).await;
    }

    async fn did_close(&self, params: DidCloseTextDocumentParams) {
        let uri = params.text_document.uri;
        let _ordered = self.sync.lock().await;
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
        // Without published lints, they're recomputed through the same
        // include/exclude filter and opt-in gate as publishing, so no action
        // fixes a diagnostic the user wasn't shown.
        let lints = if let Some(lints) = published_lints {
            lints
        } else if let Some(settings) = self.settings_if_included(uri).await {
            lint_off_the_serve_loop(&document, settings).await
        } else {
            Vec::new().into()
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
        let settings = self.settings_cache.settings_for(file_path(uri).as_deref()).await;
        // A `require` under the cursor resolves through the disk.
        Ok(spawn_blocking(move || definition::goto(&document, &settings, &params))
            .await
            .ok()
            .flatten())
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
    ) -> JsonRpcResult<Option<WorkspaceSymbolResponse>> {
        let documents = self.open_documents().await;
        Ok(Some(WorkspaceSymbolResponse::Flat(workspace_symbol::workspace_symbols(
            &documents,
            &params.query,
        ))))
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
        let uri = params.text_document.uri;
        let Some(document) = self.snapshot(&uri).await else {
            return Ok(None);
        };
        let settings = self.settings_cache.settings_for(file_path(&uri).as_deref()).await;
        // Each `require` resolves through the disk.
        let links =
            spawn_blocking(move || document_link::document_links(&document, &uri, &settings))
                .await
                .unwrap_or_default();
        Ok(Some(links))
    }
}
