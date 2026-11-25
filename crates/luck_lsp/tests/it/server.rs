use std::fs;
#[cfg(unix)]
use std::os::unix::fs::symlink as symlink_file;
#[cfg(windows)]
use std::os::windows::fs::symlink_file;
use std::path::Path;

use ls_types::notification::{Cancel, DidCloseTextDocument};
use ls_types::request::{
    CodeActionRequest, Completion, DocumentHighlightRequest, DocumentLinkRequest,
    DocumentSymbolRequest, FoldingRangeRequest, Formatting, GotoDefinition, HoverRequest,
    PrepareRenameRequest, RangeFormatting, RegisterCapability, Rename, Request as LspRequest,
    SelectionRangeRequest, SemanticTokensFullRequest, SemanticTokensRangeRequest, Shutdown,
    SignatureHelpRequest, WorkspaceSymbolRequest,
};
use ls_types::{
    CancelParams, ClientCapabilities, CodeActionContext, CodeActionKind, CodeActionOrCommand,
    CodeActionParams, CompletionParams, CompletionResponse,
    DidChangeWatchedFilesClientCapabilities, DidCloseTextDocumentParams, DocumentFormattingParams,
    DocumentHighlightKind, DocumentHighlightParams, DocumentLink, DocumentLinkParams,
    DocumentRangeFormattingParams, DocumentSymbolParams, DocumentSymbolResponse, FileChangeType,
    FoldingRangeParams, FormattingOptions, GeneralClientCapabilities, GotoDefinitionParams,
    GotoDefinitionResponse, HoverContents, HoverParams, MessageType, NumberOrString,
    PartialResultParams, Position, PositionEncodingKind, PrepareRenameResponse, Range,
    RenameParams, SelectionRangeParams, SemanticTokensParams, SemanticTokensRangeParams,
    SemanticTokensRangeResult, SemanticTokensResult, SignatureHelpParams,
    TextDocumentContentChangeEvent, TextEdit, Uri, WorkDoneProgressParams,
    WorkspaceClientCapabilities, WorkspaceEdit, WorkspaceSymbolParams, WorkspaceSymbolResponse,
};
use lsp_server::ErrorCode;
use luck_token::index_to_offset;
use serde_json::json;

use crate::common::{Client, document, position_params, references, workspace_uri};

/// `luck/syntaxTree`, which the extension sends.
enum SyntaxTree {}

impl LspRequest for SyntaxTree {
    type Params = serde_json::Value;
    type Result = String;
    const METHOD: &'static str = "luck/syntaxTree";
}

/// `luck/fixAllWorkspace`, which the extension sends.
enum FixAllWorkspace {}

impl LspRequest for FixAllWorkspace {
    type Params = serde_json::Value;
    type Result = WorkspaceEdit;
    const METHOD: &'static str = "luck/fixAllWorkspace";
}

fn rename_refusal(client: &mut Client, uri: &Uri, new_name: &str) -> Option<String> {
    client
        .request_result::<Rename>(RenameParams {
            text_document_position: position_params(uri, 0, 8),
            new_name: new_name.to_owned(),
            work_done_progress_params: WorkDoneProgressParams::default(),
        })
        .err()
        .map(|refusal| refusal.message)
}

fn project_with_excluded_directory(directory: &Path) -> (Uri, Uri) {
    fs::write(
        directory.join("luck.json"),
        r#"{ "exclude": ["gen/**"], "lint": { "categories": { "correctness": "off" }, "rules": { "redundant_nil_init": "warn" } } }"#,
    )
    .expect("write luck.json");
    let generated = directory.join("gen");
    fs::create_dir_all(&generated).expect("mkdir gen");
    let included_path = directory.join("kept.luau");
    let excluded_path = generated.join("a.luau");
    fs::write(&included_path, "local x = nil\n").expect("write kept.luau");
    fs::write(&excluded_path, "local x = nil\n").expect("write gen/a.luau");
    (
        Uri::from_file_path(&included_path).expect("file path URI"),
        Uri::from_file_path(&excluded_path).expect("file path URI"),
    )
}

fn fix_all_params(uri: &Uri) -> CodeActionParams {
    CodeActionParams {
        text_document: document(uri),
        range: Range::default(),
        context: CodeActionContext {
            diagnostics: Vec::new(),
            only: Some(vec![CodeActionKind::SOURCE_FIX_ALL]),
            trigger_kind: None,
        },
        work_done_progress_params: WorkDoneProgressParams::default(),
        partial_result_params: PartialResultParams::default(),
    }
}

fn formatting_params(uri: &Uri) -> DocumentFormattingParams {
    DocumentFormattingParams {
        text_document: document(uri),
        options: formatting_options(),
        work_done_progress_params: WorkDoneProgressParams::default(),
    }
}

fn formatting_options() -> FormattingOptions {
    FormattingOptions { tab_size: 4, insert_spaces: false, ..Default::default() }
}

fn document_symbols(client: &mut Client, uri: &Uri) -> Vec<String> {
    let response = client
        .request::<DocumentSymbolRequest>(DocumentSymbolParams {
            text_document: document(uri),
            work_done_progress_params: WorkDoneProgressParams::default(),
            partial_result_params: PartialResultParams::default(),
        })
        .expect("the document is open");
    let DocumentSymbolResponse::Nested(symbols) = response else {
        panic!("document symbols should be nested");
    };
    symbols.into_iter().map(|symbol| symbol.name).collect()
}

fn document_links(client: &mut Client, uri: &Uri) -> Vec<DocumentLink> {
    client
        .request::<DocumentLinkRequest>(DocumentLinkParams {
            text_document: document(uri),
            work_done_progress_params: WorkDoneProgressParams::default(),
            partial_result_params: PartialResultParams::default(),
        })
        .expect("the document is open")
}

#[test]
fn initialize_advertises_required_capabilities() {
    let (_client, result) = Client::start_with(ClientCapabilities::default());
    let capabilities = result.capabilities;
    assert!(capabilities.document_formatting_provider.is_some(), "formatting capability missing");
    assert!(
        capabilities.document_range_formatting_provider.is_some(),
        "range formatting capability missing"
    );
    assert!(capabilities.text_document_sync.is_some(), "text sync capability missing");
    assert_eq!(
        capabilities.position_encoding,
        Some(PositionEncodingKind::UTF16),
        "a client that names no encoding gets UTF-16"
    );
}

fn client_offering(encodings: &[PositionEncodingKind]) -> (Client, Option<PositionEncodingKind>) {
    let capabilities = ClientCapabilities {
        general: Some(GeneralClientCapabilities {
            position_encodings: Some(encodings.to_vec()),
            ..GeneralClientCapabilities::default()
        }),
        ..ClientCapabilities::default()
    };
    let (client, result) = Client::start_with(capabilities);
    (client, result.capabilities.position_encoding)
}

