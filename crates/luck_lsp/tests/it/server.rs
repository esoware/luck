//! Tests that call the `LanguageServer` methods directly, with
//! `CapturedNotifier` standing in for the client and no transport.

#![expect(
    clippy::missing_panics_doc,
    reason = "a test panics to fail, so a Panics section says nothing"
)]
#![cfg(test)]

extern crate alloc;

use alloc::sync::Arc;
use core::str::FromStr as _;
use std::fs;

use luck_lsp::{Backend, Notifier};
use luck_token::index_to_offset;
use tokio::sync::RwLock;
use tower_lsp::LanguageServer as _;
use tower_lsp::lsp_types::{
    CodeActionContext, CodeActionOrCommand, CodeActionParams, CompletionParams, CompletionResponse,
    Diagnostic, DidChangeTextDocumentParams, DidOpenTextDocumentParams, DocumentFormattingParams,
    DocumentHighlightKind, DocumentHighlightParams, DocumentLinkParams,
    DocumentRangeFormattingParams, DocumentSymbolParams, DocumentSymbolResponse,
    FoldingRangeParams, FormattingOptions, GotoDefinitionParams, GotoDefinitionResponse,
    HoverContents, HoverParams, InitializeParams, MessageType, PartialResultParams, Position,
    PrepareRenameResponse, Range, ReferenceContext, ReferenceParams, Registration, RenameParams,
    SelectionRangeParams, SemanticTokensParams, SemanticTokensRangeParams,
    SemanticTokensRangeResult, SemanticTokensResult, SignatureHelpParams,
    TextDocumentContentChangeEvent, TextDocumentIdentifier, TextDocumentItem,
    TextDocumentPositionParams, Url, VersionedTextDocumentIdentifier, WorkDoneProgressParams,
    WorkspaceSymbolParams,
};

/// A single captured `publishDiagnostics` notification.
type PublishedRecord = (Url, Vec<Diagnostic>, Option<i32>);

/// A single captured `showMessage` / `logMessage` notification.
type MessageRecord = (MessageType, String);

/// A [`Notifier`] that records everything the server sends.
#[derive(Default, Clone, Debug)]
struct CapturedNotifier {
    published: Arc<RwLock<Vec<PublishedRecord>>>,
    messages: Arc<RwLock<Vec<MessageRecord>>>,
}

impl Notifier for CapturedNotifier {
    async fn publish_diagnostics(
        &self,
        uri: Url,
        diagnostics: Vec<Diagnostic>,
        version: Option<i32>,
    ) {
        self.published.write().await.push((uri, diagnostics, version));
    }

    async fn show_message(&self, message_type: MessageType, message: String) {
        self.messages.write().await.push((message_type, message));
    }

    async fn log_message(&self, message_type: MessageType, message: String) {
        self.messages.write().await.push((message_type, message));
    }

    async fn register_capability(&self, _registrations: Vec<Registration>) {}
}

impl CapturedNotifier {
    /// The diagnostics last published for `uri`.
    async fn diagnostics_for(&self, uri: &Url) -> Vec<Diagnostic> {
        let published = self.published.read().await;
        published
            .iter()
            .rev()
            .find(|(other, _, _)| other == uri)
            .map(|(_, diagnostics, _)| diagnostics.clone())
            .unwrap_or_default()
    }
}

fn workspace_uri(name: &str) -> Url {
    // `file:///` URIs work cross-platform for tests that never hit disk.
    Url::from_str(&format!("file:///tmp/{name}")).expect("hand-rolled URI is valid")
}

fn position_params(uri: &Url, line: u32, utf16_column: u32) -> TextDocumentPositionParams {
    TextDocumentPositionParams {
        text_document: TextDocumentIdentifier { uri: uri.clone() },
        position: Position { line, character: utf16_column },
    }
}

async fn open(server: &Backend<CapturedNotifier>, uri: &Url, text: &str) {
    server
        .did_open(DidOpenTextDocumentParams {
            text_document: TextDocumentItem {
                uri: uri.clone(),
                language_id: "lua".to_owned(),
                version: 1,
                text: text.to_owned(),
            },
        })
        .await;
}

fn formatting_options() -> FormattingOptions {
    FormattingOptions { tab_size: 4, insert_spaces: false, ..Default::default() }
}

#[tokio::test]
async fn initialize_advertises_required_capabilities() {
    let server = Backend::new(CapturedNotifier::default());
    let result = server.initialize(InitializeParams::default()).await.expect("initialize succeeds");
    let capabilities = result.capabilities;
    assert!(capabilities.document_formatting_provider.is_some(), "formatting capability missing");
    assert!(
        capabilities.document_range_formatting_provider.is_some(),
        "range formatting capability missing"
    );
    assert!(capabilities.text_document_sync.is_some(), "text sync capability missing");
}

