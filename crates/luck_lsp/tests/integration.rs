//! End-to-end tests that drive the `LanguageServer` trait directly via the
//! `CapturedNotifier` shim. This exercises the full backend pipeline -
//! document-store updates, lint runs, formatter calls, position mapping -
//! without going through a stdio transport, which would add framing overhead
//! and obscure assertions.

use std::str::FromStr;

use luck_lsp::backend::{Backend, CapturedNotifier};
use tower_lsp::LanguageServer;
use tower_lsp::lsp_types::{
    CodeActionContext, CodeActionOrCommand, CodeActionParams, CompletionItem, CompletionParams,
    CompletionResponse, DidChangeTextDocumentParams, DidOpenTextDocumentParams,
    DocumentFormattingParams, DocumentHighlightKind, DocumentHighlightParams, DocumentLinkParams,
    DocumentRangeFormattingParams, DocumentSymbolParams, DocumentSymbolResponse,
    FoldingRangeParams, FormattingOptions, GotoDefinitionParams, GotoDefinitionResponse,
    HoverContents, HoverParams, InitializeParams, PartialResultParams, Position,
    PrepareRenameResponse, Range, ReferenceContext, ReferenceParams, RenameParams,
    SelectionRangeParams, SemanticTokensParams, SemanticTokensRangeParams,
    SemanticTokensRangeResult, SemanticTokensResult, SignatureHelpParams,
    TextDocumentContentChangeEvent, TextDocumentIdentifier, TextDocumentItem,
    TextDocumentPositionParams, Url, VersionedTextDocumentIdentifier, WorkDoneProgressParams,
    WorkspaceSymbolParams,
};

fn workspace_uri(name: &str) -> Url {
    // `file:///` URIs work cross-platform for tests that never hit disk.
    Url::from_str(&format!("file:///tmp/{name}")).expect("hand-rolled URI is valid")
}

fn position_params(uri: &Url, line: u32, character: u32) -> TextDocumentPositionParams {
    TextDocumentPositionParams {
        text_document: TextDocumentIdentifier { uri: uri.clone() },
        position: Position { line, character },
    }
}

async fn open(server: &Backend<CapturedNotifier>, uri: &Url, text: &str) {
    server
        .did_open(DidOpenTextDocumentParams {
            text_document: TextDocumentItem {
                uri: uri.clone(),
                language_id: "lua".to_string(),
                version: 1,
                text: text.to_string(),
            },
        })
        .await;
}

fn formatting_options() -> FormattingOptions {
    FormattingOptions {
        tab_size: 4,
        insert_spaces: false,
        ..Default::default()
    }
}

#[tokio::test]
async fn initialize_advertises_required_capabilities() {
    let server = Backend::new(CapturedNotifier::default());
    let result = server
        .initialize(InitializeParams::default())
        .await
        .expect("initialize succeeds");
    let caps = result.capabilities;
    assert!(
        caps.document_formatting_provider.is_some(),
        "formatting capability missing"
    );
    assert!(
        caps.document_range_formatting_provider.is_some(),
        "range formatting capability missing"
    );
    assert!(
        caps.text_document_sync.is_some(),
        "text sync capability missing"
    );
}

#[tokio::test]
async fn format_on_save_returns_expected_edits() {
    let notifier = CapturedNotifier::default();
    let server = Backend::new(notifier.clone());
    let uri = workspace_uri("ugly.lua");
    // Ugly source: extra spaces, no consistent indentation.
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

    assert!(
        !edits.is_empty(),
        "ugly source should produce at least one edit"
    );
    let new_text = &edits[0].new_text;
    assert!(
        new_text.contains("local x = 1"),
        "formatter normalizes spacing: {new_text}"
    );
    assert!(
        new_text.contains("local y = 2"),
        "formatter normalizes spacing: {new_text}"
    );
    assert_ne!(new_text.as_str(), ugly, "formatter must change ugly source");
}

