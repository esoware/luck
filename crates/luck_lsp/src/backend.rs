//! Open documents live in memory and take incremental changes, so the server
//! never reads an open file from disk.

use alloc::collections::BTreeMap;
use alloc::sync::Arc;
use core::future::ready;

use luck_allocator::Allocator;
use luck_linter::LintDiagnostic;
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

use crate::document::{DocumentSettings, DocumentState, OpenDocument, file_path};
use crate::providers::{
    code_action, completion, definition, diagnostics, document_highlight, document_link,
    document_symbol, folding_range, hover, references, rename, selection_range, semantic_tokens,
    signature_help, syntax_tree, workspace_symbol,
};
use crate::settings::SettingsCache;

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
pub struct Backend<N: Notifier = Client> {
    notifier: N,
    documents: RwLock<BTreeMap<Uri, OpenDocument>>,
    /// Orders what changes the open documents and what tells the client about
    /// them. tower-lsp-server polls notifications concurrently, but this
    /// lock queues them in arrival order: an edit can't overtake the open it
    /// follows, and a publish can't land after what replaced or closed its
    /// document.
    ///
    /// It also keeps the emptied arena of the last version replaced, so the
    /// next version parses into memory that's already mapped. A close frees
    /// it.
    sync: Mutex<Option<Allocator>>,
    settings_cache: SettingsCache,
}

impl<N: Notifier> Backend<N> {
    pub fn new(notifier: N) -> Self {
        Self {
            notifier,
            documents: RwLock::new(BTreeMap::new()),
            sync: Mutex::new(None),
            settings_cache: SettingsCache::default(),
        }
    }

    /// Makes `state` what's open at `uri`, which leaves any lint still
    /// running on what it replaces with nothing to publish. What it replaces
    /// leaves its arena in `spare` when nothing else holds it.
    async fn store(
        &self,
        uri: &Uri,
        state: DocumentState,
        spare: &mut Option<Allocator>,
    ) -> Arc<DocumentState> {
        let shared = Arc::new(state);
        let open = OpenDocument { state: Arc::clone(&shared), published_lints: None };
        let replaced = self.documents.write().await.insert(uri.clone(), open);
        if let Some(Ok(version)) = replaced.map(|previous| Arc::try_unwrap(previous.state)) {
            *spare = Some(version.into_arena());
        }
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

    async fn document_settings(&self, uri: &Uri) -> Arc<DocumentSettings> {
        let path = file_path(uri);
        let project = self.settings_cache.settings_for(path.as_deref()).await;
        let target = project.target_for(path.as_deref());
        let is_included = match &path {
            Some(document_path) => {
                let (filter_settings, filtered) = (Arc::clone(&project), document_path.clone());
                // `is_included` canonicalizes the path, which touches the
                // disk. A check that dies counts as excluded, so no fix lands
                // on a guess.
                spawn_blocking(move || filter_settings.filter.is_included(&filtered))
                    .await
                    .unwrap_or(false)
            }
            None => true,
        };
        Arc::new(DocumentSettings { path, project, target, is_included })
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
        let lints = lint_off_the_serve_loop(&document).await;
        let diags = diagnostics::to_lsp(&document, &lints);
        // Through the send, so nothing newer can reach the client before it.
        let _ordered = self.sync.lock().await;
        if !self.is_open(&uri, &document).await {
            return;
        }
        if let Some(open) = self.documents.write().await.get_mut(&uri) {
            open.published_lints = Some(lints);
        }
        self.notifier.publish_diagnostics(uri, diags, Some(document.version())).await;
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
        let action = if byte_range.is_some() { "range-format" } else { "format" };
        // `format_range` returns the whole file with text outside the range
        // kept byte-for-byte, so a whole-file edit only changes the range.
        let formatted = Arc::clone(&document);
        let result = spawn_blocking(move || formatted.format(byte_range)).await.ok()?;
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
        let documents: Vec<(Uri, OpenDocument)> = self
            .documents
            .read()
            .await
            .iter()
            .map(|(uri, open)| (uri.clone(), open.clone()))
            .collect();
        Ok(spawn_blocking(move || code_action::fix_all_open(&documents)).await.unwrap_or_default())
    }

    /// Folds a `didChange` batch into the store. Returns the new state when
    /// the document is still open and the batch was current, so diagnostics
    /// should be republished.
    async fn apply_change(
        &self,
        params: DidChangeTextDocumentParams,
        spare: &mut Option<Allocator>,
    ) -> Option<Arc<DocumentState>> {
        let uri = &params.text_document.uri;
        let current = self.snapshot(uri).await?;
        // A repeated or older batch's ranges don't fit the current text, and
        // applying them would corrupt it.
        let version = params.text_document.version;
        if version <= current.version() {
            return None;
        }
        let Some(text) = current.changed_text(params.content_changes) else {
            // A range that doesn't fit means we've already desynced from the
            // client. Drop the document so nothing is served from corrupt
            // text until the client reopens it.
            self.documents.write().await.remove(uri);
            return None;
        };
        let state = current.with_text(text.pieces(), version, spare.take().unwrap_or_default());
        // Unheld, the version it replaces can lend the next one its arena.
        drop(current);
        Some(self.store(uri, state, spare).await)
    }
}

/// `document`'s lints, computed on the blocking pool: tower-lsp-server polls
/// every handler on one task, so a lint run inline would stall the whole
/// server.
///
/// A lint that dies yields nothing, and so does a document the project
/// leaves out.
async fn lint_off_the_serve_loop(document: &Arc<DocumentState>) -> Arc<[LintDiagnostic]> {
    if !document.settings().is_included {
        return Vec::new().into();
    }
    let linted = Arc::clone(document);
    spawn_blocking(move || linted.lint()).await.unwrap_or_default().into()
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
            let settings = self.document_settings(&uri).await;
            let document = {
                let mut spare = self.sync.lock().await;
                let Some(current) = self.snapshot(&uri).await else {
                    continue;
                };
                let arena = spare.take().unwrap_or_default();
                let state =
                    DocumentState::new([current.text()], current.version(), settings, arena);
                drop(current);
                let document = self.store(&uri, state, &mut spare).await;
                drop(spare);
                document
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
            let mut spare = self.sync.lock().await;
            let settings = self.document_settings(&uri).await;
            let (text, version) = (&params.text_document.text, params.text_document.version);
            let state =
                DocumentState::new([text], version, settings, spare.take().unwrap_or_default());
            let document = self.store(&uri, state, &mut spare).await;
            drop(spare);
            document
        };
        self.lint_and_publish(uri, document).await;
    }