#[tokio::test]
async fn format_on_save_returns_expected_edits() {
    let notifier = CapturedNotifier::default();
    let server = Backend::new(notifier.clone());
    let uri = workspace_uri("ugly.lua");
    // No spaces around `=`, extra spaces, and trailing whitespace.
    let ugly = "local x=1   \nlocal     y=2";
    open(&server, &uri, ugly).await;

    let edits = server
        .formatting(DocumentFormattingParams {
            text_document: TextDocumentIdentifier { uri: uri.clone() },
            options: formatting_options(),
            work_done_progress_params: WorkDoneProgressParams::default(),
        })
        .await
        .expect("formatting returned an error")
        .expect("formatter returned no edits");

    assert!(!edits.is_empty(), "ugly source should produce at least one edit");
    let new_text = &edits[0].new_text;
    assert!(new_text.contains("local x = 1"), "formatter normalizes spacing: {new_text}");
    assert!(new_text.contains("local y = 2"), "formatter normalizes spacing: {new_text}");
    assert_ne!(new_text.as_str(), ugly, "formatter must change ugly source");
}

#[tokio::test]
async fn lint_diagnostics_are_opt_in_without_config() {
    let notifier = CapturedNotifier::default();
    let server = Backend::new(notifier.clone());
    let uri = workspace_uri("unused.lua");
    // Linting is opt-in. With no `luck.json` for `/tmp/unused.lua`, only
    // parse errors surface, so the unused local must not be flagged.
    open(&server, &uri, "local unused = 1\n").await;

    let diags = notifier.diagnostics_for(&uri).await;
    assert!(diags.is_empty(), "lint rules must be off without a lint config: {diags:?}");
}

#[tokio::test]
async fn parse_errors_surface_without_config() {
    let notifier = CapturedNotifier::default();
    let server = Backend::new(notifier.clone());
    let uri = workspace_uri("broken.lua");
    // Parse errors surface even with linting off.
    open(&server, &uri, "local x =\n").await;

    let diags = notifier.diagnostics_for(&uri).await;
    assert!(!diags.is_empty(), "parse errors must surface even when lint rules are disabled");
}

#[tokio::test]
async fn range_formatting_preserves_out_of_range_content() {
    // The backend sends one whole-document edit, so this checks that text
    // outside the range survives, not that the edit is narrowed.
    let notifier = CapturedNotifier::default();
    let server = Backend::new(notifier.clone());
    let uri = workspace_uri("partial.lua");
    let source = "local first=1\nlocal middle=2\nlocal last=3\n";
    open(&server, &uri, source).await;

    // Only the middle line is in range; the first and third must come back
    // byte-identical.
    let edits = server
        .range_formatting(DocumentRangeFormattingParams {
            text_document: TextDocumentIdentifier { uri: uri.clone() },
            range: Range {
                start: Position { line: 1, character: 0 },
                end: Position { line: 1, character: 15 },
            },
            options: formatting_options(),
            work_done_progress_params: WorkDoneProgressParams::default(),
        })
        .await
        .expect("range formatting errored")
        .expect("formatter returned no edits");
    assert!(!edits.is_empty(), "range formatter produced no edits");
    let new_text = &edits[0].new_text;
    assert!(new_text.contains("local middle = 2"), "middle line must be reformatted: {new_text}");
    // Lines outside the range must stay verbatim (no inserted spaces around `=`).
    assert!(
        new_text.contains("local first=1"),
        "first line was reformatted but shouldn't have been: {new_text}"
    );
    assert!(
        new_text.contains("local last=3"),
        "last line was reformatted but shouldn't have been: {new_text}"
    );
}

#[tokio::test]
async fn formatting_unknown_document_returns_none() {
    let server = Backend::new(CapturedNotifier::default());
    let uri = workspace_uri("missing.lua");
    let result = server
        .formatting(DocumentFormattingParams {
            text_document: TextDocumentIdentifier { uri },
            options: formatting_options(),
            work_done_progress_params: WorkDoneProgressParams::default(),
        })
        .await
        .expect("formatting errored");
    assert!(result.is_none(), "formatting an unopened document should return None");
}

#[tokio::test]
async fn shutdown_succeeds() {
    let server = Backend::new(CapturedNotifier::default());
    server.shutdown().await.expect("shutdown errored");
}

#[tokio::test]
async fn hover_renders_stdlib_signature() {
    let server = Backend::new(CapturedNotifier::default());
    let uri = workspace_uri("hover.lua");
    // Cursor on `format` in `string.format`.
    open(&server, &uri, "local formatted = string.format(\"%d\", 1)\n").await;

    let hover = server
        .hover(HoverParams {
            text_document_position_params: position_params(&uri, 0, 27),
            work_done_progress_params: WorkDoneProgressParams::default(),
        })
        .await
        .expect("hover errored")
        .expect("cursor on string.format should hover");
    let HoverContents::Markup(markup) = hover.contents else {
        panic!("hover should return markup contents");
    };
    assert!(
        markup.value.contains("string.format"),
        "hover markdown should name the stdlib symbol: {}",
        markup.value
    );
    assert!(
        markup.value.contains("function"),
        "hover markdown should render the function signature: {}",
        markup.value
    );
}

