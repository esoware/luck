//! The main loop owns the open documents and touches no tree.
//!
//! It applies each notification in the order it arrives, hands every request
//! to the pool with the versions it reads, and sends on what the jobs give
//! back unless it went stale first, as rust-analyzer's main loop does.

use std::collections::BTreeMap;
use std::io;
use std::mem;
use std::num::NonZero;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use crossbeam_channel::{Receiver, Sender, select, unbounded};
use ls_types::notification::{
    Cancel, DidChangeConfiguration, DidChangeTextDocument, DidChangeWatchedFiles,
    DidChangeWorkspaceFolders, DidCloseTextDocument, DidOpenTextDocument, Exit,
    Notification as LspNotification, PublishDiagnostics, ShowMessage,
};
use ls_types::request::{
    CodeActionRequest, Completion, DocumentHighlightRequest, DocumentLinkRequest,
    DocumentSymbolRequest, FoldingRangeRequest, Formatting, GotoDefinition, HoverRequest,
    PrepareRenameRequest, RangeFormatting, References, Rename, Request as LspRequest,
    SelectionRangeRequest, SemanticTokensFullRequest, SemanticTokensRangeRequest, Shutdown,
    SignatureHelpRequest, WorkspaceSymbolRequest,
};
use ls_types::{
    Diagnostic, DidChangeConfigurationParams, DidChangeTextDocumentParams,
    DidChangeWatchedFilesParams, DidChangeWorkspaceFoldersParams, DidCloseTextDocumentParams,
    DidOpenTextDocumentParams, DocumentSymbolResponse, FileChangeType, InitializeParams,
    MessageType, NumberOrString, PublishDiagnosticsParams, ShowMessageParams,
    TextDocumentIdentifier, Uri, WorkspaceEdit, WorkspaceFoldersChangeEvent,
    WorkspaceSymbolResponse,
};
use lsp_server::{
    Connection, ErrorCode, Message, Notification, Request, RequestId, Response, ResponseError,
};
use luck_config::EDITORCONFIG_FILE_NAME;
use luck_resolver::is_definition_file;
use luck_type_checker::CancellationToken;
use serde::{Deserialize, Serialize};

use crate::capabilities;
use crate::definitions;
use crate::document::{Document, DocumentState};
use crate::files::Files;
use crate::line_index::PositionEncoding;
use crate::pool::{Pool, Priority};
use crate::providers::cursor::name_start;
use crate::providers::{
    code_action, completion, definition, diagnostics, document_highlight, document_link,
    folding_range, formatting, hover, references, rename, selection_range, semantic_tokens,
    signature_help, syntax_tree, workspace_symbol,
};
use crate::session::Session;
use crate::settings::GivenConfig;
use crate::type_check::{self, FileChanges, TypeErrors, Types};
use crate::workspace::{Walk, WorkspaceView};

/// What a job hands back to the main loop, which sends it on only if it
/// still holds.
#[derive(Debug)]
pub(crate) enum Task {
    /// A request's answer, unless the client cancelled the request first.
    Respond(Response),
    /// A message for the user, such as why formatting failed.
    Notify(Notification),
    /// One version's lints, unless a newer version or a close replaced it.
    Lints { uri: Uri, document: Arc<Document>, diags: Vec<Diagnostic> },
    /// What Luau found checking the open documents together, each with its
    /// type errors, unless a change since cancelled the check that
    /// `cancellation` stops.
    Checked {
        documents: Vec<(Uri, Arc<Document>, Vec<Diagnostic>)>,
        problems: Vec<Problem>,
        cancellation: CancellationToken,
    },
    /// Why a config doesn't load, unless the settings were reloaded since
    /// the lookup read it.
    ConfigError { generation: u64, problem: Problem },
    /// The files of a walk not yet read, which jobs read ahead of the first
    /// request that needs them, unless a new walk replaced it.
    Walked { walk: Arc<Walk>, unread: Vec<PathBuf> },
}

/// Why a config or a definition file doesn't load, as the user is shown it,
/// and as it's published on the file at fault, unless that's a config no file
/// holds or a file no URI names.
#[derive(Debug)]
pub(crate) struct Problem {
    pub message: String,
    pub published: Option<(Uri, Vec<Diagnostic>)>,
}

/// What the server last published on an open document: its lints, and the
/// type errors the last check found, which go out together.
#[derive(Debug, Default)]
struct Published {
    lints: Vec<Diagnostic>,
    type_errors: Vec<Diagnostic>,
    is_sent: bool,
}

