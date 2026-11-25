use ls_types::notification::{DidChangeWatchedFiles, Notification as _};
use ls_types::request::{RegisterCapability, Request as _};
use ls_types::{
    CodeActionKind, CodeActionOptions, CodeActionProviderCapability, CompletionOptions,
    DidChangeWatchedFilesRegistrationOptions, DocumentLinkOptions, FileSystemWatcher,
    FoldingRangeProviderCapability, GlobPattern, HoverProviderCapability, InitializeResult, OneOf,
    Registration, RegistrationParams, RenameOptions, SelectionRangeProviderCapability,
    SemanticTokensFullOptions, SemanticTokensOptions, SemanticTokensRegistrationOptions,
    SemanticTokensServerCapabilities, ServerCapabilities, ServerInfo, SignatureHelpOptions,
    StaticRegistrationOptions, TextDocumentRegistrationOptions, TextDocumentSyncCapability,
    TextDocumentSyncKind, TextDocumentSyncOptions, WorkDoneProgressOptions,
    WorkspaceFoldersServerCapabilities, WorkspaceServerCapabilities,
};
use lsp_server::{Request, RequestId};

use crate::line_index::PositionEncoding;
use crate::providers::{code_action, semantic_tokens};

/// The id of the one request the server sends, which needs no answer.
const WATCHER_REGISTRATION: &str = "luck-file-watchers";

/// The answer to `initialize`, with columns in `encoding`.
pub(crate) fn initialize_result(encoding: PositionEncoding) -> InitializeResult {
    InitializeResult {
        capabilities: ServerCapabilities {
            position_encoding: Some(encoding.into()),
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

/// The request that has the client watch every file whose change can change
/// an answer.
pub(crate) fn watcher_registration() -> Request {
    let watchers = [
        // Any JSON or TOML file, since a `luck.json` can extend a file, or
        // name a std definition, of any name. A change touching none the
        // projects read is ignored.
        "**/*.json",
        "**/*.toml",
        "**/.editorconfig",
        "**/.luaurc",
        "**/*.{luau,lua}",
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
    Request::new(
        RequestId::from(WATCHER_REGISTRATION.to_owned()),
        RegisterCapability::METHOD.to_owned(),
        RegistrationParams { registrations: vec![registration] },
    )
}