#[tokio::test]
async fn completion_lists_locals_and_stdlib_globals() {
    let server = Backend::new(CapturedNotifier::default());
    let uri = workspace_uri("completion.lua");
    // A visible local plus a bare-identifier completion site inside a call.
    open(&server, &uri, "local myLocal = 1\nprint(m)\n").await;

    let response = server
        .completion(CompletionParams {
            text_document_position: position_params(&uri, 1, 7),
            work_done_progress_params: WorkDoneProgressParams::default(),
            partial_result_params: PartialResultParams::default(),
            context: None,
        })
        .await
        .expect("completion errored")
        .expect("bare identifier completion should return items");
    let CompletionResponse::Array(items) = response else {
        panic!("completion should return an array response");
    };
    assert!(
        items.iter().any(|item| item.label == "myLocal"),
        "completion should offer the visible local `myLocal`"
    );
    assert!(
        items.iter().any(|item| item.label == "print"),
        "completion should offer the stdlib global `print`"
    );
}

#[tokio::test]
async fn signature_help_marks_active_parameter() {
    let server = Backend::new(CapturedNotifier::default());
    let uri = workspace_uri("signature.lua");
    // Cursor lands on the second argument `5`, so parameter index 1 is active.
    open(&server, &uri, "local x = string.format(\"a\", 5)\n").await;

    let help = server
        .signature_help(SignatureHelpParams {
            context: None,
            text_document_position_params: position_params(&uri, 0, 29),
            work_done_progress_params: WorkDoneProgressParams::default(),
        })
        .await
        .expect("signature help errored")
        .expect("cursor inside call args should return signature help");
    assert_eq!(
        help.active_parameter,
        Some(1),
        "the second argument should be the active parameter"
    );
    assert!(
        help.signatures[0].label.contains("string.format"),
        "signature label should name the callee: {}",
        help.signatures[0].label
    );
}

#[tokio::test]
async fn document_symbol_reports_named_function() {
    let server = Backend::new(CapturedNotifier::default());
    let uri = workspace_uri("symbols.lua");
    open(&server, &uri, "local function greet(name)\n    return name\nend\n").await;

    let response = server
        .document_symbol(DocumentSymbolParams {
            text_document: TextDocumentIdentifier { uri: uri.clone() },
            work_done_progress_params: WorkDoneProgressParams::default(),
            partial_result_params: PartialResultParams::default(),
        })
        .await
        .expect("document symbol errored")
        .expect("document with a function should return symbols");
    let DocumentSymbolResponse::Nested(symbols) = response else {
        panic!("document symbols should be nested");
    };
    assert!(
        symbols.iter().any(|symbol| symbol.name == "greet"),
        "outline should contain the `greet` function symbol"
    );
}

#[tokio::test]
async fn folding_range_covers_multiline_function() {
    let server = Backend::new(CapturedNotifier::default());
    let uri = workspace_uri("folding.lua");
    open(&server, &uri, "local function greet(name)\n    return name\nend\n").await;

    let ranges = server
        .folding_range(FoldingRangeParams {
            text_document: TextDocumentIdentifier { uri: uri.clone() },
            work_done_progress_params: WorkDoneProgressParams::default(),
            partial_result_params: PartialResultParams::default(),
        })
        .await
        .expect("folding range errored")
        .expect("multi-line function should return folding ranges");
    assert!(
        ranges.iter().any(|range| range.start_line == 0 && range.end_line >= 2),
        "the three-line function body should produce a fold: {ranges:?}"
    );
}

#[tokio::test]
async fn document_highlight_finds_all_occurrences() {
    let server = Backend::new(CapturedNotifier::default());
    let uri = workspace_uri("highlight.lua");
    open(&server, &uri, "local count = 1\ncount = count + 1\n").await;

    // Cursor on the write target `count` on line 1.
    let highlights = server
        .document_highlight(DocumentHighlightParams {
            text_document_position_params: position_params(&uri, 1, 2),
            work_done_progress_params: WorkDoneProgressParams::default(),
            partial_result_params: PartialResultParams::default(),
        })
        .await
        .expect("document highlight errored")
        .expect("name under cursor should highlight");
    assert!(highlights.len() >= 2, "`count` occurs multiple times: {highlights:?}");
    assert!(
        highlights.iter().any(|highlight| highlight.kind == Some(DocumentHighlightKind::WRITE)),
        "a reassignment of `count` should be a WRITE highlight"
    );
}

