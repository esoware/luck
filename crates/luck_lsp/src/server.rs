//! The main loop owns the open documents and touches no tree.
//!
//! It applies each notification in the order it arrives, hands every request
//! to the pool with the versions it reads, and sends on what the jobs give
//! back unless it went stale first, as rust-analyzer's main loop does.

use std::collections::BTreeMap;
use std::io;
use std::mem;
use std::num::NonZero;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use crossbeam_channel::{Receiver, Sender, select, unbounded};
use ls_types::notification::{
    Cancel, DidChangeTextDocument, DidChangeWatchedFiles, DidChangeWorkspaceFolders,
    DidCloseTextDocument, DidOpenTextDocument, Exit, Notification as _, PublishDiagnostics,
    ShowMessage,
};
use ls_types::request::{
    CodeActionRequest, Completion, DocumentHighlightRequest, DocumentLinkRequest,
    DocumentSymbolRequest, FoldingRangeRequest, Formatting, GotoDefinition, HoverRequest,
    PrepareRenameRequest, RangeFormatting, References, RegisterCapability, Rename,
    Request as LspRequest, SelectionRangeRequest, SemanticTokensFullRequest,
    SemanticTokensRangeRequest, Shutdown, SignatureHelpRequest, WorkspaceSymbolRequest,
};
use ls_types::{
    CodeActionKind, CodeActionOptions, CodeActionProviderCapability, CompletionOptions, Diagnostic,
    DidChangeTextDocumentParams, DidChangeWatchedFilesParams,
    DidChangeWatchedFilesRegistrationOptions, DidChangeWorkspaceFoldersParams,
    DidCloseTextDocumentParams, DidOpenTextDocumentParams, DocumentLinkOptions,
    DocumentSymbolResponse, FileChangeType, FileSystemWatcher, FoldingRangeProviderCapability,
    GlobPattern, HoverProviderCapability, InitializeParams, InitializeResult, MessageType,
    NumberOrString, OneOf, PublishDiagnosticsParams, Range, Registration, RegistrationParams,
    RenameOptions, SelectionRangeProviderCapability, SemanticTokensFullOptions,
    SemanticTokensOptions, SemanticTokensRegistrationOptions, SemanticTokensServerCapabilities,
    ServerCapabilities, ServerInfo, ShowMessageParams, SignatureHelpOptions,
    StaticRegistrationOptions, TextDocumentRegistrationOptions, TextDocumentSyncCapability,
    TextDocumentSyncKind, TextDocumentSyncOptions, TextEdit, Uri, WorkDoneProgressOptions,
    WorkspaceEdit, WorkspaceFoldersChangeEvent, WorkspaceFoldersServerCapabilities,
    WorkspaceServerCapabilities, WorkspaceSymbolResponse,
};
use lsp_server::{Connection, Message, Notification, Request, RequestId, Response, ResponseError};

use crate::document::{Document, DocumentState, LazySettings, file_path};
use crate::line_index::PositionEncoding;
use crate::pool::{Pool, Priority};
use crate::providers::{
    code_action, completion, definition, diagnostics, document_highlight, document_link,
    folding_range, hover, references, rename, selection_range, semantic_tokens, signature_help,
    syntax_tree, workspace_symbol,
};
use crate::session::Session;
use crate::workspace::{Walk, WorkspaceView};

/// JSON-RPC's error codes, and LSP's for a request the client or a change
/// made moot.
const INVALID_REQUEST: i32 = -32600;
const METHOD_NOT_FOUND: i32 = -32601;
const INVALID_PARAMS: i32 = -32602;
const REQUEST_CANCELLED: i32 = -32800;
const CONTENT_MODIFIED: i32 = -32801;

/// The id of the one request the server sends, which needs no answer.
const WATCHER_REGISTRATION: &str = "luck-file-watchers";

/// What a job hands back to the main loop, which sends it on only if it
/// still holds.
#[derive(Debug)]
pub(crate) enum Task {
    /// A request's answer, unless the client cancelled the request first.
    Respond(Response),
    /// A message for the user, such as why formatting failed.
    Notify(Notification),
    /// One version's diagnostics, unless a newer version or a close
    /// replaced it, or a change to what its requires resolve to started
    /// another publish of it.
    Publish { uri: Uri, document: Arc<Document>, diags: Vec<Diagnostic>, resolutions: u64 },
    /// Why a `luck.json` doesn't load, unless the settings were reloaded
    /// since the lookup read it.
    ConfigError { generation: u64, notifications: Vec<Notification> },
    /// The files of a walk not yet read, which jobs read ahead of the first
    /// request that needs them, unless a new walk replaced it.
    Walked { walk: Arc<Walk>, unread: Vec<PathBuf> },
}