impl Published {
    /// Makes `diags` the part `part` picks, and gives every diagnostic it
    /// holds, unless that part held `diags` already and went out.
    fn update(
        &mut self,
        part: fn(&mut Self) -> &mut Vec<Diagnostic>,
        diags: Vec<Diagnostic>,
    ) -> Option<Vec<Diagnostic>> {
        if self.is_sent && *part(self) == diags {
            return None;
        }
        *part(self) = diags;
        self.is_sent = true;
        Some(self.lints.iter().chain(&self.type_errors).cloned().collect())
    }
}

/// `luck/syntaxTree`: the open document's tree, dumped.
enum SyntaxTree {}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SyntaxTreeParams {
    text_document: TextDocumentIdentifier,
}

impl LspRequest for SyntaxTree {
    type Params = SyntaxTreeParams;
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

/// `luck/didChangeFiles`: files the client holds, each with its text, or
/// with none once it's deleted, which the server reads as it would a disk's
/// once the client has given a config.
enum DidChangeFiles {}

#[derive(Debug, Serialize, Deserialize)]
struct DidChangeFilesParams {
    changes: Vec<FileChange>,
}

#[derive(Debug, Serialize, Deserialize)]
struct FileChange {
    uri: Uri,
    text: Option<String>,
}

impl LspNotification for DidChangeFiles {
    type Params = DidChangeFilesParams;
    const METHOD: &'static str = "luck/didChangeFiles";
}

/// LSP 3.18's `workspace/textDocumentContent`, which `ls_types` doesn't
/// have: the text of a document of a scheme the server registered.
enum TextDocumentContent {}

#[derive(Debug, Serialize, Deserialize)]
struct TextDocumentContentParams {
    uri: Uri,
}

#[derive(Debug, Serialize, Deserialize)]
struct TextDocumentContentResult {
    text: String,
}

impl LspRequest for TextDocumentContent {
    type Params = TextDocumentContentParams;
    type Result = TextDocumentContentResult;
    const METHOD: &'static str = "workspace/textDocumentContent";
}

/// Serves one client over `connection` until it sends `exit`.
///
/// A client that gives a config as its `initializationOptions`, the keys of a
/// `luck.json` but `extends`, relative to its first workspace folder, or to
/// `/` with none, has every document take that config's project, which
/// `workspace/didChangeConfiguration` replaces with its `settings`, and the
/// server reads only its open documents and the files it pushes with
/// `luck/didChangeFiles`, never the disk.
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
    let InitializeParams {
        capabilities: client, workspace_folders, initialization_options, ..
    } = match serde_json::from_value::<InitializeParams>(params) {
        Ok(parsed) => parsed,
        Err(error) => {
            let refusal = Response::new_err(id, ErrorCode::InvalidParams as i32, error.to_string());
            let _ = connection.sender.send(refusal.into());
            return Err(io::Error::other(error));
        }
    };
    let offered = client.general.as_ref().and_then(|general| general.position_encodings.as_deref());
    let encoding = PositionEncoding::negotiate(offered.unwrap_or_default());
    let result = capabilities::initialize_result(encoding).map_err(io::Error::other)?;
    connection.initialize_finish(id, result).map_err(io::Error::other)?;