#[tokio::test]
async fn selection_range_expands_from_identifier() {
    let server = Backend::new(CapturedNotifier::default());
    let uri = workspace_uri("selection.lua");
    open(&server, &uri, "local value = alpha.beta + 1\n").await;

    // Cursor on `beta` inside the `alpha.beta` field access.
    let ranges = server
        .selection_range(SelectionRangeParams {
            work_done_progress_params: WorkDoneProgressParams::default(),
            partial_result_params: PartialResultParams::default(),
            text_document: TextDocumentIdentifier { uri: uri.clone() },
            positions: vec![Position { line: 0, character: 20 }],
        })
        .await
        .expect("selection range errored")
        .expect("selection range should return a chain");
    // The chain nests from the widest enclosing node down to the narrowest.
    let root = &ranges[0];
    let mut deepest = root;
    let mut depth = 1u32;
    while let Some(parent) = deepest.parent.as_ref() {
        deepest = parent;
        depth += 1;
    }
    assert!(depth >= 2u32, "selection should expand through at least one enclosing node: {root:?}");
    let root_width = root.range.end.character - root.range.start.character;
    let deepest_width = deepest.range.end.character - deepest.range.start.character;
    assert!(
        deepest_width < root_width,
        "the narrowest selection must cover less than the whole statement: {root:?}"
    );
}

#[tokio::test]
async fn semantic_tokens_cover_every_identifier() {
    let server = Backend::new(CapturedNotifier::default());
    let uri = workspace_uri("tokens.lua");
    open(&server, &uri, "print(math.pi)\nlocal mine = my_helper(1)\n").await;

    let result = server
        .semantic_tokens_full(SemanticTokensParams {
            work_done_progress_params: WorkDoneProgressParams::default(),
            partial_result_params: PartialResultParams::default(),
            text_document: TextDocumentIdentifier { uri: uri.clone() },
        })
        .await
        .expect("semantic tokens errored")
        .expect("a document with identifiers should tokenize");
    let SemanticTokensResult::Tokens(tokens) = result else {
        panic!("expected a full semantic token set");
    };
    // print, math, pi, mine, my_helper: every identifier gets a token,
    // including stdlib members after a dot and unresolved globals.
    assert_eq!(tokens.data.len(), 5, "every identifier should tokenize: {:?}", tokens.data);
}

#[tokio::test]
async fn document_link_resolves_require_path() {
    let directory = tempfile::tempdir().expect("tempdir");
    // Pin the project root here. Discovery walks up past the temp directory,
    // so without a config of its own this would resolve `?.lua` against
    // whatever `luck.json` happens to sit above the machine's temp root.
    fs::write(directory.path().join("luck.json"), "{}").expect("write luck.json");
    // A neighboring module the `require` should resolve to.
    fs::write(directory.path().join("foo.lua"), "return {}\n").expect("write foo.lua");
    let main_path = directory.path().join("main.lua");
    fs::write(&main_path, "local foo = require(\"foo\")\n").expect("write main.lua");
    let uri = Url::from_file_path(&main_path).expect("file path URI");

    let server = Backend::new(CapturedNotifier::default());
    open(&server, &uri, "local foo = require(\"foo\")\n").await;

    let links = server
        .document_link(DocumentLinkParams {
            text_document: TextDocumentIdentifier { uri: uri.clone() },
            work_done_progress_params: WorkDoneProgressParams::default(),
            partial_result_params: PartialResultParams::default(),
        })
        .await
        .expect("document link errored")
        .expect("require call should produce a document link");
    assert_eq!(links.len(), 1, "exactly one require link expected");
    assert_eq!(
        links[0].tooltip.as_deref(),
        Some("foo"),
        "link tooltip should carry the module name"
    );
    let target = links[0].target.as_ref().expect("link should have a target");
    assert!(target.path().ends_with("foo.lua"), "link should resolve to foo.lua: {target}");
}

#[tokio::test]
async fn document_link_resolves_luau_relative_import() {
    // Luau relative imports go through the shared resolver, so `./sibling`
    // survives instead of being mangled by a dots-to-slashes rewrite.
    let directory = tempfile::tempdir().expect("tempdir");
    fs::write(directory.path().join("sibling.luau"), "return {}\n").expect("write sibling.luau");
    let main_path = directory.path().join("main.luau");
    let source = "local s = require(\"./sibling\")\n";
    fs::write(&main_path, source).expect("write main.luau");
    let uri = Url::from_file_path(&main_path).expect("file path URI");

    let server = Backend::new(CapturedNotifier::default());
    open(&server, &uri, source).await;

    let links = server
        .document_link(DocumentLinkParams {
            text_document: TextDocumentIdentifier { uri: uri.clone() },
            work_done_progress_params: WorkDoneProgressParams::default(),
            partial_result_params: PartialResultParams::default(),
        })
        .await
        .expect("document link errored")
        .expect("relative require should produce a document link");
    assert_eq!(links.len(), 1, "exactly one require link expected");
    let target = links[0].target.as_ref().expect("link should have a target");
    assert!(
        target.path().ends_with("sibling.luau"),
        "link should resolve to sibling.luau: {target}"
    );
}