#[test]
fn a_client_that_offers_utf8_gets_byte_columns() {
    let (mut client, encoding) =
        client_offering(&[PositionEncodingKind::UTF16, PositionEncodingKind::UTF8]);
    assert_eq!(encoding, Some(PositionEncodingKind::UTF8));
    let uri = workspace_uri("utf8.luau");
    // The crab is 4 bytes, so `target` starts at byte 23, UTF-16 unit 21.
    client.open(&uri, "local s = \"\u{1F980}\" local target = 1\nprint(s, target)\n");
    let prepared = client
        .request::<PrepareRenameRequest>(position_params(&uri, 0, 25))
        .expect("target should be renameable");
    let PrepareRenameResponse::RangeWithPlaceholder { range, placeholder } = prepared else {
        panic!("expected range with placeholder");
    };
    assert_eq!(placeholder, "target");
    assert_eq!(range, Range::new(Position::new(0, 23), Position::new(0, 29)));
}

#[test]
fn a_client_that_offers_only_utf32_gets_character_columns() {
    let (mut client, encoding) = client_offering(&[PositionEncodingKind::UTF32]);
    assert_eq!(encoding, Some(PositionEncodingKind::UTF32));
    let uri = workspace_uri("utf32.luau");
    // The crab is one character, so `target` starts at character 20.
    client.open(&uri, "local s = \"\u{1F980}\" local target = 1\nprint(s, target)\n");
    let prepared = client
        .request::<PrepareRenameRequest>(position_params(&uri, 0, 22))
        .expect("target should be renameable");
    let PrepareRenameResponse::RangeWithPlaceholder { range, .. } = prepared else {
        panic!("expected range with placeholder");
    };
    assert_eq!(range, Range::new(Position::new(0, 20), Position::new(0, 26)));
}

#[test]
fn formatting_normalizes_spacing() {
    let mut client = Client::start();
    let uri = workspace_uri("ugly.luau");
    // No spaces around `=`, extra spaces, and trailing whitespace.
    let ugly = "local x=1   \nlocal     y=2";
    client.open(&uri, ugly);

    let edits =
        client.request::<Formatting>(formatting_params(&uri)).expect("formatter returned no edits");
    assert!(!edits.is_empty(), "ugly source should produce at least one edit");
    let new_text = &edits[0].new_text;
    assert!(new_text.contains("local x = 1"), "formatter normalizes spacing: {new_text}");
    assert!(new_text.contains("local y = 2"), "formatter normalizes spacing: {new_text}");
    assert_ne!(new_text.as_str(), ugly, "formatter must change ugly source");
}

/// The editor runs the rules `luck lint` runs on the same project.
#[test]
fn lint_runs_the_default_rules_without_config() {
    let mut client = Client::start();
    let uri = workspace_uri("unused.luau");
    client.open(&uri, "local unused = 1\n");

    let diags = client.diagnostics(&uri, Some(1));
    assert!(
        diags
            .iter()
            .any(|diag| diag.code == Some(NumberOrString::String("unused_variable".into()))),
        "the correctness rules run by default, as they do in `luck lint`: {diags:?}"
    );
}

#[test]
fn parse_errors_surface_without_config() {
    let mut client = Client::start();
    let uri = workspace_uri("broken.luau");
    client.open(&uri, "local x =\n");
    assert!(!client.diagnostics(&uri, Some(1)).is_empty(), "parse errors must surface");
}

/// A broken `luck.json` is shown and published on the file, and its files
/// lint under the defaults until it's fixed.
#[test]
fn a_broken_config_is_reported_on_itself_until_it_is_fixed() {
    let directory = tempfile::tempdir().expect("tempdir");
    let config_path = directory.path().join("luck.json");
    fs::write(&config_path, "{\n  \"variant\": \"bogus\"\n}\n").expect("write luck.json");
    let path = directory.path().join("main.luau");
    fs::write(&path, "local unused = 1\n").expect("write main.luau");
    let uri = Uri::from_file_path(&path).expect("file path URI");
    let config_uri = Uri::from_file_path(&config_path).expect("file path URI");

    let mut client = Client::start();
    client.open(&uri, "local unused = 1\n");

    let config_diags = client.diagnostics(&config_uri, None);
    let [diag] = config_diags.as_slice() else { panic!("one config error: {config_diags:?}") };
    assert_eq!(diag.code, Some(NumberOrString::String("E014".to_owned())));
    assert!(diag.message.contains("unknown variant `bogus`"), "{}", diag.message);
    assert_eq!(diag.range, Range::new(Position::new(1, 13), Position::new(1, 20)));
    let errors = client.shown(MessageType::ERROR);
    assert!(errors.iter().any(|text| text.contains("bogus")), "{errors:?}");
    assert!(!client.diagnostics(&uri, Some(1)).is_empty(), "the defaults lint the file");

    fs::write(&config_path, "{}").expect("fix luck.json");
    client.file_changed(&config_path, FileChangeType::CHANGED);
    assert!(client.diagnostics(&config_uri, None).is_empty(), "the fix clears the error");
}

/// A symlink reads under the project of its own directory, as `luck lint`
/// reads the path it's given, and not under its target's.
#[test]
fn a_symlink_takes_the_project_of_its_own_directory() {
    let directory = tempfile::tempdir().expect("tempdir");
    fs::write(directory.path().join("luck.json"), "{}").expect("write luck.json");
    let elsewhere = directory.path().join("elsewhere");
    fs::create_dir_all(&elsewhere).expect("mkdir elsewhere");
    fs::write(
        elsewhere.join("luck.json"),
        r#"{ "lint": { "rules": { "unused_variable": "off" } } }"#,
    )
    .expect("write elsewhere/luck.json");
    let text = "local unused = 1\n";
    let target = elsewhere.join("b.luau");
    fs::write(&target, text).expect("write b.luau");
    let link = directory.path().join("a.luau");
    // Windows lets only an administrator or developer mode make one.
    if symlink_file(&target, &link).is_err() {
        return;
    }
    let uri = Uri::from_file_path(&link).expect("file path URI");

    let mut client = Client::start();
    client.open(&uri, text);
    let diags = client.diagnostics(&uri, Some(1));
    let codes: Vec<_> = diags.iter().filter_map(|diag| diag.code.as_ref()).collect();
    assert_eq!(
        codes,
        [&NumberOrString::String("unused_variable".to_owned())],
        "linted under the link's project: {diags:?}"
    );
}

#[test]
fn an_override_layers_its_lint_settings_over_the_files_it_matches() {
    let directory = tempfile::tempdir().expect("tempdir");
    fs::write(
        directory.path().join("luck.json"),
        r#"{ "overrides": [{ "files": ["spec/**"], "lint": { "rules": { "unused_variable": "off" } } }] }"#,
    )
    .expect("write luck.json");
    fs::create_dir_all(directory.path().join("spec")).expect("mkdir spec");
    let (source, spec) = (directory.path().join("a.luau"), directory.path().join("spec/a.luau"));
    for path in [&source, &spec] {
        fs::write(path, "local unused = 1\n").expect("write source");
    }
    let (source_uri, spec_uri) = (
        Uri::from_file_path(&source).expect("file path URI"),
        Uri::from_file_path(&spec).expect("file path URI"),
    );

    let mut client = Client::start();
    client.open(&source_uri, "local unused = 1\n");
    client.open(&spec_uri, "local unused = 1\n");
    let source_diags = client.diagnostics(&source_uri, Some(1));
    assert!(!source_diags.is_empty(), "the rule runs outside the override: {source_diags:?}");
    let spec_diags = client.diagnostics(&spec_uri, Some(1));
    assert!(spec_diags.is_empty(), "the override turns the rule off: {spec_diags:?}");
}