    let Connection { sender, receiver } = connection;
    let (tasks, finished) = unbounded();
    let folders: Vec<Uri> =
        workspace_folders.into_iter().flatten().map(|folder| folder.uri).collect();
    let given = initialization_options.filter(|config| !config.is_null());
    let files = Files::new(given.is_some().then(Arc::default));
    let given = given.map(|config| GivenConfig {
        root: folders
            .first()
            .and_then(|folder| files.path(folder))
            .unwrap_or_else(|| PathBuf::from("/")),
        text: config.to_string().into(),
    });
    let mut server = Server {
        sender,
        session: Arc::new(Session::new(encoding, tasks, threads, files, given)),
        pool: Pool::new(threads)?,
        pending: BTreeMap::new(),
        documents: BTreeMap::new(),
        published: BTreeMap::new(),
        config_problems: Vec::new(),
        definition_problems: BTreeMap::new(),
        workspace_folders: Vec::new(),
        checks: CancellationToken::new(),
        is_shutting_down: false,
    };
    server.set_workspace_folders(folders);
    // Without file watchers, cached settings, resolutions, cached misses
    // included, and the workspace's files would live until a restart, so
    // creating or editing a config file or a module would do nothing. LSP
    // has no static form of them. A client's own files say they changed.
    let can_watch_files = client
        .workspace
        .as_ref()
        .and_then(|workspace| workspace.did_change_watched_files.as_ref())
        .and_then(|watched| watched.dynamic_registration)
        .unwrap_or(false);
    if can_watch_files && server.session.files().client().is_none() {
        server.send(capabilities::watcher_registration());
    }
    server.run(&receiver, &finished)
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
    /// What the server last published on each open document.
    published: BTreeMap<Uri, Published>,
    /// Each file a config's problem is published on, which a reload of the
    /// settings clears.
    config_problems: Vec<Uri>,
    /// What's published on each definition file no document holds open
    /// about why Luau can't load it.
    definition_problems: BTreeMap<Uri, Vec<Diagnostic>>,
    /// The client's, which the workspace walks.
    workspace_folders: Vec<Uri>,
    /// What stops the type checks that the last change to an open document,
    /// or to a file one can read, started, which the next change, or
    /// `shutdown`, cancels.
    checks: CancellationToken,
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
        let Self { session, pool, documents, checks, .. } = self;
        session.stop();
        checks.cancel();
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
            Task::Lints { uri, document, diags } => {
                self.update_published(uri, &document, |published| &mut published.lints, diags);
            }
            Task::Checked { documents, problems, cancellation } => {
                if cancellation.is_cancelled() {
                    return;
                }
                for (uri, document, diags) in documents {
                    self.update_published(
                        uri,
                        &document,
                        |published| &mut published.type_errors,
                        diags,
                    );
                }
                let mut last = mem::take(&mut self.definition_problems);
                for problem in problems {
                    let Some((uri, diagnostics)) = &problem.published else {
                        continue;
                    };
                    let (uri, diagnostics) = (uri.clone(), diagnostics.clone());
                    if last.remove(&uri).as_ref() != Some(&diagnostics) {
                        self.report(problem);
                    }
                    self.definition_problems.insert(uri, diagnostics);
                }
                for uri in last.into_keys() {
                    self.publish(uri, Vec::new(), None);
                }
            }
            Task::ConfigError { generation, problem } => {
                if generation == self.session.settings().generation() {
                    if let Some((uri, _)) = &problem.published {
                        self.config_problems.push(uri.clone());
                    }
                    self.report(problem);
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

    /// Publishes on `document`, if it's still what's open at `uri`, its
    /// diagnostics with `diags` as the part `part` picks, unless that
    /// changes nothing published.
    fn update_published(
        &mut self,
        uri: Uri,
        document: &Arc<Document>,
        part: fn(&mut Published) -> &mut Vec<Diagnostic>,
        diags: Vec<Diagnostic>,
    ) {
        if !self.documents.get(&uri).is_some_and(|current| Arc::ptr_eq(current, document)) {
            return;
        }
        let published = self.published.entry(uri.clone()).or_default();
        if let Some(all) = published.update(part, diags) {
            self.publish(uri, all, Some(document.version()));
        }
    }

    /// Shows `problem` and publishes it on its file.
    fn report(&self, problem: Problem) {
        let Problem { message, published } = problem;
        let shown = ShowMessageParams { typ: MessageType::ERROR, message };
        self.send(Notification::new(ShowMessage::METHOD.to_owned(), shown));
        if let Some((uri, diagnostics)) = published {
            self.publish(uri, diagnostics, None);
        }
    }

    /// Makes `document` what's open at `uri`, which leaves any job still
    /// linting what it replaces to stop with nothing to publish.
    fn replace(&mut self, uri: Uri, document: Arc<Document>) {
        if let Some(replaced) = self.documents.insert(uri, document) {
            replaced.mark_stale();
        }
    }

    /// Gives each open document at `uris` a new version of the same text,
    /// whose settings are found afresh.
    fn reload(&mut self, uris: Vec<Uri>) {
        for uri in uris {
            if let Some(current) = self.documents.get(&uri) {
                let reloaded = Arc::new(current.reloaded());
                self.replace(uri, reloaded);
            }
        }
    }

    /// Stops the type checks the last change started, and lints every open
    /// document and checks them all again, since a change to one module can
    /// change the types of every module that requires it. Each document's
    /// lints go out once they're done, with the type errors of the last
    /// check until this one's are.
    fn republish(&mut self) {
        self.checks.cancel();
        self.checks = CancellationToken::new();
        let documents = self.open_documents();
        for (uri, document) in &documents {
            let (session, uri, document) =
                (Arc::clone(&self.session), uri.clone(), Arc::clone(document));
            self.pool.spawn(Priority::Background, move || {
                if session.is_stopping() || document.is_stale() {
                    return;
                }
                let Some(lints) = document.lints(&session) else {
                    return;
                };
                let diags = diagnostics::to_lsp(&uri, document.state(&session), lints);
                session.send(Task::Lints { uri, document, diags });
            });
        }
        let (session, cancellation) = (Arc::clone(&self.session), self.checks.clone());
        self.pool.spawn(Priority::Background, move || {
            if session.is_stopping() {
                return;
            }
            let Some(TypeErrors { documents: type_errors, problems }) =
                type_check::type_errors(&session, &documents, &cancellation)
            else {
                return;
            };
            let mut checked = Vec::with_capacity(documents.len());
            for ((uri, document), errors) in documents.into_iter().zip(type_errors) {
                if cancellation.is_cancelled() {
                    return;
                }
                let state = document.state(&session);
                let diags = diagnostics::type_errors_to_lsp(&uri, state, &errors);
                checked.push((uri, document, diags));
            }
            session.send(Task::Checked { documents: checked, problems, cancellation });
            session.type_checks().drop_unused();
        });
    }

    /// Makes `folders` the workspace and walks it. A client's files with no
    /// folder all are its workspace.
    fn set_workspace_folders(&mut self, folders: Vec<Uri>) {
        let files = self.session.files();
        let mut paths: Vec<PathBuf> = folders.iter().filter_map(|uri| files.path(uri)).collect();
        if paths.is_empty() && files.client().is_some() {
            paths.push(PathBuf::from("/"));
        }
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
                    self.watched_files_changed(watched);
                }
            }
            DidChangeFiles::METHOD => {
                if let Ok(pushed) = serde_json::from_value(params) {
                    self.pushed_files_changed(pushed);
                }
            }
            DidChangeConfiguration::METHOD => {
                if let Ok(changed) = serde_json::from_value(params) {
                    self.configuration_changed(&changed);
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
        let path = self.session.files().path(&item.uri);
        let text: Arc<str> = Arc::from(item.text);
        // The document's own diagnostics take the place of its file's.
        self.definition_problems.remove(&item.uri);
        let document = Document::new(item.version, Arc::clone(&text), path);
        self.replace(item.uri.clone(), Arc::new(document));
        let change = self.lay_over(&item.uri, Some(text));
        if !change.is_some_and(|change| self.files_changed(&[change])) {
            self.republish();
        }
    }

    /// Makes `text`, or with `None`, nothing, what's open at `uri`: among the
    /// client's files, when the client gave a config, and as the text a
    /// definition file reads as. Gives how that changed the file there, if
    /// it did.
    fn lay_over(&self, uri: &Uri, text: Option<Arc<str>>) -> Option<(PathBuf, FileChangeType)> {
        let files = self.session.files();
        let path = files.path(uri)?;
        let is_created_or_deleted =
            files.client().is_some_and(|client| client.open(&path, uri.clone(), text.clone()));
        let is_definitions = is_definition_file(&path);
        let typ = match (is_created_or_deleted, &text) {
            (true, Some(_)) => FileChangeType::CREATED,
            (true, None) => FileChangeType::DELETED,
            (false, _) if is_definitions => FileChangeType::CHANGED,
            (false, _) => return None,
        };
        if is_definitions {
            files.set_open_definition(&path, text);
        }
        Some((path, typ))
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
            let text = Arc::clone(next.text());
            self.replace(text_document.uri.clone(), Arc::new(next));
            let change = self.lay_over(&text_document.uri, Some(text));
            if !change.is_some_and(|change| self.files_changed(&[change])) {
                self.republish();
            }
            return;
        }
        // A range that doesn't fit means we've already desynced from the
        // client. Drop the document so nothing is served from corrupt text
        // until the client reopens it.
        if let Some(desynced) = self.documents.remove(&text_document.uri) {
            desynced.mark_stale();
        }
        self.published.remove(&text_document.uri);
        self.publish(text_document.uri, Vec::new(), None);
        self.republish();
    }

    fn close(&mut self, params: DidCloseTextDocumentParams) {
        let uri = params.text_document.uri;
        if let Some(closed) = self.documents.remove(&uri) {
            closed.mark_stale();
        }
        self.published.remove(&uri);
        let change = self.lay_over(&uri, None);
        self.session.spare_arena().clear();
        // LSP has a server clear a closed file's diagnostics.
        self.publish(uri.clone(), Vec::new(), None);
        if let Some(path) = self.session.files().path(&uri) {
            // What the files hold may not be what the document last held,
            // which a client that watches no files never says.
            self.session.workspace().forget(path);
        }
        if !change.is_some_and(|change| self.files_changed(&[change])) {
            self.republish();
        }
    }

    fn watched_files_changed(&mut self, params: DidChangeWatchedFilesParams) {
        if self.session.files().client().is_some() {
            return;
        }
        let files = self.session.files();
        let changes: Vec<(PathBuf, FileChangeType)> = params
            .changes
            .into_iter()
            .filter_map(|change| Some((files.path(&change.uri)?, change.typ)))
            .collect();
        self.files_changed(&changes);
    }

    /// Takes in the files the client pushed, when it gave a config.
    fn pushed_files_changed(&mut self, params: DidChangeFilesParams) {
        let files = self.session.files();
        let Some(client) = files.client() else {
            return;
        };
        let mut changes = Vec::with_capacity(params.changes.len());
        for FileChange { uri, text } in params.changes {
            let Some(path) = files.path(&uri) else {
                continue;
            };
            let is_deleted = text.is_none();
            let typ = if !client.push(&path, uri, text.map(Arc::from)) {
                FileChangeType::CHANGED
            } else if is_deleted {
                FileChangeType::DELETED
            } else {
                FileChangeType::CREATED
            };
            changes.push((path, typ));
        }
        self.files_changed(&changes);
    }

    /// Takes the client's new config, when it gave one to start with.
    fn configuration_changed(&mut self, params: &DidChangeConfigurationParams) {
        if !self.session.settings().is_given() {
            return;
        }
        self.session.settings().set_given(params.settings.to_string());
        self.reload_settings();
    }

    /// Brings everything to the files at each of `changes`' paths, which
    /// changed as it says, and says whether that republished, as a change
    /// to a settings file, a definition file a project lists, a module, or a
    /// `.luaurc` or `.config.luau` does.
    fn files_changed(&mut self, changes: &[(PathBuf, FileChangeType)]) -> bool {
        let session = Arc::clone(&self.session);
        let mut affected = FileChanges::default();
        let (mut is_settings_change, mut is_walk_stale) = (false, false);
        let mut definitions = Vec::new();
        for (path, typ) in changes {
            let Some(name) = path.file_name() else {
                continue;
            };
            let is_added_or_removed = *typ != FileChangeType::CHANGED;
            let is_luau =
                path.extension().is_some_and(|extension| extension == "luau" || extension == "lua");
            let is_created = *typ == FileChangeType::CREATED;
            is_settings_change |= session.settings().is_settings_file(path, is_created);
            if name == EDITORCONFIG_FILE_NAME {
                session.settings().clear_editorconfigs();
            } else if name == ".luaurc" || name == ".config.luau" {
                affected.is_config_changed = true;
            } else if is_definition_file(path) {
                definitions.push(name);
            } else if is_luau {
                affected.modules.push(path.clone());
                affected.is_module_added_or_removed |= is_added_or_removed;
            }
            if is_luau {
                session.workspace().forget(path.clone());
                is_walk_stale |= is_added_or_removed;
            }
        }
        let is_resolution_change =
            affected.is_module_added_or_removed || affected.is_config_changed;
        if is_resolution_change {
            session.resolver().clear_cache();
        }
        let is_module_change = is_resolution_change || !affected.modules.is_empty();
        if is_module_change {
            session.type_checks().changed(&affected);
        }
        if is_settings_change {
            self.reload_settings();
            return true;
        }
        if is_walk_stale {
            session.workspace().walk_again();
            self.walk_workspace();
        }
        let mut is_definition_change = false;
        for name in definitions {
            is_definition_change |= session.settings().definition_changed(name);
            // A job may still be finding a document's settings under a
            // project that read the file before it changed.
            let stale: Vec<Uri> = self
                .documents
                .iter()
                .filter(|(_, document)| {
                    let found = document.found_settings();
                    found.is_none_or(|settings| settings.project.lists_definition(name))
                })
                .map(|(uri, _)| uri.clone())
                .collect();
            is_definition_change |= !stale.is_empty();
            self.reload(stale);
        }
        let is_republished = is_module_change || is_definition_change;
        if is_republished {
            self.republish();
        }
        is_republished
    }

    fn workspace_folders_changed(&mut self, params: DidChangeWorkspaceFoldersParams) {
        let WorkspaceFoldersChangeEvent { added, removed } = params.event;
        let mut folders = mem::take(&mut self.workspace_folders);
        folders.retain(|folder| removed.iter().all(|gone| gone.uri != *folder));
        folders.extend(added.into_iter().map(|folder| folder.uri));
        self.set_workspace_folders(folders);
    }

    fn reload_settings(&mut self) {
        self.session.settings().clear();
        // A config still broken reports itself again as documents reload.
        for uri in mem::take(&mut self.config_problems) {
            self.publish(uri, Vec::new(), None);
        }
        self.session.workspace().walk_again();
        self.walk_workspace();
        // Every open document gets a new version of the same text under the
        // new settings, whether or not its target or extra globals changed,
        // so a lint under the old ones has nothing to publish.
        let uris = self.documents.keys().cloned().collect();
        self.reload(uris);
        self.republish();
    }

    fn cancel(&mut self, cancelled: NumberOrString) {
        let id = match cancelled {
            NumberOrString::Number(number) => RequestId::from(number),
            NumberOrString::String(text) => RequestId::from(text),
        };
        if let Some(is_cancelled) = self.pending.remove(&id) {
            is_cancelled.store(true, Ordering::Relaxed);
            let refusal = "the client cancelled it".to_owned();
            self.send(Response::new_err(id, ErrorCode::RequestCanceled as i32, refusal));
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
                let refusal = error.to_string();
                self.send(Response::new_err(id, ErrorCode::InvalidParams as i32, refusal));
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
        F: FnOnce(&Session, &dyn Fn() -> bool) -> Result<R::Result, ResponseError> + Send + 'static,
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
    /// the document `uri_of` picks from its params, or with `null` when that
    /// document isn't open.
    fn spawn_on_document<R, U, A, T>(&mut self, request: Request, uri_of: U, answer: A)
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
        let document = self.document(uri_of(&params));
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

    /// Answers `request` as [`Self::spawn_on_document`] does, with what
    /// `answer` gives for the document's types at what `target` finds in it,
    /// once Luau has checked it with the open documents as they stood when
    /// the request came, or with `null` where `target` finds nothing, or
    /// once a change stops the check.
    fn spawn_on_types<R, U, G, A, Target, T>(
        &mut self,
        request: Request,
        uri_of: U,
        target: G,
        answer: A,
    ) where
        R: LspRequest<Result = Option<T>>,
        U: FnOnce(&R::Params) -> &Uri,
        G: FnOnce(&DocumentState, R::Params) -> Option<Target> + Send + 'static,
        A: FnOnce(&DocumentState, Target, &mut Types<'_>) -> Option<T> + Send + 'static,
        T: 'static,
    {
        let Some((id, params)) = self.params::<R>(request) else {
            return;
        };
        let uri = uri_of(&params).clone();
        let document = self.document(&uri);
        let documents = self.open_documents();
        let cancellation = self.checks.clone();
        self.spawn::<R, _>(id, move |session, _| {
            let Some(open) = document else {
                return Ok(None);
            };
            let state = open.state(session);
            let Some(found) = target(state, params) else {
                return Ok(None);
            };
            let answered =
                type_check::with_types(session, &uri, state, &documents, &cancellation, |types| {
                    answer(state, found, types)
                });
            Ok(answered.flatten())
        });
    }

    fn handle_request(&mut self, request: Request) {
        if self.is_shutting_down {
            let refusal = "the server is shutting down".to_owned();
            self.send(Response::new_err(request.id, ErrorCode::InvalidRequest as i32, refusal));
            return;
        }
        match request.method.as_str() {
            Shutdown::METHOD => {
                self.is_shutting_down = true;
                self.checks.cancel();
                for document in self.documents.values() {
                    document.mark_stale();
                }
                self.send(Response::new_ok(request.id, ()));
            }
            Formatting::METHOD => self.spawn_on_document::<Formatting, _, _, _>(
                request,
                |params| &params.text_document.uri,
                |session, document, _| Ok(formatting::formatting(session, document, None)),
            ),
            RangeFormatting::METHOD => self.spawn_on_document::<RangeFormatting, _, _, _>(
                request,
                |params| &params.text_document.uri,
                |session, document, params| {
                    Ok(formatting::formatting(session, document, Some(params.range)))
                },
            ),
            HoverRequest::METHOD => self.spawn_on_types::<HoverRequest, _, _, _, _, _>(
                request,
                |params| &params.text_document_position_params.text_document.uri,
                |document, params| {
                    name_start(document, params.text_document_position_params.position)
                },
                hover::hover,
            ),
            Completion::METHOD => self.spawn_on_types::<Completion, _, _, _, _, _>(
                request,
                |params| &params.text_document_position.text_document.uri,
                |_, params| Some(params.text_document_position.position),
                |document, position, types| Some(completion::completion(document, position, types)),
            ),
            SignatureHelpRequest::METHOD => self
                .spawn_on_types::<SignatureHelpRequest, _, _, _, _, _>(
                    request,
                    |params| &params.text_document_position_params.text_document.uri,
                    |document, params| {
                        let position = params.text_document_position_params.position;
                        signature_help::call_at(document, position)
                    },
                    signature_help::signature_help,
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
                        code: ErrorCode::ContentModified as i32,
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
            GotoDefinition::METHOD => {
                let Some((id, params)) = self.params::<GotoDefinition>(request) else {
                    return;
                };
                let position = params.text_document_position_params.position;
                let uri = params.text_document_position_params.text_document.uri;
                let document = self.document(&uri);
                let documents = self.open_documents();
                let cancellation = self.checks.clone();
                self.spawn::<GotoDefinition, _>(id, move |session, is_cancelled| {
                    let Some(open) = document else {
                        return Ok(None);
                    };
                    let state = open.state(session);
                    let workspace = WorkspaceView::new(session, &documents, is_cancelled);
                    if let Some(found) = definition::goto(&workspace, &uri, state, position) {
                        return Ok(Some(found));
                    }
                    let Some(start) = name_start(state, position) else {
                        return Ok(None);
                    };
                    let declared = type_check::with_types(
                        session,
                        &uri,
                        state,
                        &documents,
                        &cancellation,
                        |types| definition::declared(start, types),
                    );
                    Ok(declared.flatten())
                });
            }
            TextDocumentContent::METHOD => {
                let Some((id, params)) = self.params::<TextDocumentContent>(request) else {
                    return;
                };
                self.spawn::<TextDocumentContent, _>(id, move |_, _| {
                    let text =
                        definitions::served_text(&params.uri).ok_or_else(|| ResponseError {
                            code: ErrorCode::InvalidParams as i32,
                            message: format!(
                                "the server serves no document at `{}`",
                                params.uri.as_str()
                            ),
                            data: None,
                        })?;
                    Ok(TextDocumentContentResult { text: text.to_owned() })
                });
            }
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
                        let code = ErrorCode::InvalidParams as i32;
                        ResponseError { code, message: refusal, data: None }
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
                let Some((id, params)) = self.params::<SyntaxTree>(request) else {
                    return;
                };
                let document = self.document(&params.text_document.uri);
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
                self.send(Response::new_err(request.id, ErrorCode::MethodNotFound as i32, refusal));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn diagnostic(message: &str) -> Diagnostic {
        Diagnostic { message: message.to_owned(), ..Diagnostic::default() }
    }

    #[test]
    fn a_document_s_diagnostics_go_out_once_a_part_of_them_changes() {
        let lints: fn(&mut Published) -> &mut Vec<Diagnostic> = |published| &mut published.lints;
        let type_errors: fn(&mut Published) -> &mut Vec<Diagnostic> =
            |published| &mut published.type_errors;
        let mut published = Published::default();
        assert_eq!(published.update(lints, Vec::new()), Some(Vec::new()), "the first goes out");
        assert_eq!(published.update(lints, Vec::new()), None);

        let mismatch = diagnostic("Expected this to be 'number', but got 'string'");
        let checked = published.update(type_errors, vec![mismatch.clone()]);
        assert_eq!(checked, Some(vec![mismatch.clone()]));
        assert_eq!(published.update(type_errors, vec![mismatch.clone()]), None);

        let unused = diagnostic("`count` is never used");
        assert_eq!(
            published.update(lints, vec![unused.clone()]),
            Some(vec![unused, mismatch]),
            "lints go out with the last check's type errors"
        );
    }
}