#[tokio::test]
async fn document_link_resolves_luau_alias_import() {
    // `@alias` imports resolve through the `.luaurc` alias table.
    let directory = tempfile::tempdir().expect("tempdir");
    let shared = directory.path().join("shared");
    fs::create_dir_all(&shared).expect("mkdir shared");
    fs::write(shared.join("common.luau"), "return {}\n").expect("write common.luau");
    fs::write(
        directory.path().join(".luaurc"),
        serde_json::json!({ "aliases": { "shared": "./shared" } }).to_string(),
    )
    .expect("write .luaurc");
    let main_path = directory.path().join("main.luau");
    let source = "local c = require(\"@shared/common\")\n";
    fs::write(&main_path, source).expect("write main.luau");
    let uri = Url::from_file_path(&main_path).expect("file path URI");

    let server = Backend::new(CapturedNotifier::default());
    open(&server, &uri, source).await;

    let links = server
        .document_link(DocumentLinkParams {
            text_document: TextDocumentIdentifier { uri: uri.clone() },
            work_done_progress_params: WorkDoneProgressParams::default(),
            partial_result_params: PartialResultParams::default(),
        })
        .await
        .expect("document link errored")
        .expect("alias require should produce a document link");
    assert_eq!(links.len(), 1, "exactly one require link expected");
    let target = links[0].target.as_ref().expect("link should have a target");
    assert!(
        target.path().ends_with("shared/common.luau"),
        "alias link should resolve to shared/common.luau: {target}"
    );
}

#[tokio::test]
async fn code_action_offers_lint_fix() {
    let directory = tempfile::tempdir().expect("tempdir");
    // Opt in to linting and enable a fixable style rule.
    fs::write(
        directory.path().join("luck.json"),
        r#"{ "lint": { "rule_overrides": { "redundant_nil_init": { "enabled": true } } } }"#,
    )
    .expect("write luck.json");
    let file_path = directory.path().join("nilinit.lua");
    fs::write(&file_path, "local x = nil\n").expect("write lua file");
    let uri = Url::from_file_path(&file_path).expect("file path URI");

    let notifier = CapturedNotifier::default();
    let server = Backend::new(notifier.clone());
    open(&server, &uri, "local x = nil\n").await;

    // The enabled rule should have surfaced a diagnostic on open.
    let diagnostics = notifier.diagnostics_for(&uri).await;
    assert!(!diagnostics.is_empty(), "the enabled lint rule should publish a diagnostic");

    let actions = server
        .code_action(CodeActionParams {
            text_document: TextDocumentIdentifier { uri: uri.clone() },
            range: Range {
                start: Position { line: 0, character: 0 },
                end: Position { line: 0, character: 13 },
            },
            context: CodeActionContext { diagnostics, only: None, trigger_kind: None },
            work_done_progress_params: WorkDoneProgressParams::default(),
            partial_result_params: PartialResultParams::default(),
        })
        .await
        .expect("code action errored")
        .expect("a fixable diagnostic should yield code actions");
    let titles: Vec<&str> = actions
        .iter()
        .map(|action| match action {
            CodeActionOrCommand::CodeAction(code_action) => code_action.title.as_str(),
            CodeActionOrCommand::Command(command) => command.title.as_str(),
        })
        .collect();
    assert!(
        titles.iter().any(|title| title.contains("drop redundant `= nil` initializer")),
        "the redundant-nil fix should be offered: {titles:?}"
    );
}

#[tokio::test]
async fn syntax_tree_request_dumps_ast() {
    let server = Backend::new(CapturedNotifier::default());
    let uri = workspace_uri("tree.lua");
    open(&server, &uri, "local x = 1\n").await;

    let dump = server
        .syntax_tree_request(serde_json::json!({
            "textDocument": { "uri": uri.as_str() }
        }))
        .await
        .expect("syntax tree request errored");
    let text = dump.as_str().expect("syntax tree returns a string");
    assert!(
        text.contains("LocalAssignment"),
        "the AST dump should contain the parsed statement node: {text}"
    );
}

#[tokio::test]
async fn goto_definition_jumps_to_declaration() {
    let server = Backend::new(CapturedNotifier::default());
    let uri = workspace_uri("definition.lua");
    open(&server, &uri, "local value = 1\nprint(value)\n").await;

    let response = server
        .goto_definition(GotoDefinitionParams {
            text_document_position_params: position_params(&uri, 1, 8),
            work_done_progress_params: WorkDoneProgressParams::default(),
            partial_result_params: PartialResultParams::default(),
        })
        .await
        .expect("goto definition errored")
        .expect("local should have a definition");
    let GotoDefinitionResponse::Scalar(location) = response else {
        panic!("expected scalar location");
    };
    assert_eq!(location.uri, uri);
    assert_eq!(location.range.start.line, 0);
    assert_eq!(location.range.start.character, 6);
}

