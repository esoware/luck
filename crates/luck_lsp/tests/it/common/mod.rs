use core::mem;
use core::str::FromStr as _;
use core::time::Duration;
use std::io;
use std::panic;
use std::path::Path;
use std::thread::{self, JoinHandle};

use crossbeam_channel::unbounded;
use ls_types::notification::{
    DidChangeTextDocument, DidChangeWatchedFiles, DidOpenTextDocument, Exit, Initialized,
    Notification as LspNotification, PublishDiagnostics, ShowMessage,
};
use ls_types::request::{Initialize, Request as LspRequest, Shutdown};
use ls_types::{
    ClientCapabilities, Diagnostic, DidChangeTextDocumentParams, DidChangeWatchedFilesParams,
    DidOpenTextDocumentParams, FileChangeType, FileEvent, InitializeParams, InitializeResult,
    InitializedParams, MessageType, Position, PublishDiagnosticsParams, ShowMessageParams,
    TextDocumentContentChangeEvent, TextDocumentIdentifier, TextDocumentItem,
    TextDocumentPositionParams, Uri, VersionedTextDocumentIdentifier, WorkspaceFolder,
};
use lsp_server::{Connection, Message, Notification, Request, RequestId, Response, ResponseError};

/// How long a test waits for any one message before it fails.
const PATIENCE: Duration = Duration::from_secs(60);

/// A client talking to a server on a thread of its own, over channels.
pub(crate) struct Client {
    connection: Connection,
    server: Option<JoinHandle<io::Result<()>>>,
    next_id: i32,
    /// Notifications that came while the client waited for something else,
    /// oldest first.
    notifications: Vec<Notification>,
    /// Answers to requests the client wasn't waiting on.
    stray_responses: Vec<Response>,
    /// Requests from the server, which the client never answers.
    server_requests: Vec<Request>,
}

impl Client {
    /// A server initialized for a client that names no capability.
    pub(crate) fn start() -> Self {
        Self::start_with(ClientCapabilities::default()).0
    }

    /// A server whose workspace is `folder`.
    pub(crate) fn start_in(folder: &Path) -> Self {
        let workspace_folders = vec![WorkspaceFolder {
            uri: Uri::from_file_path(folder).expect("file path URI"),
            name: "workspace".to_owned(),
        }];
        let params = InitializeParams {
            workspace_folders: Some(workspace_folders),
            ..InitializeParams::default()
        };
        Self::initialized(params).0
    }

    pub(crate) fn start_with(capabilities: ClientCapabilities) -> (Self, InitializeResult) {
        Self::initialized(InitializeParams { capabilities, ..InitializeParams::default() })
    }

    fn initialized(params: InitializeParams) -> (Self, InitializeResult) {
        let (client, server) = Connection::memory();
        let serving = thread::Builder::new()
            .name("luck-lsp-test-server".to_owned())
            .spawn(move || luck_lsp::serve(server))
            .expect("the server thread starts");
        let mut started = Self {
            connection: client,
            server: Some(serving),
            next_id: 0,
            notifications: Vec::new(),
            stray_responses: Vec::new(),
            server_requests: Vec::new(),
        };
        let result = started.request::<Initialize>(params);
        started.notify::<Initialized>(InitializedParams {});
        (started, result)
    }

    pub(crate) fn notify<N>(&self, params: N::Params)
    where
        N: LspNotification,
    {
        let notification = Notification::new(N::METHOD.to_owned(), params);
        self.connection.sender.send(notification.into()).expect("the server reads");
    }

    /// Sends a request and returns its id, without waiting for the answer.
    pub(crate) fn send<R>(&mut self, params: R::Params) -> RequestId
    where
        R: LspRequest,
    {
        self.next_id = self.next_id.saturating_add(1);
        let id = RequestId::from(self.next_id);
        let request = Request::new(id.clone(), R::METHOD.to_owned(), params);
        self.connection.sender.send(request.into()).expect("the server reads");
        id
    }

    /// The answer to `id`.
    ///
    /// # Errors
    ///
    /// Returns the error the server answered with.
    pub(crate) fn answer<R>(&mut self, id: &RequestId) -> Result<R::Result, ResponseError>
    where
        R: LspRequest,
    {
        loop {
            match self.receive() {
                Message::Response(response) if response.id == *id => {
                    return response.response_result.map(|result| {
                        serde_json::from_value(result).expect("the result has the request's type")
                    });
                }
                Message::Response(response) => self.stray_responses.push(response),
                Message::Request(request) => self.server_requests.push(request),
                Message::Notification(notification) => self.notifications.push(notification),
            }
        }
    }

    /// # Errors
    ///
    /// Returns the error the server answered with.
    pub(crate) fn request_result<R>(
        &mut self,
        params: R::Params,
    ) -> Result<R::Result, ResponseError>
    where
        R: LspRequest,
    {
        let id = self.send::<R>(params);
        self.answer::<R>(&id)
    }

    pub(crate) fn request<R>(&mut self, params: R::Params) -> R::Result
    where
        R: LspRequest,
    {
        self.request_result::<R>(params)
            .unwrap_or_else(|error| panic!("`{}` failed: {error:?}", R::METHOD))
    }

    fn receive(&self) -> Message {
        self.connection.receiver.recv_timeout(PATIENCE).expect("the server sends in time")
    }