/// `luck/syntaxTree`: the open document's tree, dumped.
enum SyntaxTree {}

impl LspRequest for SyntaxTree {
    type Params = serde_json::Value;
    type Result = String;
    const METHOD: &'static str = "luck/syntaxTree";
}

/// `luck/fixAllWorkspace`: every open document's fixes, as one edit.
enum FixAllWorkspace {}

impl LspRequest for FixAllWorkspace {
    type Params = serde_json::Value;
    type Result = WorkspaceEdit;
    const METHOD: &'static str = "luck/fixAllWorkspace";
}

/// Serves one client over `connection` until it sends `exit`.
///
/// Every job runs on the `threads` threads this starts, each with
/// [`luck_ast::MIN_STACK_BYTES`] of stack.
///
/// # Errors
///
/// Returns why the session didn't end as LSP says it should: the connection
/// broke, the client didn't start with `initialize`, a worker didn't start,
/// or the client sent `exit` without `shutdown` before it, which makes the
/// server exit with an error.
pub fn serve(connection: Connection, threads: NonZero<usize>) -> io::Result<()> {
    let (id, params) = connection.initialize_start().map_err(io::Error::other)?;
    let InitializeParams { capabilities, workspace_folders, .. } =
        match serde_json::from_value::<InitializeParams>(params) {
            Ok(parsed) => parsed,
            Err(error) => {
                let refusal = error_response(id, INVALID_PARAMS, error.to_string());
                let _ = connection.sender.send(refusal.into());
                return Err(io::Error::other(error));
            }
        };
    let offered =
        capabilities.general.as_ref().and_then(|general| general.position_encodings.as_deref());
    let encoding = PositionEncoding::negotiate(offered.unwrap_or_default());
    let result = serde_json::to_value(initialize_result(encoding)).map_err(io::Error::other)?;
    connection.initialize_finish(id, result).map_err(io::Error::other)?;

    let Connection { sender, receiver } = connection;
    let (tasks, finished) = unbounded();
    let mut server = Server {
        sender,
        session: Arc::new(Session::new(encoding, tasks, threads)),
        pool: Pool::new(threads)?,
        pending: BTreeMap::new(),
        documents: BTreeMap::new(),
        workspace_folders: Vec::new(),
        resolutions: 0,
        is_shutting_down: false,
    };
    server.set_workspace_folders(
        workspace_folders.into_iter().flatten().map(|folder| folder.uri).collect(),
    );
    // Without file watchers, cached settings, resolutions, cached misses
    // included, and the workspace's files would live until a restart, so
    // creating or editing a config file or a module would do nothing. LSP
    // has no static form of them.
    let can_watch_files = capabilities
        .workspace
        .as_ref()
        .and_then(|workspace| workspace.did_change_watched_files.as_ref())
        .and_then(|watched| watched.dynamic_registration)
        .unwrap_or(false);
    if can_watch_files {
        server.register_watchers();
    }
    server.run(&receiver, &finished)
}