/// A file a `luck.json` extends is as much its settings as the file itself,
/// but a JSON file no project reads, such as Rojo's sourcemap, isn't.
#[test]
fn a_change_to_an_extended_config_reloads_and_another_json_file_does_not() {
    let directory = tempfile::tempdir().expect("tempdir");
    let base = directory.path().join("base.json");
    fs::write(&base, "{}").expect("write base.json");
    fs::write(directory.path().join("luck.json"), r#"{ "extends": ["base.json"] }"#)
        .expect("write luck.json");
    let path = directory.path().join("main.luau");
    fs::write(&path, "local count = 1\n").expect("write main.luau");
    let uri = Uri::from_file_path(&path).expect("file path URI");

    let mut client = Client::start();
    client.open(&uri, "local count = 1\n");
    assert_eq!(rename_refusal(&mut client, &uri, "app"), None, "`app` isn't configured yet");

    fs::write(&base, r#"{ "lint": { "extra_globals": ["app"] } }"#).expect("rewrite base.json");
    let sourcemap = directory.path().join("sourcemap.json");
    fs::write(&sourcemap, "{}").expect("write sourcemap.json");
    client.file_changed(&sourcemap, FileChangeType::CHANGED);
    assert_eq!(rename_refusal(&mut client, &uri, "app"), None, "the sourcemap isn't settings");

    client.file_changed(&base, FileChangeType::CHANGED);
    assert!(rename_refusal(&mut client, &uri, "app").is_some(), "the base's change took effect");
}

#[test]
fn range_formatting_preserves_out_of_range_content() {
    // The server sends one whole-document edit, so this checks that text
    // outside the range survives, not that the edit is narrowed.
    let mut client = Client::start();
    let uri = workspace_uri("partial.luau");
    let source = "\
local first=1
local middle=2
local last=3
";
    client.open(&uri, source);

    // Only the middle line is in range; the first and third must come back
    // byte-identical.
    let edits = client
        .request::<RangeFormatting>(DocumentRangeFormattingParams {
            text_document: document(&uri),
            range: Range::new(Position::new(1, 0), Position::new(1, 15)),
            options: formatting_options(),
            work_done_progress_params: WorkDoneProgressParams::default(),
        })
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

#[test]
fn formatting_unknown_document_returns_none() {
    let mut client = Client::start();
    let result = client.request::<Formatting>(formatting_params(&workspace_uri("missing.luau")));
    assert!(result.is_none(), "formatting an unopened document should return None");
}

#[test]
fn shutdown_then_exit_ends_the_server_cleanly() {
    let client = Client::start();
    client.shut_down().expect("shutdown then exit is a clean end");
}

#[test]
fn exit_without_shutdown_ends_the_server_with_an_error() {
    let client = Client::start();
    let ended = client.exit();
    assert!(ended.is_err(), "LSP has a server exit with an error code then: {ended:?}");
}

/// The server never waits on the client, so a request the client leaves
/// unanswered holds up nothing.
#[test]
fn exit_ends_the_server_while_its_request_to_the_client_is_unanswered() {
    let capabilities = ClientCapabilities {
        workspace: Some(WorkspaceClientCapabilities {
            did_change_watched_files: Some(DidChangeWatchedFilesClientCapabilities {
                dynamic_registration: Some(true),
                relative_pattern_support: None,
            }),
            ..WorkspaceClientCapabilities::default()
        }),
        ..ClientCapabilities::default()
    };
    let (mut client, _) = Client::start_with(capabilities);
    let uri = workspace_uri("unanswered.luau");
    client.open(&uri, "local unused = 1\n");
    assert!(!client.diagnostics(&uri, Some(1)).is_empty(), "the server works meanwhile");
    assert!(
        client.server_requests().iter().any(|request| request.method == RegisterCapability::METHOD),
        "a client that can watch files is asked to: {:?}",
        client.server_requests()
    );
    client.shut_down().expect("the server ends while the client owes it an answer");
}

#[test]
fn requests_after_shutdown_are_refused() {
    let mut client = Client::start();
    client.request::<Shutdown>(());
    let refused = client.request_result::<Formatting>(formatting_params(&workspace_uri("a.luau")));
    assert!(refused.is_err(), "{refused:?}");
    client.exit().expect("exit after shutdown is a clean end");
}

#[test]
fn a_cancelled_request_gets_exactly_one_answer() {
    let mut client = Client::start();
    let uri = workspace_uri("cancelled.luau");
    client.open(&uri, "local value = 1\n");
    let id = client.send::<SemanticTokensFullRequest>(SemanticTokensParams {
        work_done_progress_params: WorkDoneProgressParams::default(),
        partial_result_params: PartialResultParams::default(),
        text_document: document(&uri),
    });
    let as_json = serde_json::to_value(&id).expect("an id serializes");
    let cancelled = serde_json::from_value(as_json).expect("an id is a number or a string");
    client.notify::<Cancel>(CancelParams { id: cancelled });
    match client.answer::<SemanticTokensFullRequest>(&id) {
        Ok(tokens) => assert!(tokens.is_some(), "an answer that beat the cancel is whole"),
        Err(error) => assert_eq!(error.code, -32800, "RequestCancelled: {error:?}"),
    }
    // A later request's answer comes after any second answer to the first.
    assert_eq!(document_symbols(&mut client, &uri), ["value"]);
    assert!(client.stray_responses().is_empty(), "{:?}", client.stray_responses());
}

#[test]
fn hover_renders_stdlib_signature() {
    let mut client = Client::start();
    let uri = workspace_uri("hover.luau");
    // Cursor on `format` in `string.format`.
    client.open(&uri, "local formatted = string.format(\"%d\", 1)\n");

    let hover = client
        .request::<HoverRequest>(HoverParams {
            text_document_position_params: position_params(&uri, 0, 27),
            work_done_progress_params: WorkDoneProgressParams::default(),
        })
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

#[test]
fn completion_lists_locals_and_stdlib_globals() {
    let mut client = Client::start();
    let uri = workspace_uri("completion.luau");
    // A visible local plus a bare-identifier completion site inside a call.
    client.open(
        &uri,
        "\
local myLocal = 1
print(m)
",
    );

    let response = client
        .request::<Completion>(CompletionParams {
            text_document_position: position_params(&uri, 1, 7),
            work_done_progress_params: WorkDoneProgressParams::default(),
            partial_result_params: PartialResultParams::default(),
            context: None,
        })
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

#[test]
fn signature_help_marks_active_parameter() {
    let mut client = Client::start();
    let uri = workspace_uri("signature.luau");
    // Cursor lands on the second argument `5`, so parameter index 1 is active.
    client.open(&uri, "local x = string.format(\"a\", 5)\n");

    let help = client
        .request::<SignatureHelpRequest>(SignatureHelpParams {
            context: None,
            text_document_position_params: position_params(&uri, 0, 29),
            work_done_progress_params: WorkDoneProgressParams::default(),
        })
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

#[test]
fn document_symbol_reports_named_function() {
    let mut client = Client::start();
    let uri = workspace_uri("symbols.luau");
    client.open(
        &uri,
        "\
local function greet(name)
    return name
end
",
    );
    let names = document_symbols(&mut client, &uri);
    assert!(names.iter().any(|name| name == "greet"), "outline should contain `greet`: {names:?}");
}

#[test]
fn folding_range_covers_multiline_function() {
    let mut client = Client::start();
    let uri = workspace_uri("folding.luau");
    client.open(
        &uri,
        "\
local function greet(name)
    return name
end
",
    );

    let ranges = client
        .request::<FoldingRangeRequest>(FoldingRangeParams {
            text_document: document(&uri),
            work_done_progress_params: WorkDoneProgressParams::default(),
            partial_result_params: PartialResultParams::default(),
        })
        .expect("multi-line function should return folding ranges");
    assert!(
        ranges.iter().any(|range| range.start_line == 0 && range.end_line >= 2),
        "the three-line function body should produce a fold: {ranges:?}"
    );
}

#[test]
fn document_highlight_finds_all_occurrences() {
    let mut client = Client::start();
    let uri = workspace_uri("highlight.luau");
    client.open(
        &uri,
        "\
local count = 1
count = count + 1
",
    );

    // Cursor on the write target `count` on line 1.
    let highlights = client
        .request::<DocumentHighlightRequest>(DocumentHighlightParams {
            text_document_position_params: position_params(&uri, 1, 2),
            work_done_progress_params: WorkDoneProgressParams::default(),
            partial_result_params: PartialResultParams::default(),
        })
        .expect("name under cursor should highlight");
    assert!(highlights.len() >= 2, "`count` occurs multiple times: {highlights:?}");
    assert!(
        highlights.iter().any(|highlight| highlight.kind == Some(DocumentHighlightKind::WRITE)),
        "a reassignment of `count` should be a WRITE highlight"
    );
}

#[test]
fn selection_range_expands_from_identifier() {
    let mut client = Client::start();
    let uri = workspace_uri("selection.luau");
    client.open(&uri, "local value = alpha.beta + 1\n");

    // Cursor on `beta` inside the `alpha.beta` field access.
    let ranges = client
        .request::<SelectionRangeRequest>(SelectionRangeParams {
            work_done_progress_params: WorkDoneProgressParams::default(),
            partial_result_params: PartialResultParams::default(),
            text_document: document(&uri),
            positions: vec![Position { line: 0, character: 20 }],
        })
        .expect("selection range should return a chain");
    // Each link's `parent` contains it, so the chain runs from the narrowest
    // span out to the widest.
    let narrowest = &ranges[0];
    let mut widest = narrowest;
    let mut depth = 1;
    while let Some(parent) = widest.parent.as_ref() {
        widest = parent;
        depth += 1;
    }
    assert!(
        depth >= 2,
        "selection should expand through at least one enclosing node: {narrowest:?}"
    );
    let narrowest_width = narrowest.range.end.character - narrowest.range.start.character;
    let widest_width = widest.range.end.character - widest.range.start.character;
    assert!(
        narrowest_width < widest_width,
        "the narrowest selection must cover less than the whole statement: {narrowest:?}"
    );
}

fn semantic_tokens(client: &mut Client, uri: &Uri) -> Vec<ls_types::SemanticToken> {
    let result = client
        .request::<SemanticTokensFullRequest>(SemanticTokensParams {
            work_done_progress_params: WorkDoneProgressParams::default(),
            partial_result_params: PartialResultParams::default(),
            text_document: document(uri),
        })
        .expect("a document with identifiers should tokenize");
    let SemanticTokensResult::Tokens(tokens) = result else {
        panic!("expected a full semantic token set");
    };
    tokens.data
}

#[test]
fn semantic_tokens_cover_every_identifier() {
    let mut client = Client::start();
    let uri = workspace_uri("tokens.luau");
    client.open(
        &uri,
        "\
print(math.pi)
local mine = my_helper(1)
",
    );
    // print, math, pi, mine, my_helper: every identifier gets a token,
    // including stdlib members after a dot and unresolved globals.
    let tokens = semantic_tokens(&mut client, &uri);
    assert_eq!(tokens.len(), 5, "every identifier should tokenize: {tokens:?}");
}

#[test]
fn semantic_tokens_count_the_session_s_units() {
    let (mut client, _) = client_offering(&[PositionEncodingKind::UTF8]);
    let uri = workspace_uri("wide_string.luau");
    client.open(&uri, "local s = \"\u{1F980}\" local target = 1\n");
    let tokens = semantic_tokens(&mut client, &uri);
    let starts: Vec<(u32, u32)> =
        tokens.iter().map(|token| (token.delta_start, token.length)).collect();
    // `s` at byte 6, then `target` 17 bytes on, past the crab's 4.
    assert_eq!(starts, [(6, 1), (17, 6)], "{tokens:?}");
}

#[test]
fn document_link_resolves_require_path() {
    let directory = tempfile::tempdir().expect("tempdir");
    // Pins the project, so no `luck.json` above the temp root can pick its
    // settings.
    fs::write(directory.path().join("luck.json"), "{}").expect("write luck.json");
    // A neighboring module the `require` should resolve to.
    fs::write(directory.path().join("foo.luau"), "return {}\n").expect("write foo.luau");
    let main_path = directory.path().join("main.luau");
    fs::write(&main_path, "local foo = require(\"./foo\")\n").expect("write main.luau");
    let uri = Uri::from_file_path(&main_path).expect("file path URI");

    let mut client = Client::start();
    client.open(&uri, "local foo = require(\"./foo\")\n");
    let links = document_links(&mut client, &uri);
    assert_eq!(links.len(), 1, "exactly one require link expected");
    assert_eq!(
        links[0].tooltip.as_deref(),
        Some("./foo"),
        "link tooltip should carry the module name"
    );
    let target = links[0].target.as_ref().expect("link should have a target");
    assert!(
        target.path().as_str().ends_with("foo.luau"),
        "link should resolve to foo.luau: {}",
        target.as_str()
    );
}

/// The project keeps its resolver, and the client's watcher tells it what
/// changed on disk.
#[test]
fn a_module_created_on_disk_links_once_the_watcher_reports_it() {
    let directory = tempfile::tempdir().expect("tempdir");
    fs::write(directory.path().join("luck.json"), "{}").expect("write luck.json");
    let main_path = directory.path().join("main.luau");
    let text = "local late = require(\"./late\")\n";
    fs::write(&main_path, text).expect("write main.luau");
    let uri = Uri::from_file_path(&main_path).expect("file path URI");

    let mut client = Client::start();
    client.open(&uri, text);
    assert!(document_links(&mut client, &uri).is_empty(), "`late.luau` doesn't exist yet");

    let late = directory.path().join("late.luau");
    fs::write(&late, "return {}\n").expect("write late.luau");
    assert!(document_links(&mut client, &uri).is_empty(), "the resolver answers from its cache");
    client.file_changed(&late, FileChangeType::CREATED);
    let links = document_links(&mut client, &uri);
    assert_eq!(links.len(), 1, "the created module links: {links:?}");
}

#[test]
fn a_luaurc_change_reaches_the_project_s_resolver() {
    let directory = tempfile::tempdir().expect("tempdir");
    fs::write(directory.path().join("luck.json"), "{}").expect("write luck.json");
    let shared = directory.path().join("shared");
    fs::create_dir_all(&shared).expect("mkdir shared");
    fs::write(shared.join("common.luau"), "return {}\n").expect("write common.luau");
    let luaurc = directory.path().join(".luaurc");
    fs::write(&luaurc, "{}").expect("write .luaurc");
    let main_path = directory.path().join("main.luau");
    let source = "local c = require(\"@shared/common\")\n";
    fs::write(&main_path, source).expect("write main.luau");
    let uri = Uri::from_file_path(&main_path).expect("file path URI");

    let mut client = Client::start();
    client.open(&uri, source);
    assert!(document_links(&mut client, &uri).is_empty(), "no alias `shared` yet");

    fs::write(&luaurc, json!({ "aliases": { "shared": "./shared" } }).to_string())
        .expect("rewrite .luaurc");
    client.file_changed(&luaurc, FileChangeType::CHANGED);
    assert_eq!(document_links(&mut client, &uri).len(), 1, "the alias resolves");
}

#[test]
fn document_link_resolves_a_relative_require() {
    let directory = tempfile::tempdir().expect("tempdir");
    // Pins the project, so no `luck.json` above the temp root can pick its
    // settings.
    fs::write(directory.path().join("luck.json"), "{}").expect("write luck.json");
    fs::write(directory.path().join("sibling.luau"), "return {}\n").expect("write sibling.luau");
    let main_path = directory.path().join("main.luau");
    let source = "local s = require(\"./sibling\")\n";
    fs::write(&main_path, source).expect("write main.luau");
    let uri = Uri::from_file_path(&main_path).expect("file path URI");

    let mut client = Client::start();
    client.open(&uri, source);
    let links = document_links(&mut client, &uri);
    assert_eq!(links.len(), 1, "exactly one require link expected");
    let target = links[0].target.as_ref().expect("link should have a target");
    assert!(
        target.path().as_str().ends_with("sibling.luau"),
        "link should resolve to sibling.luau: {}",
        target.as_str()
    );
}

#[test]
fn document_link_resolves_an_alias_require() {
    // `@alias` requires resolve through the `.luaurc` alias table.
    let directory = tempfile::tempdir().expect("tempdir");
    // Pins the project, so no `luck.json` above the temp root can pick its
    // settings.
    fs::write(directory.path().join("luck.json"), "{}").expect("write luck.json");
    let shared = directory.path().join("shared");
    fs::create_dir_all(&shared).expect("mkdir shared");
    fs::write(shared.join("common.luau"), "return {}\n").expect("write common.luau");
    fs::write(
        directory.path().join(".luaurc"),
        json!({ "aliases": { "shared": "./shared" } }).to_string(),
    )
    .expect("write .luaurc");
    let main_path = directory.path().join("main.luau");
    let source = "local c = require(\"@shared/common\")\n";
    fs::write(&main_path, source).expect("write main.luau");
    let uri = Uri::from_file_path(&main_path).expect("file path URI");

    let mut client = Client::start();
    client.open(&uri, source);
    let links = document_links(&mut client, &uri);
    assert_eq!(links.len(), 1, "exactly one require link expected");
    let target = links[0].target.as_ref().expect("link should have a target");
    assert!(
        target.path().as_str().ends_with("shared/common.luau"),
        "alias link should resolve to shared/common.luau: {}",
        target.as_str()
    );
}

fn action_titles(actions: &[CodeActionOrCommand]) -> Vec<&str> {
    actions
        .iter()
        .map(|action| match action {
            CodeActionOrCommand::CodeAction(code_action) => code_action.title.as_str(),
            CodeActionOrCommand::Command(command) => command.title.as_str(),
        })
        .collect()
}

#[test]
fn code_action_offers_lint_fix() {
    let directory = tempfile::tempdir().expect("tempdir");
    // Enable a fixable style rule.
    fs::write(
        directory.path().join("luck.json"),
        r#"{ "lint": { "rules": { "redundant_nil_init": "warn" } } }"#,
    )
    .expect("write luck.json");
    let file_path = directory.path().join("nilinit.luau");
    fs::write(&file_path, "local x = nil\n").expect("write luau file");
    let uri = Uri::from_file_path(&file_path).expect("file path URI");

    let mut client = Client::start();
    client.open(&uri, "local x = nil\n");
    // The enabled rule should have surfaced a diagnostic on open.
    let diags = client.diagnostics(&uri, Some(1));
    assert!(!diags.is_empty(), "the enabled lint rule should publish a diagnostic");

    let actions = client
        .request::<CodeActionRequest>(CodeActionParams {
            text_document: document(&uri),
            range: Range::new(Position::new(0, 0), Position::new(0, 13)),
            context: CodeActionContext { diagnostics: diags, only: None, trigger_kind: None },
            work_done_progress_params: WorkDoneProgressParams::default(),
            partial_result_params: PartialResultParams::default(),
        })
        .expect("a fixable diagnostic should yield code actions");
    let titles = action_titles(&actions);
    assert!(
        titles.iter().any(|title| title.contains("drop redundant `= nil` initializer")),
        "the redundant-nil fix should be offered: {titles:?}"
    );
}

#[test]
fn syntax_tree_request_dumps_ast() {
    let mut client = Client::start();
    let uri = workspace_uri("tree.luau");
    client.open(&uri, "local x = 1\n");
    let dump = client.request::<SyntaxTree>(json!({ "textDocument": { "uri": uri.as_str() } }));
    assert!(
        dump.contains("LocalAssignment"),
        "the AST dump should contain the parsed statement node: {dump}"
    );
    let unopened = workspace_uri("unopened.luau");
    let unopened_dump =
        client.request::<SyntaxTree>(json!({ "textDocument": { "uri": unopened.as_str() } }));
    assert_eq!(unopened_dump, "", "a document that isn't open has no tree");
    for params in [json!({}), json!({ "textDocument": { "uri": "not a uri" } })] {
        let refused = client.request_result::<SyntaxTree>(params);
        let refusal = refused.expect_err("a request naming no document is refused");
        assert_eq!(refusal.code, ErrorCode::InvalidParams as i32, "{refusal:?}");
    }
}

#[test]
fn goto_definition_jumps_to_declaration() {
    let mut client = Client::start();
    let uri = workspace_uri("definition.luau");
    client.open(
        &uri,
        "\
local value = 1
print(value)
",
    );

    let response = client
        .request::<GotoDefinition>(GotoDefinitionParams {
            text_document_position_params: position_params(&uri, 1, 8),
            work_done_progress_params: WorkDoneProgressParams::default(),
            partial_result_params: PartialResultParams::default(),
        })
        .expect("local should have a definition");
    let GotoDefinitionResponse::Scalar(location) = response else {
        panic!("expected scalar location");
    };
    assert_eq!(location.uri, uri);
    assert_eq!(location.range.start, Position::new(0, 6));
}

#[test]
fn goto_definition_returns_none_for_a_global() {
    let mut client = Client::start();
    let uri = workspace_uri("definition_global.luau");
    client.open(&uri, "print(1)\n");

    let response = client.request::<GotoDefinition>(GotoDefinitionParams {
        text_document_position_params: position_params(&uri, 0, 2),
        work_done_progress_params: WorkDoneProgressParams::default(),
        partial_result_params: PartialResultParams::default(),
    });
    assert!(response.is_none(), "stdlib globals have no definition site");
}

#[test]
fn references_are_scope_exact_for_shadowed_names() {
    let mut client = Client::start();
    let uri = workspace_uri("references.luau");
    client.open(
        &uri,
        "\
local x = 1
do
    local x = 2
    print(x)
end
print(x)
",
    );

    // Cursor on the INNER x read at line 3.
    let locations = references(&mut client, &uri, Position::new(3, 10), true);
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

#[test]
fn references_exclude_declaration_when_not_requested() {
    let mut client = Client::start();
    let uri = workspace_uri("references_no_decl.luau");
    client.open(
        &uri,
        "\
local count = 1
count = count + 1
",
    );

    let locations = references(&mut client, &uri, Position::new(0, 8), false);
    assert!(
        locations.iter().all(|location| location.range.start.line == 1),
        "declaration should be excluded: {locations:?}"
    );
}

#[test]
fn prepare_rename_accepts_local_and_rejects_global() {
    let mut client = Client::start();
    let uri = workspace_uri("prepare_rename.luau");
    client.open(
        &uri,
        "\
local count = 1
print(count)
",
    );

    let prepared = client
        .request::<PrepareRenameRequest>(position_params(&uri, 0, 8))
        .expect("local should be renameable");
    let PrepareRenameResponse::RangeWithPlaceholder { placeholder, .. } = prepared else {
        panic!("expected range with placeholder");
    };
    assert_eq!(placeholder, "count");

    let on_global = client.request::<PrepareRenameRequest>(position_params(&uri, 1, 2));
    assert!(on_global.is_none(), "globals must not be renameable");
}

#[test]
fn rename_updates_declaration_and_references() {
    let mut client = Client::start();
    let uri = workspace_uri("rename.luau");
    client.open(
        &uri,
        "\
local count = 1
count = count + 1
",
    );

    let edit = client
        .request::<Rename>(RenameParams {
            text_document_position: position_params(&uri, 0, 8),
            new_name: "total".to_owned(),
            work_done_progress_params: WorkDoneProgressParams::default(),
        })
        .expect("rename should produce an edit");
    let edits: Vec<TextEdit> =
        edit.changes.expect("changes map").remove(&uri).expect("edits for the document");
    assert_eq!(edits.len(), 3, "declaration + two references: {edits:?}");
    assert!(edits.iter().all(|text_edit| text_edit.new_text == "total"));
}

#[test]
fn rename_rejects_keywords_used_names_and_builtins() {
    let mut client = Client::start();
    let uri = workspace_uri("rename_reject.luau");
    client.open(
        &uri,
        "\
local count = 1
local other = count
",
    );
    for (new_name, why) in [
        ("end", "keywords are not identifiers"),
        ("other", "in-use names must be rejected"),
        ("print", "stdlib globals must be rejected"),
    ] {
        assert!(rename_refusal(&mut client, &uri, new_name).is_some(), "{why}");
    }
}

#[test]
fn prepare_rename_handles_utf16_offsets() {
    let mut client = Client::start();
    let uri = workspace_uri("rename_utf16.luau");
    // The crab emoji is two UTF-16 units; positions after it require
    // real UTF-16 conversion, not byte arithmetic.
    client.open(
        &uri,
        "\
local s = \"\u{1F980}\" local target = 1
print(s, target)
",
    );

    let prepared = client
        .request::<PrepareRenameRequest>(position_params(&uri, 0, 23))
        .expect("target should be renameable");
    let PrepareRenameResponse::RangeWithPlaceholder { placeholder, .. } = prepared else {
        panic!("expected range with placeholder");
    };
    assert_eq!(placeholder, "target");
}

fn workspace_symbols(client: &mut Client, query: &str) -> Vec<ls_types::SymbolInformation> {
    let response = client
        .request::<WorkspaceSymbolRequest>(WorkspaceSymbolParams {
            query: query.to_owned(),
            work_done_progress_params: WorkDoneProgressParams::default(),
            partial_result_params: PartialResultParams::default(),
        })
        .expect("symbols expected");
    let WorkspaceSymbolResponse::Flat(symbols) = response else {
        panic!("workspace symbols should be flat");
    };
    symbols
}

#[test]
fn workspace_symbols_filter_across_open_documents() {
    let mut client = Client::start();
    let first = workspace_uri("ws_first.luau");
    let second = workspace_uri("ws_second.luau");
    client.open(
        &first,
        "\
local function setup_camera() end
setup_camera()
",
    );
    client.open(
        &second,
        "\
local function teardown() end
teardown()
",
    );

    let symbols = workspace_symbols(&mut client, "camera");
    assert_eq!(symbols.len(), 1, "{symbols:?}");
    assert_eq!(symbols[0].name, "setup_camera");
    assert_eq!(symbols[0].location.uri, first);

    let all = workspace_symbols(&mut client, "");
    assert!(all.len() >= 2, "empty query returns everything: {all:?}");
}

#[test]
fn semantic_tokens_range_returns_subset() {
    let mut client = Client::start();
    let uri = workspace_uri("tokens_range.luau");
    client.open(
        &uri,
        "\
print(1)
print(2)
print(3)
",
    );

    let full = semantic_tokens(&mut client, &uri);
    let ranged_result = client
        .request::<SemanticTokensRangeRequest>(SemanticTokensRangeParams {
            text_document: document(&uri),
            range: Range::new(Position::new(1, 0), Position::new(1, 11)),
            work_done_progress_params: WorkDoneProgressParams::default(),
            partial_result_params: PartialResultParams::default(),
        })
        .expect("tokens expected");
    let SemanticTokensRangeResult::Tokens(ranged) = ranged_result else {
        panic!("expected ranged tokens");
    };
    assert!(!ranged.data.is_empty(), "line 1 has tokens");
    assert!(
        ranged.data.len() < full.len(),
        "range must be a strict subset: {} vs {}",
        ranged.data.len(),
        full.len()
    );
    // Delta re-encoding: the first ranged token is absolute (line 1).
    assert_eq!(ranged.data[0].delta_line, 1);
}

#[test]
fn completion_offers_collectgarbage_options_inside_the_string() {
    let mut client = Client::start();
    let uri = workspace_uri("gc.luau");
    let text = "collectgarbage('c')";
    client.open(&uri, text);

    let utf16_column = index_to_offset(text.find("('").expect("literal") + 2);
    let response = client
        .request::<Completion>(CompletionParams {
            text_document_position: position_params(&uri, 0, utf16_column),
            work_done_progress_params: WorkDoneProgressParams::default(),
            partial_result_params: PartialResultParams::default(),
            context: None,
        })
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

#[test]
fn formatting_unparsable_file_warns_and_returns_none() {
    let mut client = Client::start();
    let uri = workspace_uri("unparsable.luau");
    client.open(&uri, "local x =\n");

    let formatted = client.request::<Formatting>(formatting_params(&uri));
    assert!(formatted.is_none(), "{formatted:?}");
    let range_formatted = client.request::<RangeFormatting>(DocumentRangeFormattingParams {
        text_document: document(&uri),
        range: Range::new(Position::new(0, 0), Position::new(0, 9)),
        options: formatting_options(),
        work_done_progress_params: WorkDoneProgressParams::default(),
    });
    assert!(range_formatted.is_none(), "{range_formatted:?}");

    let warnings = client.shown(MessageType::WARNING);
    assert_eq!(warnings.len(), 2, "{warnings:?}");
    assert!(warnings[0].starts_with("luck: can't format file (parse error): "), "{warnings:?}");
    assert!(
        warnings[1].starts_with("luck: can't range-format file (parse error): "),
        "{warnings:?}"
    );
}

#[test]
fn did_change_applies_multi_change_batch_with_shifted_ranges() {
    let mut client = Client::start();
    let uri = workspace_uri("batch.luau");
    client.open(
        &uri,
        "\
local a = 1
local b = 2
local c = 3",
    );

    // A single batch whose first edit inserts a line. The second edit's
    // range is expressed against the text the first one produced; applied
    // against the original lines it would rename `b` instead.
    client.change(
        &uri,
        2,
        vec![
            TextDocumentContentChangeEvent {
                range: Some(Range::new(Position::new(0, 0), Position::new(0, 11))),
                range_length: None,
                text: "\
local a = 10
local aa = 11"
                    .to_owned(),
            },
            TextDocumentContentChangeEvent {
                range: Some(Range::new(Position::new(3, 10), Position::new(3, 11))),
                range_length: None,
                text: "30".to_owned(),
            },
        ],
    );
    let _republished = client.diagnostics(&uri, Some(2));
    let response = client
        .request::<DocumentSymbolRequest>(DocumentSymbolParams {
            text_document: document(&uri),
            work_done_progress_params: WorkDoneProgressParams::default(),
            partial_result_params: PartialResultParams::default(),
        })
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

#[test]
fn a_change_whose_range_does_not_fit_drops_the_document() {
    let mut client = Client::start();
    let uri = workspace_uri("desync.luau");
    client.open(&uri, "local a = 1\n");
    let _opened = client.diagnostics(&uri, Some(1));
    client.change(
        &uri,
        2,
        vec![TextDocumentContentChangeEvent {
            range: Some(Range::new(Position::new(0, 5), Position::new(0, 2))),
            range_length: None,
            text: "x".to_owned(),
        }],
    );
    assert!(client.diagnostics(&uri, None).is_empty(), "nothing stays shown for corrupt text");
    let formatted = client.request::<Formatting>(formatting_params(&uri));
    assert!(formatted.is_none(), "the document isn't served until it's reopened");
}

#[test]
fn fix_all_workspace_collects_fixes_from_open_documents() {
    let directory = tempfile::tempdir().expect("tempdir");
    fs::write(
        directory.path().join("luck.json"),
        r#"{ "lint": { "categories": { "correctness": "off" }, "rules": { "redundant_nil_init": "warn" } } }"#,
    )
    .expect("write luck.json");
    let fixable_path = directory.path().join("fixable.luau");
    let clean_path = directory.path().join("clean.luau");
    fs::write(&fixable_path, "local x = nil\n").expect("write fixable.luau");
    fs::write(&clean_path, "return 1\n").expect("write clean.luau");
    let fixable = Uri::from_file_path(&fixable_path).expect("file path URI");
    let clean = Uri::from_file_path(&clean_path).expect("file path URI");

    let mut client = Client::start();
    client.open(&fixable, "local x = nil\n");
    client.open(&clean, "return 1\n");

    let changes =
        client.request::<FixAllWorkspace>(serde_json::Value::Null).changes.expect("changes map");
    assert_eq!(
        changes.keys().collect::<Vec<_>>(),
        [&fixable],
        "only the document with a fix gets edits: {changes:?}"
    );
    assert!(!changes[&fixable].is_empty(), "{changes:?}");
}

#[test]
fn did_change_with_no_changes_stores_the_new_version() {
    let mut client = Client::start();
    let uri = workspace_uri("empty_change.luau");
    client.open(&uri, "local a = 1\n");
    let _opened = client.diagnostics(&uri, Some(1));

    client.change(&uri, 5, Vec::new());
    let _republished = client.diagnostics(&uri, Some(5));

    // A repeated version is stale, so it changes and republishes nothing.
    client.replace_all(&uri, 5, "local b = 2\n");
    assert_eq!(document_symbols(&mut client, &uri), ["a"]);
    let published = client
        .notifications()
        .iter()
        .filter(|notification| notification.method == "textDocument/publishDiagnostics")
        .count();
    assert_eq!(published, 0, "{:?}", client.notifications());
}

#[test]
fn fix_all_workspace_skips_excluded_documents() {
    let directory = tempfile::tempdir().expect("tempdir");
    let (included, excluded) = project_with_excluded_directory(directory.path());

    let mut client = Client::start();
    client.open(&included, "local x = nil\n");
    client.open(&excluded, "local x = nil\n");
    assert!(
        client.diagnostics(&excluded, Some(1)).is_empty(),
        "an excluded document publishes nothing"
    );

    let changes =
        client.request::<FixAllWorkspace>(serde_json::Value::Null).changes.expect("changes map");
    assert_eq!(
        changes.keys().collect::<Vec<_>>(),
        [&included],
        "an excluded document gets no edits: {changes:?}"
    );
}

#[test]
fn source_fix_all_skips_excluded_documents() {
    let directory = tempfile::tempdir().expect("tempdir");
    let (included, excluded) = project_with_excluded_directory(directory.path());

    let mut client = Client::start();
    client.open(&included, "local x = nil\n");
    client.open(&excluded, "local x = nil\n");

    let excluded_actions = client
        .request::<CodeActionRequest>(fix_all_params(&excluded))
        .expect("the document is open");
    assert!(excluded_actions.is_empty(), "{excluded_actions:?}");
    let included_actions = client
        .request::<CodeActionRequest>(fix_all_params(&included))
        .expect("the document is open");
    assert_eq!(included_actions.len(), 1, "{included_actions:?}");
}

#[test]
fn source_fix_all_stops_once_a_config_change_excludes_the_document() {
    let directory = tempfile::tempdir().expect("tempdir");
    let (_included, excluded) = project_with_excluded_directory(directory.path());
    let config_path = directory.path().join("luck.json");
    fs::write(
        &config_path,
        r#"{ "lint": { "categories": { "correctness": "off" }, "rules": { "redundant_nil_init": "warn" } } }"#,
    )
    .expect("rewrite luck.json without the exclude");

    let mut client = Client::start();
    client.open(&excluded, "local x = nil\n");
    let before = client
        .request::<CodeActionRequest>(fix_all_params(&excluded))
        .expect("the document is open");
    assert_eq!(before.len(), 1, "not excluded yet: {before:?}");

    fs::write(
        &config_path,
        r#"{ "exclude": ["gen/**"], "lint": { "categories": { "correctness": "off" }, "rules": { "redundant_nil_init": "warn" } } }"#,
    )
    .expect("rewrite luck.json with the exclude");
    client.file_changed(&config_path, FileChangeType::CHANGED);

    let after = client
        .request::<CodeActionRequest>(fix_all_params(&excluded))
        .expect("the document is open");
    assert!(after.is_empty(), "{after:?}");
}

/// The form VS Code sends on Windows, `file:///c%3A/...`, unlike the
/// `file:///C%3A/...` that `Uri::from_file_path` builds.
#[cfg(windows)]
fn vscode_windows_uri(path: &Path) -> Uri {
    use std::str::FromStr as _;

    let slashed = path.to_str().expect("UTF-8 temp path").replace('\\', "/");
    let (drive, rest) = slashed.split_once(':').expect("a drive letter");
    Uri::from_str(&format!("file:///{}%3A{rest}", drive.to_ascii_lowercase()))
        .expect("hand-rolled URI is valid")
}

#[cfg(windows)]
#[test]
fn a_vscode_windows_uri_finds_its_project_settings() {
    let directory = tempfile::tempdir().expect("tempdir");
    project_with_excluded_directory(directory.path());
    let included = vscode_windows_uri(&directory.path().join("kept.luau"));
    let excluded = vscode_windows_uri(&directory.path().join("gen").join("a.luau"));

    let mut client = Client::start();
    client.open(&included, "local x = nil\n");
    client.open(&excluded, "local x = nil\n");
    let included_diags = client.diagnostics(&included, Some(1));
    assert!(
        included_diags
            .iter()
            .any(|diag| diag.code == Some(NumberOrString::String("redundant_nil_init".to_owned()))),
        "luck.json's rules apply: {included_diags:?}"
    );
    assert!(client.diagnostics(&excluded, Some(1)).is_empty(), "luck.json's exclude applies");
}

#[test]
fn rename_refuses_a_configured_extra_global() {
    let directory = tempfile::tempdir().expect("tempdir");
    fs::write(directory.path().join("luck.json"), r#"{ "lint": { "extra_globals": ["app"] } }"#)
        .expect("write luck.json");
    let path = directory.path().join("main.luau");
    fs::write(&path, "local count = 1\n").expect("write main.luau");
    let uri = Uri::from_file_path(&path).expect("file path URI");

    let mut client = Client::start();
    client.open(&uri, "local count = 1\n");

    let refusal = rename_refusal(&mut client, &uri, "app").expect("`app` is configured");
    assert!(refusal.contains("`app` would shadow"), "{refusal}");
    assert_eq!(rename_refusal(&mut client, &uri, "total"), None);
}

#[test]
fn watched_config_change_reseeds_open_documents_with_extra_globals() {
    let directory = tempfile::tempdir().expect("tempdir");
    let config_path = directory.path().join("luck.json");
    fs::write(&config_path, "{}").expect("write luck.json");
    let path = directory.path().join("main.luau");
    fs::write(&path, "local count = 1\n").expect("write main.luau");
    let uri = Uri::from_file_path(&path).expect("file path URI");

    let mut client = Client::start();
    client.open(&uri, "local count = 1\n");
    assert_eq!(rename_refusal(&mut client, &uri, "app"), None, "`app` isn't configured yet");

    fs::write(&config_path, r#"{ "lint": { "extra_globals": ["app"] } }"#)
        .expect("rewrite luck.json");
    client.file_changed(&config_path, FileChangeType::CHANGED);
    assert!(rename_refusal(&mut client, &uri, "app").is_some(), "the config change took effect");
}

#[test]
fn a_std_definition_reaches_lints_and_hover_and_an_edit_to_it_reseeds_them() {
    let app = r#"
[globals.app]
kind = "namespace"

[globals.app.members.net]
kind = "namespace"

[globals.app.members.net.members.send]
kind = "function"
params = [{ kind = "string" }, { kind = "number" }, { kind = "number" }]
"#;
    let directory = tempfile::tempdir().expect("tempdir");
    fs::write(directory.path().join("luck.json"), r#"{ "std": ["app.toml"] }"#)
        .expect("write luck.json");
    let std_path = directory.path().join("app.toml");
    fs::write(&std_path, app).expect("write app.toml");
    let path = directory.path().join("main.luau");
    let text = "app.net.send('hello')\n";
    fs::write(&path, text).expect("write main.luau");
    let uri = Uri::from_file_path(&path).expect("file path URI");

    let mut client = Client::start();
    client.open(&uri, text);
    let diags = client.diagnostics(&uri, Some(1));
    let arity = "`app.net.send` requires at least 3 argument(s), got 1";
    assert!(diags.iter().any(|diag| diag.message == arity), "{diags:?}");
    let hover = client
        .request::<HoverRequest>(HoverParams {
            text_document_position_params: position_params(&uri, 0, 10),
            work_done_progress_params: WorkDoneProgressParams::default(),
        })
        .expect("cursor on app.net.send should hover");
    let HoverContents::Markup(markup) = hover.contents else {
        panic!("hover should return markup contents");
    };
    assert!(markup.value.contains("app.net.send"), "{}", markup.value);

    fs::write(&std_path, app.replace(", { kind = \"number\" }, { kind = \"number\" }", ""))
        .expect("rewrite app.toml");
    client.file_changed(&std_path, FileChangeType::CHANGED);
    let reseeded = client.diagnostics(&uri, Some(1));
    assert!(reseeded.iter().all(|diag| diag.message != arity), "{reseeded:?}");
}

#[test]
fn a_change_sent_right_after_the_open_is_not_lost() {
    let mut client = Client::start();
    let uri = workspace_uri("quick.luau");
    client.open(&uri, "local before = 1\n");
    client.replace_all(&uri, 2, "local after = 2\n");
    assert_eq!(document_symbols(&mut client, &uri), ["after"]);
}

#[test]
fn a_close_leaves_no_diagnostics() {
    let mut client = Client::start();
    let uri = workspace_uri("closed.luau");
    client.open(&uri, "local x =\n");
    client.replace_all(&uri, 2, "local y =\n");
    client.notify::<DidCloseTextDocument>(DidCloseTextDocumentParams {
        text_document: document(&uri),
    });
    // Whatever version published before the close, the close clears it.
    loop {
        let published = client.next_published(&uri);
        if published.version.is_none() {
            assert!(published.diagnostics.is_empty(), "{published:?}");
            break;
        }
    }
    let leftovers = workspace_symbols(&mut client, "");
    assert!(leftovers.is_empty(), "a closed document isn't searched: {leftovers:?}");
    let late = client
        .notifications()
        .iter()
        .filter(|notification| notification.method == "textDocument/publishDiagnostics")
        .count();
    assert_eq!(late, 0, "nothing publishes after the close: {:?}", client.notifications());
}

#[test]
fn publishes_follow_the_versions_in_order() {
    let mut client = Client::start();
    let uri = workspace_uri("versions.luau");
    client.open(&uri, "local one = 1\n");
    client.replace_all(&uri, 2, "local two = 2\n");
    client.replace_all(&uri, 3, "local three = 3\n");
    let mut versions = Vec::new();
    while versions.last() != Some(&Some(3)) {
        versions.push(client.next_published(&uri).version);
    }
    let mut sorted = versions.clone();
    sorted.sort_unstable();
    assert_eq!(versions, sorted, "no version publishes after a newer one: {versions:?}");
    assert_eq!(document_symbols(&mut client, &uri), ["three"]);
}