    /// The next diagnostics published for `uri` at `version`, skipping any
    /// published for it before.
    pub(crate) fn diagnostics(&mut self, uri: &Uri, version: Option<i32>) -> Vec<Diagnostic> {
        loop {
            let published = self.next_published(uri);
            if published.version == version {
                return published.diagnostics;
            }
        }
    }

    /// The next diagnostics published for `uri`, whatever the version.
    pub(crate) fn next_published(&mut self, uri: &Uri) -> PublishDiagnosticsParams {
        let is_for_uri = |notification: &Notification| {
            notification.method == PublishDiagnostics::METHOD
                && notification.params.get("uri").and_then(|value| value.as_str())
                    == Some(uri.as_str())
        };
        let buffered = self.notifications.iter().position(is_for_uri);
        let notification = match buffered {
            Some(index) => self.notifications.remove(index),
            None => loop {
                match self.receive() {
                    Message::Notification(notification) if is_for_uri(&notification) => {
                        break notification;
                    }
                    Message::Notification(notification) => self.notifications.push(notification),
                    Message::Response(response) => self.stray_responses.push(response),
                    Message::Request(request) => self.server_requests.push(request),
                }
            },
        };
        serde_json::from_value(notification.params).expect("publishDiagnostics params")
    }

    /// The messages of `message_type` the server has shown so far.
    pub(crate) fn shown(&self, message_type: MessageType) -> Vec<String> {
        self.notifications
            .iter()
            .filter(|notification| notification.method == ShowMessage::METHOD)
            .filter_map(|notification| {
                serde_json::from_value::<ShowMessageParams>(notification.params.clone()).ok()
            })
            .filter(|shown| shown.typ == message_type)
            .map(|shown| shown.message)
            .collect()
    }

    /// The notifications that came while the client waited for something
    /// else, and that no wait took, oldest first.
    pub(crate) fn notifications(&self) -> &[Notification] {
        &self.notifications
    }

    pub(crate) fn stray_responses(&self) -> &[Response] {
        &self.stray_responses
    }

    pub(crate) fn server_requests(&self) -> &[Request] {
        &self.server_requests
    }

    pub(crate) fn open(&self, uri: &Uri, text: &str) {
        self.open_as(uri, "lua", text);
    }

    pub(crate) fn open_as(&self, uri: &Uri, language_id: &str, text: &str) {
        self.notify::<DidOpenTextDocument>(DidOpenTextDocumentParams {
            text_document: TextDocumentItem {
                uri: uri.clone(),
                language_id: language_id.to_owned(),
                version: 1,
                text: text.to_owned(),
            },
        });
    }

    pub(crate) fn replace_all(&self, uri: &Uri, version: i32, text: &str) {
        self.change(
            uri,
            version,
            vec![TextDocumentContentChangeEvent {
                range: None,
                range_length: None,
                text: text.to_owned(),
            }],
        );
    }

    pub(crate) fn change(
        &self,
        uri: &Uri,
        version: i32,
        content_changes: Vec<TextDocumentContentChangeEvent>,
    ) {
        self.notify::<DidChangeTextDocument>(DidChangeTextDocumentParams {
            text_document: VersionedTextDocumentIdentifier { uri: uri.clone(), version },
            content_changes,
        });
    }

    /// What a client sends when the file at `path` changes on disk.
    pub(crate) fn file_changed(&self, path: &Path, typ: FileChangeType) {
        self.notify::<DidChangeWatchedFiles>(DidChangeWatchedFilesParams {
            changes: vec![FileEvent {
                uri: Uri::from_file_path(path).expect("file path URI"),
                typ,
            }],
        });
    }

    /// Asks the server to shut down and exit.
    ///
    /// # Errors
    ///
    /// Returns the error the server ended with.
    pub(crate) fn shut_down(mut self) -> io::Result<()> {
        self.request::<Shutdown>(());
        self.exit()
    }

    /// Sends `exit` alone.
    ///
    /// # Errors
    ///
    /// Returns the error the server ended with.
    pub(crate) fn exit(mut self) -> io::Result<()> {
        self.notify::<Exit>(());
        // Only this or a drop takes the server, and each ends the client.
        match self.server.take().map(JoinHandle::join) {
            Some(Ok(ended)) => ended,
            Some(Err(payload)) => panic::resume_unwind(payload),
            None => Ok(()),
        }
    }
}

impl Drop for Client {
    /// Hangs up, which ends a server still running, and waits for it.
    fn drop(&mut self) {
        let (unconnected, _) = unbounded();
        drop(mem::replace(&mut self.connection.sender, unconnected));
        if let Some(server) = self.server.take() {
            let _ended = server.join();
        }
    }
}

/// A URI that never names a file on disk.
pub(crate) fn workspace_uri(name: &str) -> Uri {
    Uri::from_str(&format!("file:///tmp/{name}")).expect("hand-rolled URI is valid")
}

pub(crate) fn position_params(
    uri: &Uri,
    line: u32,
    utf16_column: u32,
) -> TextDocumentPositionParams {
    TextDocumentPositionParams {
        text_document: TextDocumentIdentifier { uri: uri.clone() },
        position: Position { line, character: utf16_column },
    }
}

pub(crate) fn document(uri: &Uri) -> TextDocumentIdentifier {
    TextDocumentIdentifier { uri: uri.clone() }
}