#[tokio::test]
async fn goto_definition_none_for_global() {
    let server = Backend::new(CapturedNotifier::default());
    let uri = workspace_uri("definition_global.lua");
    open(&server, &uri, "print(1)\n").await;

    let response = server
        .goto_definition(GotoDefinitionParams {
            text_document_position_params: position_params(&uri, 0, 2),
            work_done_progress_params: WorkDoneProgressParams::default(),
            partial_result_params: PartialResultParams::default(),
        })
        .await
        .expect("goto definition errored");
    assert!(response.is_none(), "stdlib globals have no definition site");
}

#[tokio::test]
async fn references_are_scope_exact_for_shadowed_names() {
    let server = Backend::new(CapturedNotifier::default());
    let uri = workspace_uri("references.lua");
    open(&server, &uri, "local x = 1\ndo\n    local x = 2\n    print(x)\nend\nprint(x)\n").await;

    // Cursor on the INNER x read at line 3.
    let locations = server
        .references(ReferenceParams {
            text_document_position: position_params(&uri, 3, 10),
            work_done_progress_params: WorkDoneProgressParams::default(),
            partial_result_params: PartialResultParams::default(),
            context: ReferenceContext { include_declaration: true },
        })
        .await
        .expect("references errored")
        .expect("local should have references");
    // Inner x: declaration on line 2 + read on line 3. The outer x's
    // spans (lines 0 and 5) must not appear.
    assert_eq!(locations.len(), 2, "{locations:?}");
    assert!(
        locations
            .iter()
            .all(|location| location.range.start.line == 2 || location.range.start.line == 3),
        "outer x leaked into inner x references: {locations:?}"
    );
}

#[tokio::test]
async fn references_exclude_declaration_when_not_requested() {
    let server = Backend::new(CapturedNotifier::default());
    let uri = workspace_uri("references_no_decl.lua");
    open(&server, &uri, "local count = 1\ncount = count + 1\n").await;

    let locations = server
        .references(ReferenceParams {
            text_document_position: position_params(&uri, 0, 8),
            work_done_progress_params: WorkDoneProgressParams::default(),
            partial_result_params: PartialResultParams::default(),
            context: ReferenceContext { include_declaration: false },
        })
        .await
        .expect("references errored")
        .expect("local should have references");
    assert!(
        locations.iter().all(|location| location.range.start.line == 1),
        "declaration should be excluded: {locations:?}"
    );
}

#[tokio::test]
async fn prepare_rename_accepts_local_and_rejects_global() {
    let server = Backend::new(CapturedNotifier::default());
    let uri = workspace_uri("prepare_rename.lua");
    open(&server, &uri, "local count = 1\nprint(count)\n").await;

    let prepared = server
        .prepare_rename(position_params(&uri, 0, 8))
        .await
        .expect("prepare rename errored")
        .expect("local should be renameable");
    let PrepareRenameResponse::RangeWithPlaceholder { placeholder, .. } = prepared else {
        panic!("expected range with placeholder");
    };
    assert_eq!(placeholder, "count");

    let on_global =
        server.prepare_rename(position_params(&uri, 1, 2)).await.expect("prepare rename errored");
    assert!(on_global.is_none(), "globals must not be renameable");
}

#[tokio::test]
async fn rename_updates_declaration_and_references() {
    let server = Backend::new(CapturedNotifier::default());
    let uri = workspace_uri("rename.lua");
    open(&server, &uri, "local count = 1\ncount = count + 1\n").await;

    let edit = server
        .rename(RenameParams {
            text_document_position: position_params(&uri, 0, 8),
            new_name: "total".to_owned(),
            work_done_progress_params: WorkDoneProgressParams::default(),
        })
        .await
        .expect("rename errored")
        .expect("rename should produce an edit");
    let edits = edit.changes.expect("changes map").remove(&uri).expect("edits for the document");
    assert_eq!(edits.len(), 3, "declaration + two references: {edits:?}");
    assert!(edits.iter().all(|text_edit| text_edit.new_text == "total"));
}

#[tokio::test]
async fn rename_rejects_keywords_and_used_names() {
    let server = Backend::new(CapturedNotifier::default());
    let uri = workspace_uri("rename_reject.lua");
    open(&server, &uri, "local count = 1\nlocal other = count\n").await;

    let keyword = server
        .rename(RenameParams {
            text_document_position: position_params(&uri, 0, 8),
            new_name: "end".to_owned(),
            work_done_progress_params: WorkDoneProgressParams::default(),
        })
        .await;
    assert!(keyword.is_err(), "keywords are not identifiers");

    let captured = server
        .rename(RenameParams {
            text_document_position: position_params(&uri, 0, 8),
            new_name: "other".to_owned(),
            work_done_progress_params: WorkDoneProgressParams::default(),
        })
        .await;
    assert!(captured.is_err(), "in-use names must be rejected");

    let builtin = server
        .rename(RenameParams {
            text_document_position: position_params(&uri, 0, 8),
            new_name: "print".to_owned(),
            work_done_progress_params: WorkDoneProgressParams::default(),
        })
        .await;
    assert!(builtin.is_err(), "stdlib globals must be rejected");
}