#[tokio::test]
async fn lint_diagnostics_are_opt_in_without_config() {
    let notifier = CapturedNotifier::default();
    let server = Backend::new(notifier.clone());
    let uri = workspace_uri("unused.lua");
    // No `luck.json` is discoverable for `/tmp/unused.lua`, so linting is
    // opt-out by default: lint rules are disabled and only parse errors would
    // surface. An unused variable must therefore NOT be flagged.
    open(&server, &uri, "local unused = 1\n").await;

    let diags = notifier.diagnostics_for(&uri).await;
    assert!(
        diags.is_empty(),
        "lint rules must be off without a lint config: {diags:?}"
    );
}

#[tokio::test]
async fn parse_errors_surface_without_config() {
    let notifier = CapturedNotifier::default();
    let server = Backend::new(notifier.clone());
    let uri = workspace_uri("broken.lua");
    // Even with linting opt-out, parse errors still surface as diagnostics.
    open(&server, &uri, "local x =\n").await;

    let diags = notifier.diagnostics_for(&uri).await;
    assert!(
        !diags.is_empty(),
        "parse errors must surface even when lint rules are disabled"
    );
}

#[tokio::test]
async fn range_formatting_preserves_out_of_range_content() {
    // The backend emits a single whole-document edit whose text reformats only
    // the requested range and leaves out-of-range bytes verbatim. This asserts
    // that preservation property (not that the edit's span is narrowed).
    let notifier = CapturedNotifier::default();
    let server = Backend::new(notifier.clone());
    let uri = workspace_uri("partial.lua");
    // Three statements. We will request formatting of only the middle line.
    // First and third must remain byte-identical in the output.
    let source = "local first=1\nlocal middle=2\nlocal last=3\n";
    open(&server, &uri, source).await;

    // Range covers only line 1 (the middle statement).
    let edits = server
        .range_formatting(DocumentRangeFormattingParams {
            text_document: TextDocumentIdentifier { uri: uri.clone() },
            range: Range {
                start: Position {
                    line: 1,
                    character: 0,
                },
                end: Position {
                    line: 1,
                    character: 15,
                },
            },
            options: formatting_options(),
            work_done_progress_params: WorkDoneProgressParams::default(),
        })
        .await
        .expect("range formatting errored")
        .expect("formatter returned no edits");
    assert!(!edits.is_empty(), "range formatter produced no edits");
    let new_text = &edits[0].new_text;
    // The middle line must be reformatted.
    assert!(
        new_text.contains("local middle = 2"),
        "middle line must be reformatted: {new_text}"
    );
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
    assert!(
        result.is_none(),
        "formatting an unopened document should return None"
    );
}

#[tokio::test]
async fn shutdown_succeeds() {
    let server = Backend::new(CapturedNotifier::default());
    server.shutdown().await.expect("shutdown errored");
}

// --- provider coverage ---------------------------------------------------

#[tokio::test]
async fn hover_renders_stdlib_signature() {
    let server = Backend::new(CapturedNotifier::default());
    let uri = workspace_uri("hover.lua");
    // Cursor on `format` in `string.format`.
    open(
        &server,
        &uri,
        "local formatted = string.format(\"%d\", 1)\n",
    )
    .await;

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
    open(
        &server,
        &uri,
        "local function greet(name)\n    return name\nend\n",
    )
    .await;

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
    open(
        &server,
        &uri,
        "local function greet(name)\n    return name\nend\n",
    )
    .await;

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
        ranges
            .iter()
            .any(|range| range.start_line == 0 && range.end_line >= 2),
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
    assert!(
        highlights.len() >= 2,
        "`count` occurs multiple times: {highlights:?}"
    );
    assert!(
        highlights
            .iter()
            .any(|hl| hl.kind == Some(DocumentHighlightKind::WRITE)),
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
            positions: vec![Position {
                line: 0,
                character: 20,
            }],
        })
        .await
        .expect("selection range errored")
        .expect("selection range should return a chain");
    // The chain nests from the widest enclosing node down to the narrowest.
    let root = &ranges[0];
    let mut deepest = root;
    let mut depth = 1;
    while let Some(parent) = deepest.parent.as_ref() {
        deepest = parent;
        depth += 1;
    }
    assert!(
        depth >= 2,
        "selection should expand through at least one enclosing node: {root:?}"
    );
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
    assert_eq!(
        tokens.data.len(),
        5,
        "every identifier should tokenize: {:?}",
        tokens.data
    );
}