fn initialize_result(encoding: PositionEncoding) -> InitializeResult {
    InitializeResult {
        capabilities: ServerCapabilities {
            position_encoding: Some(encoding.kind()),
            text_document_sync: Some(TextDocumentSyncCapability::Options(
                TextDocumentSyncOptions {
                    open_close: Some(true),
                    change: Some(TextDocumentSyncKind::INCREMENTAL),
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
            code_action_provider: Some(CodeActionProviderCapability::Options(CodeActionOptions {
                code_action_kinds: Some(vec![CodeActionKind::QUICKFIX, code_action::FIX_ALL_KIND]),
                resolve_provider: Some(false),
                work_done_progress_options: WorkDoneProgressOptions::default(),
            })),
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
            workspace: Some(WorkspaceServerCapabilities {
                workspace_folders: Some(WorkspaceFoldersServerCapabilities {
                    supported: Some(true),
                    change_notifications: Some(OneOf::Left(true)),
                }),
                file_operations: None,
            }),
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
    }
}

const fn error_response(id: RequestId, code: i32, message: String) -> Response {
    Response { id, response_result: Err(ResponseError { code, message, data: None }) }
}

struct Server {
    sender: Sender<Message>,
    session: Arc<Session>,
    pool: Pool,
    /// Each request a job is answering, with what tells the job the client
    /// cancelled it.
    pending: BTreeMap<RequestId, Arc<AtomicBool>>,
    /// The current version of each open document.
    documents: BTreeMap<Uri, Arc<Document>>,
    /// The client's, which the workspace walks.
    workspace_folders: Vec<Uri>,
    /// Bumped by each change to what a require can resolve to, which starts
    /// a new publish of every open document's diagnostics.
    resolutions: u64,
    is_shutting_down: bool,
}

impl Server {
    /// Takes each message from the client and each task a job finishes, in
    /// the order they come, until the client sends `exit`, then stops every
    /// job and waits for the workers.
    fn run(mut self, messages: &Receiver<Message>, tasks: &Receiver<Task>) -> io::Result<()> {
        let ended = loop {
            select! {
                recv(messages) -> received => match received {
                    Ok(Message::Notification(notification)) if notification.method == Exit::METHOD => {
                        break if self.is_shutting_down {
                            Ok(())
                        } else {
                            Err(io::Error::other("the client sent `exit` before `shutdown`"))
                        };
                    }
                    Ok(message) => self.handle(message),
                    Err(_) => break Err(io::Error::other("the client hung up before `exit`")),
                },
                recv(tasks) -> task => {
                    // The session holds the other end, so this never closes.
                    if let Ok(finished) = task {
                        self.finish(finished);
                    }
                }
            }
        };
        let Self { session, pool, documents, .. } = self;
        session.stop();
        for document in documents.values() {
            document.mark_stale();
        }
        pool.join();
        ended
    }

    fn send<M>(&self, message: M)
    where
        M: Into<Message>,
    {
        // Only a client that has hung up stops reading.
        let _ = self.sender.send(message.into());
    }

    fn publish(&self, uri: Uri, diagnostics: Vec<Diagnostic>, version: Option<i32>) {
        let published = PublishDiagnosticsParams { uri, diagnostics, version };
        self.send(Notification::new(PublishDiagnostics::METHOD.to_owned(), published));
    }

    fn register_watchers(&self) {
        let watchers = [
            // Any JSON or TOML file, since a `luck.json` can extend a file,
            // or name a std definition, of any name. A change touching none
            // the projects read is ignored.
            "**/*.json",
            "**/*.toml",
            "**/.editorconfig",
            "**/.luaurc",
            "**/*.{lua,luau}",
        ]
        .map(|glob| FileSystemWatcher {
            glob_pattern: GlobPattern::String(glob.to_owned()),
            kind: None,
        });
        let options = DidChangeWatchedFilesRegistrationOptions { watchers: watchers.into() };
        let registration = Registration {
            id: WATCHER_REGISTRATION.to_owned(),
            method: DidChangeWatchedFiles::METHOD.to_owned(),
            register_options: serde_json::to_value(options).ok(),
        };
        self.send(Request::new(
            RequestId::from(WATCHER_REGISTRATION.to_owned()),
            RegisterCapability::METHOD.to_owned(),
            RegistrationParams { registrations: vec![registration] },
        ));
    }

    fn handle(&mut self, message: Message) {
        match message {
            Message::Request(request) => self.handle_request(request),
            Message::Notification(notification) => self.handle_notification(notification),
            // The only request the server sends is the watchers' registration,
            // and a client that refuses it just sends no changes.
            Message::Response(_) => {}
        }
    }

    fn finish(&mut self, task: Task) {
        match task {
            Task::Respond(response) => {
                if self.pending.remove(&response.id).is_some() {
                    self.send(response);
                }
            }
            Task::Notify(notification) => self.send(notification),
            Task::Publish { uri, document, diags, resolutions } => {
                let is_current =
                    self.documents.get(&uri).is_some_and(|current| Arc::ptr_eq(current, &document));
                if is_current && resolutions == self.resolutions && !self.is_shutting_down {
                    self.publish(uri, diags, Some(document.version()));
                }
            }
            Task::ConfigError { generation, notifications } => {
                if generation == self.session.settings().generation() {
                    for notification in notifications {
                        self.send(notification);
                    }
                }
            }
            Task::Walked { walk, unread } => {
                if !self.session.workspace().is_current(&walk) || self.is_shutting_down {
                    return;
                }
                for path in unread {
                    let (session, read_walk) = (Arc::clone(&self.session), Arc::clone(&walk));
                    self.pool.spawn(Priority::Idle, move || {
                        let workspace = session.workspace();
                        if !session.is_stopping() && workspace.is_current(&read_walk) {
                            workspace.file(&path).source(&session);
                        }
                    });
                }
            }
        }
    }

    /// Makes `document` what's open at `uri`, which leaves any job still
    /// linting what it replaces to stop with nothing to publish, and starts
    /// its diagnostics.
    fn replace(&mut self, uri: Uri, document: Arc<Document>) {
        if let Some(replaced) = self.documents.insert(uri.clone(), Arc::clone(&document)) {
            replaced.mark_stale();
        }
        self.publish_diagnostics(uri, document);
    }

    /// Publishes the diagnostics of `document`, the version open at `uri`,
    /// unless it goes stale first.
    fn publish_diagnostics(&self, uri: Uri, document: Arc<Document>) {
        let session = Arc::clone(&self.session);
        let resolutions = self.resolutions;
        self.pool.spawn(Priority::Background, move || {
            if document.is_stale() {
                return;
            }
            let diags = {
                let Some(lints) = document.lints(&session) else {
                    return;
                };
                diagnostics::to_lsp(&uri, document.state(&session), lints)
            };
            session.send(Task::Publish { uri, document, diags, resolutions });
        });
    }

    /// Makes `folders` the workspace and walks it.
    fn set_workspace_folders(&mut self, folders: Vec<Uri>) {
        let paths = folders.iter().filter_map(file_path).collect();
        self.workspace_folders = folders;
        self.session.workspace().set_folders(paths);
        self.walk_workspace();
    }

    /// Walks the workspace's folders ahead of the first request that needs
    /// their files, and then reads each file not read since it changed.
    fn walk_workspace(&self) {
        let walk = self.session.workspace().walk();
        if !walk.has_folders() {
            return;
        }
        let session = Arc::clone(&self.session);
        self.pool.spawn(Priority::Idle, move || {
            let workspace = session.workspace();
            if session.is_stopping() || !workspace.is_current(&walk) {
                return;
            }
            let unread = walk
                .files(&session)
                .iter()
                .filter(|path| !workspace.is_read(path))
                .cloned()
                .collect();
            session.send(Task::Walked { walk, unread });
        });
    }

    fn handle_notification(&mut self, notification: Notification) {
        // After `shutdown`, LSP leaves only `exit` for the client to send.
        if self.is_shutting_down {
            return;
        }
        let Notification { method, params } = notification;
        // A notification has no answer to refuse params that don't parse.
        match method.as_str() {
            DidOpenTextDocument::METHOD => {
                if let Ok(opened) = serde_json::from_value(params) {
                    self.open(opened);
                }
            }
            DidChangeTextDocument::METHOD => {
                if let Ok(changed) = serde_json::from_value(params) {
                    self.change(changed);
                }
            }
            DidCloseTextDocument::METHOD => {
                if let Ok(closed) = serde_json::from_value(params) {
                    self.close(closed);
                }
            }
            DidChangeWatchedFiles::METHOD => {
                if let Ok(watched) = serde_json::from_value(params) {
                    self.watched_files_changed(&watched);
                }
            }
            DidChangeWorkspaceFolders::METHOD => {
                if let Ok(changed) = serde_json::from_value(params) {
                    self.workspace_folders_changed(changed);
                }
            }
            Cancel::METHOD => {
                if let Ok(cancelled) = serde_json::from_value::<ls_types::CancelParams>(params) {
                    self.cancel(cancelled.id);
                }
            }
            _ => {}
        }
    }

    fn open(&mut self, params: DidOpenTextDocumentParams) {
        let item = params.text_document;
        let settings = Arc::new(LazySettings::new(file_path(&item.uri)));
        self.replace(
            item.uri,
            Arc::new(Document::new(item.version, Arc::from(item.text), settings)),
        );
    }

    fn change(&mut self, params: DidChangeTextDocumentParams) {
        let DidChangeTextDocumentParams { text_document, content_changes } = params;
        let Some(current) = self.documents.get(&text_document.uri) else {
            return;
        };
        // A repeated or older batch's ranges don't fit the current text, and
        // applying them would corrupt it.
        if text_document.version <= current.version() {
            return;
        }
        if let Some(next) =
            current.changed(text_document.version, content_changes, self.session.encoding())
        {
            self.replace(text_document.uri, Arc::new(next));
            return;
        }
        // A range that doesn't fit means we've already desynced from the
        // client. Drop the document so nothing is served from corrupt text
        // until the client reopens it.
        if let Some(desynced) = self.documents.remove(&text_document.uri) {
            desynced.mark_stale();
        }
        self.publish(text_document.uri, Vec::new(), None);
    }

    fn close(&mut self, params: DidCloseTextDocumentParams) {
        let uri = params.text_document.uri;
        if let Some(closed) = self.documents.remove(&uri) {
            closed.mark_stale();
        }
        // What's on disk may not be what the document last held, which a
        // client that watches no files never says.
        if let Some(path) = file_path(&uri) {
            self.session.workspace().forget(path);
        }
        self.session.spare_arena().clear();
        // LSP has a server clear a closed file's diagnostics.
        self.publish(uri, Vec::new(), None);
    }

    fn watched_files_changed(&mut self, params: &DidChangeWatchedFilesParams) {
        let session = Arc::clone(&self.session);
        let (mut is_settings_change, mut is_resolution_change, mut is_file_added_or_removed) =
            (false, false, false);
        for change in &params.changes {
            let Some(path) = file_path(&change.uri) else {
                continue;
            };
            is_settings_change |= session.settings().is_settings_file(&path);
            let is_module =
                path.extension().is_some_and(|extension| extension == "lua" || extension == "luau");
            let is_module_added_or_removed = is_module && change.typ != FileChangeType::CHANGED;
            // A `.luaurc` edited, or a module created or deleted, can change
            // what a require resolves to.
            is_resolution_change |= is_module_added_or_removed
                || path.file_name().is_some_and(|name| name == ".luaurc");
            is_file_added_or_removed |= is_module_added_or_removed;
            if is_module {
                session.workspace().forget(path);
            }
        }
        if is_settings_change {
            self.reload_settings();
            return;
        }
        if is_file_added_or_removed {
            session.workspace().walk_again();
            self.walk_workspace();
        }
        if is_resolution_change {
            session.settings().clear_resolvers();
            self.republish();
        }
    }

    fn workspace_folders_changed(&mut self, params: DidChangeWorkspaceFoldersParams) {
        let WorkspaceFoldersChangeEvent { added, removed } = params.event;
        let mut folders = mem::take(&mut self.workspace_folders);
        folders.retain(|folder| removed.iter().all(|gone| gone.uri != *folder));
        folders.extend(added.into_iter().map(|folder| folder.uri));
        self.set_workspace_folders(folders);
    }

    /// Publishes every open document's diagnostics again, as a change to
    /// what their requires resolve to needs.
    fn republish(&mut self) {
        self.resolutions += 1;
        for (uri, document) in self.open_documents() {
            self.publish_diagnostics(uri, document);
        }
    }

    fn reload_settings(&mut self) {
        // A config still broken reports itself again as documents reload.
        for broken in self.session.settings().clear() {
            if let Some(uri) = Uri::from_file_path(&broken) {
                self.publish(uri, Vec::new(), None);
            }
        }
        self.session.workspace().clear();
        self.walk_workspace();
        // Every open document gets a new version of the same text under the
        // new settings, whether or not its dialect or extra globals changed,
        // so a lint under the old ones has nothing to publish.
        let reopened: Vec<(Uri, Document)> = self
            .documents
            .iter()
            .map(|(uri, current)| {
                let path = current.settings().path().map(Path::to_path_buf);
                (uri.clone(), current.with_settings(Arc::new(LazySettings::new(path))))
            })
            .collect();
        for (uri, document) in reopened {
            self.replace(uri, Arc::new(document));
        }
    }

    fn cancel(&mut self, cancelled: NumberOrString) {
        let id = match cancelled {
            NumberOrString::Number(number) => RequestId::from(number),
            NumberOrString::String(text) => RequestId::from(text),
        };
        if let Some(is_cancelled) = self.pending.remove(&id) {
            is_cancelled.store(true, Ordering::Relaxed);
            self.send(error_response(id, REQUEST_CANCELLED, "the client cancelled it".to_owned()));
        }
    }

    fn document(&self, uri: &Uri) -> Option<Arc<Document>> {
        self.documents.get(uri).map(Arc::clone)
    }

    /// Every open document, by URI.
    fn open_documents(&self) -> Vec<(Uri, Arc<Document>)> {
        self.documents.iter().map(|(uri, document)| (uri.clone(), Arc::clone(document))).collect()
    }

    /// `request`'s id and params, or `None` once it's refused params that
    /// don't parse.
    fn params<R>(&self, request: Request) -> Option<(RequestId, R::Params)>
    where
        R: LspRequest,
    {
        let Request { id, params, .. } = request;
        match serde_json::from_value(params) {
            Ok(parsed) => Some((id, parsed)),
            Err(error) => {
                self.send(error_response(id, INVALID_PARAMS, error.to_string()));
                None
            }
        }
    }

    /// Answers request `id` with what `job` gives on a worker, unless the
    /// client cancels it before the job starts. `job` gets the check for
    /// that, to ask between documents.
    fn spawn<R, F>(&mut self, id: RequestId, job: F)
    where
        R: LspRequest,
        F: FnOnce(&Session, &(dyn Fn() -> bool + Sync)) -> Result<R::Result, ResponseError>
            + Send
            + 'static,
    {
        let is_cancelled = Arc::new(AtomicBool::new(false));
        self.pending.insert(id.clone(), Arc::clone(&is_cancelled));
        let session = Arc::clone(&self.session);
        self.pool.spawn(Priority::Request, move || {
            let is_moot = || is_cancelled.load(Ordering::Relaxed) || session.is_stopping();
            if is_moot() {
                return;
            }
            let response = match job(&session, &is_moot) {
                Ok(result) => Response::new_ok(id, result),
                Err(error) => Response { id, response_result: Err(error) },
            };
            session.send(Task::Respond(response));
        });
    }

    /// Answers `request` with what `answer` gives for the current version of
    /// the document `uri` picks from its params, or with `null` when that
    /// document isn't open.
    fn spawn_on_document<R, U, A, T>(&mut self, request: Request, uri: U, answer: A)
    where
        R: LspRequest<Result = Option<T>>,
        U: FnOnce(&R::Params) -> &Uri,
        A: FnOnce(&Session, &DocumentState, R::Params) -> Result<Option<T>, ResponseError>
            + Send
            + 'static,
        T: 'static,
    {
        let Some((id, params)) = self.params::<R>(request) else {
            return;
        };
        let document = self.document(uri(&params));
        self.spawn::<R, _>(id, move |session, _| {
            document.map_or(Ok(None), |open| answer(session, open.state(session), params))
        });
    }

    /// Answers `request` as [`Self::spawn_on_document`] does, with what
    /// `answer` gives for the document at the URI `uri_of` picks and the
    /// workspace as every open document stood when the request came.
    fn spawn_on_workspace<R, U, A, T>(&mut self, request: Request, uri_of: U, answer: A)
    where
        R: LspRequest<Result = Option<T>>,
        U: FnOnce(&R::Params) -> &Uri,
        A: FnOnce(
                &WorkspaceView<'_>,
                &Uri,
                &DocumentState,
                R::Params,
            ) -> Result<Option<T>, ResponseError>
            + Send
            + 'static,
        T: 'static,
    {
        let Some((id, params)) = self.params::<R>(request) else {
            return;
        };
        let uri = uri_of(&params).clone();
        let document = self.document(&uri);
        let documents = self.open_documents();
        self.spawn::<R, _>(id, move |session, is_cancelled| {
            let Some(open) = document else {
                return Ok(None);
            };
            let workspace = WorkspaceView::new(session, &documents, is_cancelled);
            answer(&workspace, &uri, open.state(session), params)
        });
    }

    fn handle_request(&mut self, request: Request) {
        if self.is_shutting_down {
            let refusal = "the server is shutting down".to_owned();
            self.send(error_response(request.id, INVALID_REQUEST, refusal));
            return;
        }
        match request.method.as_str() {
            Shutdown::METHOD => {
                self.is_shutting_down = true;
                for document in self.documents.values() {
                    document.mark_stale();
                }
                self.send(Response::new_ok(request.id, ()));
            }
            Formatting::METHOD => self.spawn_on_document::<Formatting, _, _, _>(
                request,
                |params| &params.text_document.uri,
                |session, document, _| Ok(format_edits(session, document, None)),
            ),
            RangeFormatting::METHOD => self.spawn_on_document::<RangeFormatting, _, _, _>(
                request,
                |params| &params.text_document.uri,
                |session, document, params| Ok(format_edits(session, document, Some(params.range))),
            ),
            HoverRequest::METHOD => self.spawn_on_document::<HoverRequest, _, _, _>(
                request,
                |params| &params.text_document_position_params.text_document.uri,
                |_, document, params| {
                    Ok(hover::hover(document, params.text_document_position_params.position))
                },
            ),
            Completion::METHOD => self.spawn_on_document::<Completion, _, _, _>(
                request,
                |params| &params.text_document_position.text_document.uri,
                |_, document, params| {
                    Ok(Some(completion::completion(
                        document,
                        params.text_document_position.position,
                    )))
                },
            ),
            SignatureHelpRequest::METHOD => self
                .spawn_on_document::<SignatureHelpRequest, _, _, _>(
                    request,
                    |params| &params.text_document_position_params.text_document.uri,
                    |_, document, params| {
                        let position = params.text_document_position_params.position;
                        Ok(signature_help::signature_help(document, position))
                    },
                ),
            DocumentSymbolRequest::METHOD => self
                .spawn_on_document::<DocumentSymbolRequest, _, _, _>(
                    request,
                    |params| &params.text_document.uri,
                    |_, document, _| {
                        Ok(Some(DocumentSymbolResponse::Nested(document.symbols().to_vec())))
                    },
                ),
            CodeActionRequest::METHOD => {
                let Some((id, params)) = self.params::<CodeActionRequest>(request) else {
                    return;
                };
                let document = self.document(&params.text_document.uri);
                self.spawn::<CodeActionRequest, _>(id, move |session, _| {
                    let Some(open) = document else {
                        return Ok(None);
                    };
                    let lints = open.lints(session).ok_or_else(|| ResponseError {
                        code: CONTENT_MODIFIED,
                        message: "the document changed before its lints were done".to_owned(),
                        data: None,
                    })?;
                    Ok(Some(code_action::code_action(open.state(session), &params, lints)))
                });
            }
            SemanticTokensFullRequest::METHOD => self
                .spawn_on_document::<SemanticTokensFullRequest, _, _, _>(
                    request,
                    |params| &params.text_document.uri,
                    |_, document, _| Ok(Some(semantic_tokens::semantic_tokens(document))),
                ),
            SemanticTokensRangeRequest::METHOD => self
                .spawn_on_document::<SemanticTokensRangeRequest, _, _, _>(
                    request,
                    |params| &params.text_document.uri,
                    |_, document, params| {
                        Ok(Some(semantic_tokens::in_range(document, params.range)))
                    },
                ),
            DocumentHighlightRequest::METHOD => self
                .spawn_on_document::<DocumentHighlightRequest, _, _, _>(
                    request,
                    |params| &params.text_document_position_params.text_document.uri,
                    |_, document, params| {
                        let position = params.text_document_position_params.position;
                        Ok(Some(document_highlight::document_highlights(document, position)))
                    },
                ),
            GotoDefinition::METHOD => self.spawn_on_workspace::<GotoDefinition, _, _, _>(
                request,
                |params| &params.text_document_position_params.text_document.uri,
                |workspace, uri, document, params| {
                    let position = params.text_document_position_params.position;
                    Ok(definition::goto(workspace, uri, document, position))
                },
            ),
            References::METHOD => self.spawn_on_workspace::<References, _, _, _>(
                request,
                |params| &params.text_document_position.text_document.uri,
                |workspace, uri, document, params| {
                    Ok(references::references(workspace, uri, document, &params))
                },
            ),
            PrepareRenameRequest::METHOD => self
                .spawn_on_workspace::<PrepareRenameRequest, _, _, _>(
                    request,
                    |params| &params.text_document.uri,
                    |workspace, uri, document, params| {
                        Ok(rename::prepare(workspace, uri, document, params.position))
                    },
                ),
            Rename::METHOD => self.spawn_on_workspace::<Rename, _, _, _>(
                request,
                |params| &params.text_document_position.text_document.uri,
                |workspace, uri, document, params| {
                    rename::rename(workspace, uri, document, &params).map_err(|refusal| {
                        ResponseError { code: INVALID_PARAMS, message: refusal, data: None }
                    })
                },
            ),
            FoldingRangeRequest::METHOD => self.spawn_on_document::<FoldingRangeRequest, _, _, _>(
                request,
                |params| &params.text_document.uri,
                |_, document, _| Ok(Some(folding_range::folding_ranges(document))),
            ),
            SelectionRangeRequest::METHOD => self
                .spawn_on_document::<SelectionRangeRequest, _, _, _>(
                    request,
                    |params| &params.text_document.uri,
                    |_, document, params| {
                        Ok(Some(selection_range::selection_ranges(document, &params.positions)))
                    },
                ),
            DocumentLinkRequest::METHOD => self.spawn_on_document::<DocumentLinkRequest, _, _, _>(
                request,
                |params| &params.text_document.uri,
                |_, document, _| Ok(Some(document_link::document_links(document))),
            ),
            WorkspaceSymbolRequest::METHOD => {
                let Some((id, params)) = self.params::<WorkspaceSymbolRequest>(request) else {
                    return;
                };
                let documents = self.open_documents();
                self.spawn::<WorkspaceSymbolRequest, _>(id, move |session, is_cancelled| {
                    let open = documents
                        .iter()
                        .take_while(|_| !is_cancelled())
                        .map(|(uri, document)| (uri, document.state(session)));
                    let symbols = workspace_symbol::workspace_symbols(open, &params.query);
                    Ok(Some(WorkspaceSymbolResponse::Flat(symbols)))
                });
            }
            SyntaxTree::METHOD => {
                let Request { id, params, .. } = request;
                let Some(uri) = params
                    .get("textDocument")
                    .and_then(|text_document| text_document.get("uri"))
                    .and_then(|uri| uri.as_str())
                    .and_then(|uri| uri.parse::<Uri>().ok())
                else {
                    let refusal = "missing `textDocument.uri`".to_owned();
                    self.send(error_response(id, INVALID_PARAMS, refusal));
                    return;
                };
                let document = self.document(&uri);
                self.spawn::<SyntaxTree, _>(id, move |session, _| {
                    Ok(document.map_or_else(String::new, |open| {
                        syntax_tree::syntax_tree(open.state(session))
                    }))
                });
            }
            FixAllWorkspace::METHOD => {
                let documents = self.open_documents();
                self.spawn::<FixAllWorkspace, _>(request.id, move |session, is_cancelled| {
                    let fixable = documents.iter().take_while(|_| !is_cancelled()).filter_map(
                        |(uri, document)| {
                            Some((uri, document.state(session), document.lints(session)?))
                        },
                    );
                    Ok(code_action::fix_all(fixable))
                });
            }
            unknown => {
                let refusal = format!("the server has no method `{unknown}`");
                self.send(error_response(request.id, METHOD_NOT_FOUND, refusal));
            }
        }
    }
}

/// The document formatted, or only `range` of it, as one whole-document
/// edit. `None` when `range` doesn't fit it, or it doesn't parse, which also
/// shows a warning.
fn format_edits(
    session: &Session,
    document: &DocumentState,
    range: Option<Range>,
) -> Option<Vec<TextEdit>> {
    let byte_range = match range {
        Some(requested) => Some(document.byte_range(requested)?),
        None => None,
    };
    let action = if byte_range.is_some() { "range-format" } else { "format" };
    // `format_range` returns the whole file with text outside the range kept
    // byte-for-byte, so a whole-file edit only changes the range.
    let result = document.format(byte_range);
    if let Some(first) = result.errors.first() {
        let message = format!("luck: can't {action} file (parse error): {}", first.message);
        let warning = ShowMessageParams { typ: MessageType::WARNING, message };
        session.send(Task::Notify(Notification::new(ShowMessage::METHOD.to_owned(), warning)));
        return None;
    }
    if result.output == document.text().as_bytes() {
        return Some(Vec::new());
    }
    // A text that's UTF-8 formats as UTF-8.
    let new_text = String::from_utf8_lossy(&result.output).into_owned();
    Some(vec![TextEdit { range: document.full_range(), new_text }])
}