#[tokio::test]
async fn prepare_rename_handles_utf16_offsets() {
    let server = Backend::new(CapturedNotifier::default());
    let uri = workspace_uri("rename_utf16.lua");
    // The crab emoji is two UTF-16 units; positions after it require
    // real UTF-16 conversion, not byte arithmetic.
    open(&server, &uri, "local s = \"\u{1F980}\" local target = 1\nprint(s, target)\n").await;

    let prepared = server
        .prepare_rename(position_params(&uri, 0, 23))
        .await
        .expect("prepare rename errored")
        .expect("target should be renameable");
    let PrepareRenameResponse::RangeWithPlaceholder { placeholder, .. } = prepared else {
        panic!("expected range with placeholder");
    };
    assert_eq!(placeholder, "target");
}

#[tokio::test]
async fn workspace_symbols_filter_across_open_documents() {
    let server = Backend::new(CapturedNotifier::default());
    let first = workspace_uri("ws_first.lua");
    let second = workspace_uri("ws_second.lua");
    open(&server, &first, "local function setup_camera() end\nsetup_camera()\n").await;
    open(&server, &second, "local function teardown() end\nteardown()\n").await;

    let symbols = server
        .symbol(WorkspaceSymbolParams {
            query: "camera".to_owned(),
            work_done_progress_params: WorkDoneProgressParams::default(),
            partial_result_params: PartialResultParams::default(),
        })
        .await
        .expect("workspace symbol errored")
        .expect("symbols expected");
    assert_eq!(symbols.len(), 1, "{symbols:?}");
    assert_eq!(symbols[0].name, "setup_camera");
    assert_eq!(symbols[0].location.uri, first);

    let all = server
        .symbol(WorkspaceSymbolParams {
            query: String::new(),
            work_done_progress_params: WorkDoneProgressParams::default(),
            partial_result_params: PartialResultParams::default(),
        })
        .await
        .expect("workspace symbol errored")
        .expect("symbols expected");
    assert!(all.len() >= 2, "empty query returns everything: {all:?}");
}

#[tokio::test]
async fn semantic_tokens_range_returns_subset() {
    let server = Backend::new(CapturedNotifier::default());
    let uri = workspace_uri("tokens_range.lua");
    open(&server, &uri, "print(1)\nprint(2)\nprint(3)\n").await;

    let full_result = server
        .semantic_tokens_full(SemanticTokensParams {
            text_document: TextDocumentIdentifier { uri: uri.clone() },
            work_done_progress_params: WorkDoneProgressParams::default(),
            partial_result_params: PartialResultParams::default(),
        })
        .await
        .expect("semantic tokens errored")
        .expect("tokens expected");
    let SemanticTokensResult::Tokens(full) = full_result else {
        panic!("expected full tokens");
    };

    let ranged_result = server
        .semantic_tokens_range(SemanticTokensRangeParams {
            text_document: TextDocumentIdentifier { uri: uri.clone() },
            range: Range {
                start: Position { line: 1, character: 0 },
                end: Position { line: 1, character: 11 },
            },
            work_done_progress_params: WorkDoneProgressParams::default(),
            partial_result_params: PartialResultParams::default(),
        })
        .await
        .expect("semantic tokens range errored")
        .expect("tokens expected");
    let SemanticTokensRangeResult::Tokens(ranged) = ranged_result else {
        panic!("expected ranged tokens");
    };
    assert!(!ranged.data.is_empty(), "line 1 has tokens");
    assert!(
        ranged.data.len() < full.data.len(),
        "range must be a strict subset: {} vs {}",
        ranged.data.len(),
        full.data.len()
    );
    // Delta re-encoding: the first ranged token is absolute (line 1).
    assert_eq!(ranged.data[0].delta_line, 1);
}

#[tokio::test]
async fn collectgarbage_option_string_completes() {
    let server = Backend::new(CapturedNotifier::default());
    let uri = workspace_uri("gc.lua");
    let text = "collectgarbage('c')";
    open(&server, &uri, text).await;

    let utf16_column = index_to_offset(text.find("('").expect("literal") + 2);
    let response = server
        .completion(CompletionParams {
            text_document_position: position_params(&uri, 0, utf16_column),
            work_done_progress_params: WorkDoneProgressParams::default(),
            partial_result_params: PartialResultParams::default(),
            context: None,
        })
        .await
        .expect("completion errored")
        .expect("constant completion should return items");
    let CompletionResponse::Array(items) = response else {
        panic!("completion should return an array response");
    };
    assert!(items.iter().any(|item| item.label == "collect"));
    assert!(items.iter().any(|item| item.label == "count"));
    assert!(
        !items.iter().any(|item| item.label == "print"),
        "no identifier completion inside a constant string"
    );
}