#[tokio::test]
async fn document_link_resolves_require_path() {
    let dir = tempfile::tempdir().expect("tempdir");
    // A neighbouring module the `require` should resolve to.
    std::fs::write(dir.path().join("foo.lua"), "return {}\n").expect("write foo.lua");
    let main_path = dir.path().join("main.lua");
    std::fs::write(&main_path, "local foo = require(\"foo\")\n").expect("write main.lua");
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
    assert!(
        target.path().ends_with("foo.lua"),
        "link should resolve to foo.lua: {target}"
    );
}

#[tokio::test]
async fn code_action_offers_lint_fix() {
    let dir = tempfile::tempdir().expect("tempdir");
    // Opt in to linting and enable a fixable style rule.
    std::fs::write(
        dir.path().join("luck.json"),
        r#"{ "lint": { "rule_overrides": { "redundant_nil_init": { "enabled": true } } } }"#,
    )
    .expect("write luck.json");
    let file_path = dir.path().join("nilinit.lua");
    std::fs::write(&file_path, "local x = nil\n").expect("write lua file");
    let uri = Url::from_file_path(&file_path).expect("file path URI");

    let notifier = CapturedNotifier::default();
    let server = Backend::new(notifier.clone());
    open(&server, &uri, "local x = nil\n").await;

    // The enabled rule should have surfaced a diagnostic on open.
    let diagnostics = notifier.diagnostics_for(&uri).await;
    assert!(
        !diagnostics.is_empty(),
        "the enabled lint rule should publish a diagnostic"
    );

    let actions = server
        .code_action(CodeActionParams {
            text_document: TextDocumentIdentifier { uri: uri.clone() },
            range: Range {
                start: Position {
                    line: 0,
                    character: 0,
                },
                end: Position {
                    line: 0,
                    character: 13,
                },
            },
            context: CodeActionContext {
                diagnostics,
                only: None,
                trigger_kind: None,
            },
            work_done_progress_params: WorkDoneProgressParams::default(),
            partial_result_params: PartialResultParams::default(),
        })
        .await
        .expect("code action errored")
        .expect("a fixable diagnostic should yield code actions");
    let titles: Vec<&str> = actions
        .iter()
        .map(|action| match action {
            CodeActionOrCommand::CodeAction(a) => a.title.as_str(),
            CodeActionOrCommand::Command(c) => c.title.as_str(),
        })
        .collect();
    assert!(
        titles
            .iter()
            .any(|title| title.contains("drop redundant `= nil` initializer")),
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
    open(
        &server,
        &uri,
        "local x = 1\ndo\n    local x = 2\n    print(x)\nend\nprint(x)\n",
    )
    .await;

    // Cursor on the INNER x read at line 3.
    let locations = server
        .references(ReferenceParams {
            text_document_position: position_params(&uri, 3, 10),
            work_done_progress_params: WorkDoneProgressParams::default(),
            partial_result_params: PartialResultParams::default(),
            context: ReferenceContext {
                include_declaration: true,
            },
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
            context: ReferenceContext {
                include_declaration: false,
            },
        })
        .await
        .expect("references errored")
        .expect("local should have references");
    assert!(
        locations
            .iter()
            .all(|location| location.range.start.line == 1),
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

    let on_global = server
        .prepare_rename(position_params(&uri, 1, 2))
        .await
        .expect("prepare rename errored");
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
            new_name: "total".to_string(),
            work_done_progress_params: WorkDoneProgressParams::default(),
        })
        .await
        .expect("rename errored")
        .expect("rename should produce an edit");
    let edits = edit
        .changes
        .expect("changes map")
        .remove(&uri)
        .expect("edits for the document");
    assert_eq!(edits.len(), 3, "declaration + two references: {edits:?}");
    assert!(edits.iter().all(|edit| edit.new_text == "total"));
}