    async fn did_change(&self, params: DidChangeTextDocumentParams) {
        let uri = params.text_document.uri.clone();
        let updated = {
            let mut spare = self.sync.lock().await;
            let updated = self.apply_change(params, &mut spare).await;
            drop(spare);
            updated
        };
        if let Some(document) = updated {
            self.lint_and_publish(uri, document).await;
        }
    }

    async fn did_save(&self, params: DidSaveTextDocumentParams) {
        let uri = params.text_document.uri;
        let document = {
            let mut spare = self.sync.lock().await;
            let Some(current) = self.snapshot(&uri).await else {
                return;
            };
            // We register with `include_text: false`, but a client may send
            // the text anyway. didSave carries no version, and the saved
            // text belongs to the current one.
            if let Some(text) = params.text
                && current.text() != text
            {
                let arena = spare.take().unwrap_or_default();
                let state = current.with_text([&text], current.version(), arena);
                drop(current);
                let document = self.store(&uri, state, &mut spare).await;
                drop(spare);
                document
            } else {
                current
            }
        };
        self.lint_and_publish(uri, document).await;
    }

    async fn did_close(&self, params: DidCloseTextDocumentParams) {
        let uri = params.text_document.uri;
        let mut ordered = self.sync.lock().await;
        self.documents.write().await.remove(&uri);
        *ordered = None;
        // Per LSP spec, clear diagnostics for closed files.
        self.notifier.publish_diagnostics(uri, Vec::new(), None).await;
        drop(ordered);
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
        let lints = match published_lints {
            Some(lints) => lints,
            None => lint_off_the_serve_loop(&document).await,
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
        // A `require` under the cursor resolves through the disk.
        Ok(spawn_blocking(move || definition::goto(&document, &params)).await.ok().flatten())
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
        // Each `require` resolves through the disk.
        let links = spawn_blocking(move || document_link::document_links(&document, &uri))
            .await
            .unwrap_or_default();
        Ok(Some(links))
    }
}

#[cfg(test)]
mod tests {
    use tower_lsp_server::ls_types::{
        TextDocumentContentChangeEvent, TextDocumentIdentifier, TextDocumentItem,
        VersionedTextDocumentIdentifier,
    };

    use super::*;

    #[derive(Debug)]
    struct Silent;

    impl Notifier for Silent {
        async fn publish_diagnostics(&self, _uri: Uri, _diags: Vec<Diagnostic>, _: Option<i32>) {}

        async fn show_message(&self, _message_type: MessageType, _message: String) {}

        async fn log_message(&self, _message_type: MessageType, _message: String) {}

        async fn register_capability(&self, _registrations: Vec<Registration>) {}
    }

    fn replace_all(uri: &Uri, version: i32, text: &str) -> DidChangeTextDocumentParams {
        DidChangeTextDocumentParams {
            text_document: VersionedTextDocumentIdentifier { uri: uri.clone(), version },
            content_changes: vec![TextDocumentContentChangeEvent {
                range: None,
                range_length: None,
                text: text.to_owned(),
            }],
        }
    }

    #[tokio::test]
    async fn a_replaced_version_lends_its_arena_to_the_next_and_a_close_frees_it() {
        let server = Backend::new(Silent);
        let uri: Uri = "untitled:Untitled-1".parse().expect("valid URI");
        server
            .did_open(DidOpenTextDocumentParams {
                text_document: TextDocumentItem {
                    uri: uri.clone(),
                    language_id: "lua".to_owned(),
                    version: 1,
                    text: "local one = 1\n".to_owned(),
                },
            })
            .await;
        assert!(server.sync.lock().await.is_none(), "nothing was replaced yet");
        server.did_change(replace_all(&uri, 2, "local two = 2\n")).await;
        assert!(server.sync.lock().await.is_some(), "version 1 left its arena");
        server.did_change(replace_all(&uri, 3, "local three = 3\n")).await;
        let document = server.snapshot(&uri).await.expect("the document is open");
        assert_eq!(document.text(), "local three = 3\n");
        assert!(server.sync.lock().await.is_some(), "version 2 left its arena");
        server
            .did_close(DidCloseTextDocumentParams { text_document: TextDocumentIdentifier { uri } })
            .await;
        assert!(server.sync.lock().await.is_none(), "a close frees the spare arena");
    }
}