#[tokio::test]
async fn formatting_unparsable_file_warns_and_returns_none() {
    let notifier = CapturedNotifier::default();
    let server = Backend::new(notifier.clone());
    let uri = workspace_uri("unparsable.lua");
    open(&server, &uri, "local x =\n").await;

    let formatted = server
        .formatting(DocumentFormattingParams {
            text_document: TextDocumentIdentifier { uri: uri.clone() },
            options: formatting_options(),
            work_done_progress_params: WorkDoneProgressParams::default(),
        })
        .await
        .expect("formatting errored");
    assert!(formatted.is_none(), "{formatted:?}");
    let range_formatted = server
        .range_formatting(DocumentRangeFormattingParams {
            text_document: TextDocumentIdentifier { uri: uri.clone() },
            range: Range::new(Position::new(0, 0), Position::new(0, 9)),
            options: formatting_options(),
            work_done_progress_params: WorkDoneProgressParams::default(),
        })
        .await
        .expect("range formatting errored");
    assert!(range_formatted.is_none(), "{range_formatted:?}");

    let warnings: Vec<String> = notifier
        .messages
        .read()
        .await
        .iter()
        .filter(|(message_type, _)| *message_type == MessageType::WARNING)
        .map(|(_, message)| message.clone())
        .collect();
    assert_eq!(warnings.len(), 2, "{warnings:?}");
    assert!(warnings[0].starts_with("luck: can't format file (parse error): "), "{warnings:?}");
    assert!(
        warnings[1].starts_with("luck: can't range-format file (parse error): "),
        "{warnings:?}"
    );
}

#[tokio::test]
async fn did_change_applies_multi_change_batch_with_shifted_ranges() {
    let notifier = CapturedNotifier::default();
    let server = Backend::new(notifier.clone());
    let uri = workspace_uri("batch.lua");
    open(&server, &uri, "local a = 1\nlocal b = 2\nlocal c = 3").await;

    // A single batch whose first edit inserts a line. The second edit's
    // range is expressed against the text the first one produced; applied
    // against the original lines it would rename `b` instead.
    server
        .did_change(DidChangeTextDocumentParams {
            text_document: VersionedTextDocumentIdentifier { uri: uri.clone(), version: 2 },
            content_changes: vec![
                TextDocumentContentChangeEvent {
                    range: Some(Range::new(Position::new(0, 0), Position::new(0, 11))),
                    range_length: None,
                    text: "local a = 10\nlocal aa = 11".to_owned(),
                },
                TextDocumentContentChangeEvent {
                    range: Some(Range::new(Position::new(3, 10), Position::new(3, 11))),
                    range_length: None,
                    text: "30".to_owned(),
                },
            ],
        })
        .await;

    let republished_version =
        notifier.published.read().await.last().map(|(_, _, version)| *version);
    assert_eq!(
        republished_version,
        Some(Some(2i32)),
        "the batch republishes diagnostics for the new version"
    );
    let response = server
        .document_symbol(DocumentSymbolParams {
            text_document: TextDocumentIdentifier { uri: uri.clone() },
            work_done_progress_params: WorkDoneProgressParams::default(),
            partial_result_params: PartialResultParams::default(),
        })
        .await
        .expect("document symbol errored")
        .expect("document remains open after applying the batch");
    let DocumentSymbolResponse::Nested(symbols) = response else {
        panic!("document symbols should be nested");
    };
    let names: Vec<&str> = symbols.iter().map(|symbol| symbol.name.as_str()).collect();
    assert_eq!(
        names,
        ["a", "aa", "b", "c"],
        "second edit must land on the line shifted by the first"
    );
    // `local c = 30` ends one column past `local c = 3`, so the second edit
    // was applied rather than dropped.
    assert_eq!(symbols[3].range.end, Position::new(3, 12), "{symbols:?}");
}

#[tokio::test]
async fn fix_all_workspace_collects_fixes_from_open_documents() {
    let directory = tempfile::tempdir().expect("tempdir");
    fs::write(
        directory.path().join("luck.json"),
        r#"{ "lint": { "disable_default_rules": true, "rule_overrides": { "redundant_nil_init": { "enabled": true } } } }"#,
    )
    .expect("write luck.json");
    let fixable_path = directory.path().join("fixable.lua");
    let clean_path = directory.path().join("clean.lua");
    fs::write(&fixable_path, "local x = nil\n").expect("write fixable.lua");
    fs::write(&clean_path, "return 1\n").expect("write clean.lua");
    let fixable = Url::from_file_path(&fixable_path).expect("file path URI");
    let clean = Url::from_file_path(&clean_path).expect("file path URI");

    let server = Backend::new(CapturedNotifier::default());
    open(&server, &fixable, "local x = nil\n").await;
    open(&server, &clean, "return 1\n").await;

    let edit = server
        .fix_all_workspace_request(serde_json::Value::Null)
        .await
        .expect("fix-all request errored");
    let changes = edit.changes.expect("changes map");
    assert_eq!(
        changes.keys().collect::<Vec<_>>(),
        [&fixable],
        "only the document with a fix gets edits: {changes:?}"
    );
    assert!(!changes[&fixable].is_empty(), "{changes:?}");
}