#[tokio::test]
async fn rename_rejects_keywords_and_used_names() {
    let server = Backend::new(CapturedNotifier::default());
    let uri = workspace_uri("rename_reject.lua");
    open(&server, &uri, "local count = 1\nlocal other = count\n").await;

    let keyword = server
        .rename(RenameParams {
            text_document_position: position_params(&uri, 0, 8),
            new_name: "end".to_string(),
            work_done_progress_params: WorkDoneProgressParams::default(),
        })
        .await;
    assert!(keyword.is_err(), "keywords are not identifiers");

    let captured = server
        .rename(RenameParams {
            text_document_position: position_params(&uri, 0, 8),
            new_name: "other".to_string(),
            work_done_progress_params: WorkDoneProgressParams::default(),
        })
        .await;
    assert!(captured.is_err(), "in-use names must be rejected");

    let builtin = server
        .rename(RenameParams {
            text_document_position: position_params(&uri, 0, 8),
            new_name: "print".to_string(),
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
    open(
        &server,
        &uri,
        "local s = \"\u{1F980}\" local target = 1\nprint(s, target)\n",
    )
    .await;

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
    open(
        &server,
        &first,
        "local function setup_camera() end\nsetup_camera()\n",
    )
    .await;
    open(
        &server,
        &second,
        "local function teardown() end\nteardown()\n",
    )
    .await;

    let symbols = server
        .symbol(WorkspaceSymbolParams {
            query: "camera".to_string(),
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

    let full = server
        .semantic_tokens_full(SemanticTokensParams {
            text_document: TextDocumentIdentifier { uri: uri.clone() },
            work_done_progress_params: WorkDoneProgressParams::default(),
            partial_result_params: PartialResultParams::default(),
        })
        .await
        .expect("semantic tokens errored")
        .expect("tokens expected");
    let SemanticTokensResult::Tokens(full) = full else {
        panic!("expected full tokens");
    };

    let ranged = server
        .semantic_tokens_range(SemanticTokensRangeParams {
            text_document: TextDocumentIdentifier { uri: uri.clone() },
            range: Range {
                start: Position {
                    line: 1,
                    character: 0,
                },
                end: Position {
                    line: 1,
                    character: 11,
                },
            },
            work_done_progress_params: WorkDoneProgressParams::default(),
            partial_result_params: PartialResultParams::default(),
        })
        .await
        .expect("semantic tokens range errored")
        .expect("tokens expected");
    let SemanticTokensRangeResult::Tokens(ranged) = ranged else {
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
async fn did_change_applies_multi_change_batch_with_shifted_ranges() {
    let server = Backend::new(CapturedNotifier::default());
    let uri = workspace_uri("batch.lua");
    open(&server, &uri, "local a = 1\nlocal b = 2\nlocal c = 3").await;

    // A single batch whose first edit inserts a line - the second edit's
    // range is expressed against the text the first one produced, so the
    // per-change line index must be refreshed between them.
    server
        .did_change(DidChangeTextDocumentParams {
            text_document: VersionedTextDocumentIdentifier {
                uri: uri.clone(),
                version: 2,
            },
            content_changes: vec![
                TextDocumentContentChangeEvent {
                    range: Some(Range {
                        start: Position {
                            line: 0,
                            character: 0,
                        },
                        end: Position {
                            line: 0,
                            character: 11,
                        },
                    }),
                    range_length: None,
                    text: "local a = 10\nlocal aa = 11".to_string(),
                },
                TextDocumentContentChangeEvent {
                    range: Some(Range {
                        start: Position {
                            line: 3,
                            character: 10,
                        },
                        end: Position {
                            line: 3,
                            character: 11,
                        },
                    }),
                    range_length: None,
                    text: "30".to_string(),
                },
            ],
        })
        .await;

    let text = server
        .document_text(&uri)
        .await
        .expect("document remains open after applying the batch");
    assert_eq!(
        text, "local a = 10\nlocal aa = 11\nlocal b = 2\nlocal c = 30",
        "second edit must land on the line shifted by the first"
    );
}
fn roblox_document(text: &str) -> luck_lsp::DocumentState {
    let target = luck_core::LuaTarget::LuauRoblox;
    let parsed = std::sync::Arc::new(luck_parser::parse(text, target.lua_version()));
    let semantic = std::sync::Arc::new(luck_semantic::analyze_with_environment(
        &parsed.block,
        target.lua_version(),
        target.stdlib_environment(),
    ));
    luck_lsp::DocumentState {
        text: text.to_string(),
        version: 1,
        target,
        line_index: luck_lsp::LineIndex::new(text),
        parsed,
        semantic,
    }
}

fn completion_at(doc: &luck_lsp::DocumentState, line: u32, character: u32) -> Vec<CompletionItem> {
    let params = CompletionParams {
        text_document_position: position_params(&workspace_uri("direct.luau"), line, character),
        work_done_progress_params: WorkDoneProgressParams::default(),
        partial_result_params: PartialResultParams::default(),
        context: None,
    };
    match luck_lsp::providers::completion::completion(doc, &params) {
        Some(CompletionResponse::Array(items)) => items,
        other => panic!("expected array completion, got {other:?}"),
    }
}

#[test]
fn get_service_string_completes_services_ranked() {
    let text = "local p = game:GetService('')";
    let doc = roblox_document(text);
    // Cursor between the quotes.
    let character = (text.find("('").expect("literal") + 2) as u32;
    let items = completion_at(&doc, 0, character);

    let players = items
        .iter()
        .find(|item| item.label == "Players")
        .expect("Players offered");
    assert!(
        players.sort_text.as_deref() == Some("0Players"),
        "common services rank first: {:?}",
        players.sort_text
    );
    let obscure = items
        .iter()
        .find(|item| item.label == "AccountService")
        .expect("full service list offered");
    assert!(obscure.sort_text.as_deref() == Some("1AccountService"));
    let dead = items
        .iter()
        .find(|item| item.label == "PointsService")
        .expect("dead services offered (tagged)");
    assert!(dead.sort_text.as_deref() == Some("2PointsService"));
    assert_eq!(dead.deprecated, Some(true));
    // Constant-value mode replaces identifier completion entirely.
    assert!(!items.iter().any(|item| item.label == "print"));
}

#[test]
fn instance_new_string_completes_class_names() {
    let text = "local f = Instance.new('Fol')";
    let doc = roblox_document(text);
    let character = (text.find("('").expect("literal") + 4) as u32;
    let items = completion_at(&doc, 0, character);
    assert!(items.iter().any(|item| item.label == "Folder"));
    assert!(
        !items.iter().any(|item| item.label == "BasePart"),
        "abstract classes are not creatable"
    );
}

#[test]
fn enum_dot_completes_enum_types() {
    let text = "local m = Enum.";
    let doc = roblox_document(text);
    let items = completion_at(&doc, 0, text.len() as u32);
    assert!(
        items.len() > 500,
        "expected the full generated enum tree, got {} items",
        items.len()
    );
    let material = items
        .iter()
        .find(|item| item.label == "Material")
        .expect("Enum.Material offered");
    assert_eq!(
        material.kind,
        Some(tower_lsp::lsp_types::CompletionItemKind::MODULE)
    );
}

#[test]
fn enum_type_dot_completes_items() {
    let text = "local m = Enum.Material.";
    let doc = roblox_document(text);
    let items = completion_at(&doc, 0, text.len() as u32);
    assert!(items.iter().any(|item| item.label == "Grass"));
    assert!(
        !items.iter().any(|item| item.label == "GetEnumItems"),
        "methods complete after ':', not '.'"
    );
}

#[test]
fn enum_type_colon_completes_methods_only() {
    let text = "local items = Enum.Material:";
    let doc = roblox_document(text);
    let items = completion_at(&doc, 0, text.len() as u32);
    assert!(items.iter().any(|item| item.label == "GetEnumItems"));
    assert!(items.iter().any(|item| item.label == "FromName"));
    assert!(
        !items.iter().any(|item| item.label == "Grass"),
        "items are dot members, not methods"
    );
}

#[test]
fn deprecated_enum_item_completion_is_tagged() {
    let text = "local a = Enum.AlignType.";
    let doc = roblox_document(text);
    let items = completion_at(&doc, 0, text.len() as u32);
    let parallel = items
        .iter()
        .find(|item| item.label == "Parallel")
        .expect("deprecated items still offered");
    assert_eq!(parallel.deprecated, Some(true));
}

#[test]
fn semantic_tokens_classify_nested_enum_item() {
    let text = "local m = Enum.Material.Grass";
    let doc = roblox_document(text);
    let SemanticTokensResult::Tokens(tokens) =
        luck_lsp::providers::semantic_tokens::semantic_tokens(&doc)
    else {
        panic!("expected full tokens");
    };
    // m, Enum, Material, Grass. The last token is `Grass`; it must be
    // classified as a stdlib property (defaultLibrary | readonly = 6),
    // not a plain property.
    let grass = tokens.data.last().expect("tokens present");
    assert_eq!(grass.length, "Grass".len() as u32);
    assert_eq!(grass.token_type, 2, "property: {:?}", tokens.data);
    assert_eq!(
        grass.token_modifiers_bitset & (1 << 2),
        1 << 2,
        "defaultLibrary modifier expected: {:?}",
        tokens.data
    );
}

#[test]
fn hover_on_enum_item_renders_constant() {
    let text = "local m = Enum.Material.Grass";
    let doc = roblox_document(text);
    // Cursor on `Grass`.
    let character = (text.find("Grass").expect("item") + 2) as u32;
    let params = HoverParams {
        text_document_position_params: position_params(&workspace_uri("direct.luau"), 0, character),
        work_done_progress_params: WorkDoneProgressParams::default(),
    };
    let hover = luck_lsp::providers::hover::hover(&doc, &params).expect("hover on enum item");
    let HoverContents::Markup(markup) = hover.contents else {
        panic!("hover should return markup contents");
    };
    assert!(
        markup.value.contains("Enum.Material.Grass"),
        "hover names the full path: {}",
        markup.value
    );
}

#[tokio::test]
async fn collectgarbage_option_string_completes() {
    let server = Backend::new(CapturedNotifier::default());
    let uri = workspace_uri("gc.lua");
    let text = "collectgarbage('c')";
    open(&server, &uri, text).await;

    let character = (text.find("('").expect("literal") + 2) as u32;
    let response = server
        .completion(CompletionParams {
            text_document_position: position_params(&uri, 0, character),
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

fn lua54_document(text: &str) -> luck_lsp::DocumentState {
    let target = luck_core::LuaTarget::Lua54;
    let parsed = std::sync::Arc::new(luck_parser::parse(text, target.lua_version()));
    let semantic = std::sync::Arc::new(luck_semantic::analyze_with_environment(
        &parsed.block,
        target.lua_version(),
        target.stdlib_environment(),
    ));
    luck_lsp::DocumentState {
        text: text.to_string(),
        version: 1,
        target,
        line_index: luck_lsp::LineIndex::new(text),
        parsed,
        semantic,
    }
}

fn signature_help_at(
    doc: &luck_lsp::DocumentState,
    line: u32,
    character: u32,
) -> Option<tower_lsp::lsp_types::SignatureHelp> {
    let params = SignatureHelpParams {
        text_document_position_params: position_params(
            &workspace_uri("direct.lua"),
            line,
            character,
        ),
        work_done_progress_params: WorkDoneProgressParams::default(),
        context: None,
    };
    luck_lsp::providers::signature_help::signature_help(doc, &params)
}

fn hover_at(doc: &luck_lsp::DocumentState, line: u32, character: u32) -> Option<String> {
    let params = HoverParams {
        text_document_position_params: position_params(
            &workspace_uri("direct.lua"),
            line,
            character,
        ),
        work_done_progress_params: WorkDoneProgressParams::default(),
    };
    let hover = luck_lsp::providers::hover::hover(doc, &params)?;
    let HoverContents::Markup(markup) = hover.contents else {
        panic!("hover should return markup contents");
    };
    Some(markup.value)
}

#[test]
fn signature_help_resolves_shaped_local_method() {
    let text = "local f = io.open('x')\nf:setvbuf('full', 512)";
    let doc = lua54_document(text);
    // Cursor inside the second argument.
    let help = signature_help_at(&doc, 1, 18).expect("file method signature help");
    let sig = &help.signatures[help.active_signature.unwrap_or(0) as usize];
    assert!(
        sig.label.starts_with("f:setvbuf("),
        "method rendering: {}",
        sig.label
    );
    assert_eq!(help.active_parameter, Some(1), "cursor is in arg 2");
}

#[test]
fn signature_help_marks_method_active_param_excluding_self() {
    let text = "local p = game:GetService('Players')";
    let doc = roblox_document(text);
    let character = (text.find("'Players'").expect("literal") + 3) as u32;
    let help = signature_help_at(&doc, 0, character).expect("GetService signature help");
    let sig = &help.signatures[help.active_signature.unwrap_or(0) as usize];
    assert!(
        sig.label.starts_with("game:GetService("),
        "label: {}",
        sig.label
    );
    // self is excluded: exactly one declared parameter, and the cursor
    // in the first paren argument selects it.
    assert_eq!(sig.parameters.as_ref().map(Vec::len), Some(1));
    assert_eq!(help.active_parameter, Some(0));
}

#[test]
fn signature_help_refuses_shadowed_base() {
    let text = "local string = 1\nstring.format('x')";
    let doc = lua54_document(text);
    assert!(
        signature_help_at(&doc, 1, 15).is_none(),
        "shadowed base must not show stdlib signatures"
    );
}

#[test]
fn signature_help_works_at_statement_position() {
    // Statement-position calls never pass through visit_expression;
    // the statement hook must find them.
    let text = "collectgarbage('collect')";
    let doc = lua54_document(text);
    let help = signature_help_at(&doc, 0, 16).expect("statement-position call");
    assert!(
        help.signatures
            .iter()
            .any(|sig| sig.label.starts_with("collectgarbage(")),
        "{help:?}"
    );
}

#[test]
fn hover_on_shaped_local_method_renders_colon_form() {
    let text = "local f = io.open('x')\nlocal n = f:seek('set')";
    let doc = lua54_document(text);
    let character = (text.find(":seek").expect("method") + 2) as u32;
    let markup = hover_at(&doc, 1, character).expect("hover on shaped local method");
    assert!(
        markup.contains("f:seek("),
        "colon rendering expected: {markup}"
    );
}

#[test]
fn hover_refuses_shadowed_namespace_member() {
    let text = "local string = 1\nlocal x = string.format";
    let doc = lua54_document(text);
    let character = (text.find(".format").expect("member") + 3) as u32;
    assert!(
        hover_at(&doc, 1, character).is_none(),
        "shadowed base must not hover stdlib docs"
    );
}

#[test]
fn hover_on_game_get_service_uses_colon_syntax() {
    let text = "local p = game:GetService('RunService')";
    let doc = roblox_document(text);
    let character = (text.find("GetService").expect("method") + 4) as u32;
    let markup = hover_at(&doc, 0, character).expect("hover on GetService");
    assert!(
        markup.contains("game:GetService("),
        "colon rendering expected: {markup}"
    );
}

#[test]
fn completion_after_shaped_local_colon_offers_file_methods() {
    let text = "local f = io.open('x')\nf:";
    let doc = lua54_document(text);
    let items = completion_at(&doc, 1, 2);
    assert!(
        items.iter().any(|item| item.label == "read"),
        "file methods expected: {:?}",
        items.iter().map(|i| &i.label).collect::<Vec<_>>()
    );
    assert!(items.iter().any(|item| item.label == "seek"));
}

#[test]
fn completion_after_shaped_local_colon_offers_datamodel_methods() {
    let text = "local g = game\ng:";
    let doc = roblox_document(text);
    let items = completion_at(&doc, 1, 2);
    assert!(
        items.iter().any(|item| item.label == "GetService"),
        "DataModel methods expected on shaped local"
    );
    assert!(
        items.iter().any(|item| item.label == "FindFirstChild"),
        "extends-inherited Instance methods expected"
    );
}

#[test]
fn completion_after_game_colon_offers_methods() {
    let text = "game:";
    let doc = roblox_document(text);
    let items = completion_at(&doc, 0, 5);
    assert!(items.iter().any(|item| item.label == "GetService"));
    assert!(
        !items.iter().any(|item| item.label == "Workspace"),
        "properties complete after '.', not ':'"
    );
}

#[test]
fn completion_ranks_deprecated_members_last() {
    let text = "local x = math.";
    let doc = lua54_document(text);
    let items = completion_at(&doc, 0, text.len() as u32);
    let floor = items
        .iter()
        .find(|item| item.label == "floor")
        .expect("live member");
    let pow = items
        .iter()
        .find(|item| item.label == "pow")
        .expect("deprecated compat member");
    assert_eq!(floor.sort_text.as_deref(), Some("1floor"));
    assert_eq!(pow.sort_text.as_deref(), Some("2pow"));
    assert_eq!(pow.deprecated, Some(true));
}

#[test]
fn semantic_tokens_classify_shaped_method_calls() {
    let text = "local f = io.open('x')\nlocal n = f:read('n')";
    let doc = lua54_document(text);
    let SemanticTokensResult::Tokens(tokens) =
        luck_lsp::providers::semantic_tokens::semantic_tokens(&doc)
    else {
        panic!("expected full tokens");
    };
    // `read` is the only identifier classified as a method here; it
    // must be METHOD (5) with defaultLibrary (1 << 2).
    let read = tokens
        .data
        .iter()
        .find(|token| token.length == 4 && token.token_type == 5)
        .unwrap_or_else(|| panic!("method token for read: {:?}", tokens.data));
    assert_eq!(
        read.token_modifiers_bitset & (1 << 2),
        1 << 2,
        "defaultLibrary expected on shaped method: {:?}",
        tokens.data
    );
}

#[test]
fn semantic_tokens_classify_get_service_method() {
    let text = "local p = game:GetService('RunService')";
    let doc = roblox_document(text);
    let SemanticTokensResult::Tokens(tokens) =
        luck_lsp::providers::semantic_tokens::semantic_tokens(&doc)
    else {
        panic!("expected full tokens");
    };
    let get_service = tokens
        .data
        .iter()
        .find(|token| token.length == "GetService".len() as u32)
        .expect("GetService token");
    assert_eq!(get_service.token_type, 5, "method: {:?}", tokens.data);
    assert_eq!(
        get_service.token_modifiers_bitset & (1 << 2),
        1 << 2,
        "defaultLibrary expected: {:?}",
        tokens.data
    );
}
