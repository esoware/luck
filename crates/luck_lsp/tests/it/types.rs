use std::fs;
use std::path::Path;

use ls_types::request::{
    Completion, GotoDefinition, HoverRequest, Request as LspRequest, SignatureHelpRequest,
};
use ls_types::{
    CompletionParams, CompletionResponse, Diagnostic, Documentation, GotoDefinitionParams,
    GotoDefinitionResponse, Hover, HoverContents, HoverParams, Location, NumberOrString,
    ParameterLabel, PartialResultParams, Position, SignatureHelpParams, Uri,
    WorkDoneProgressParams,
};
use serde::{Deserialize, Serialize};
use tempfile::TempDir;

use crate::common::{Client, position_params, workspace_uri};

/// `workspace/textDocumentContent`, which `ls_types` doesn't have.
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

const APP: &str = "\
-- Sends `text` to the server.
declare function send(text: string, port: number): ()
declare app: {
    net: {
        -- Opens a connection.
        open: (host: string) -> (),
    },
}
";

/// A project that loads `definitions` as `app.d.luau`, with `main.luau`
/// holding `main`, and the URI of `main.luau`.
fn project(definitions: &str, main: &str) -> (TempDir, Uri) {
    let directory = tempfile::tempdir().expect("tempdir");
    fs::write(directory.path().join("app.d.luau"), definitions).expect("write app.d.luau");
    fs::write(directory.path().join("luck.json"), r#"{ "definitions": ["app.d.luau"] }"#)
        .expect("write luck.json");
    let uri = file(directory.path(), "main.luau", main);
    (directory, uri)
}

fn roblox_project(main: &str) -> (TempDir, Uri) {
    let directory = tempfile::tempdir().expect("tempdir");
    fs::write(directory.path().join("luck.json"), r#"{ "definitions": ["@roblox"] }"#)
        .expect("write luck.json");
    let uri = file(directory.path(), "main.luau", main);
    (directory, uri)
}

fn file(directory: &Path, name: &str, text: &str) -> Uri {
    let path = directory.join(name);
    fs::write(&path, text).expect("write a module");
    Uri::from_file_path(&path).expect("file path URI")
}

fn hover(client: &mut Client, uri: &Uri, line: u32, utf16_column: u32) -> Hover {
    client
        .request::<HoverRequest>(HoverParams {
            text_document_position_params: position_params(uri, line, utf16_column),
            work_done_progress_params: WorkDoneProgressParams::default(),
        })
        .expect("something to hover")
}

fn markdown(hovered: &Hover) -> &str {
    let HoverContents::Markup(markup) = &hovered.contents else {
        panic!("hover should return markup contents");
    };
    &markup.value
}

fn definition(client: &mut Client, uri: &Uri, line: u32, utf16_column: u32) -> Location {
    let found = client
        .request::<GotoDefinition>(GotoDefinitionParams {
            text_document_position_params: position_params(uri, line, utf16_column),
            work_done_progress_params: WorkDoneProgressParams::default(),
            partial_result_params: PartialResultParams::default(),
        })
        .expect("a definition");
    let GotoDefinitionResponse::Scalar(location) = found else {
        panic!("one location: {found:?}");
    };
    location
}

fn type_errors(diags: &[Diagnostic]) -> Vec<&str> {
    let e015 = Some(NumberOrString::String("E015".to_owned()));
    diags.iter().filter(|diag| diag.code == e015).map(|diag| diag.message.as_str()).collect()
}

const MISMATCH: &str = "Expected this to be 'number', but got 'string'";

#[test]
fn type_errors_publish_with_lints_and_follow_an_edit() {
    let mut client = Client::start();
    let uri = workspace_uri("typed.luau");
    client.open(
        &uri,
        "\
--!strict
local unused = 1
local count: number = \"one\"
return count
",
    );

    let diags = client.diagnostics(&uri, Some(1));
    assert_eq!(type_errors(&diags), [MISMATCH], "{diags:?}");
    let error = diags.iter().find(|diag| diag.message == MISMATCH).expect("the type error");
    assert_eq!(error.range.start, Position::new(2, 22));
    let unused = Some(NumberOrString::String("unused_variable".to_owned()));
    assert!(diags.iter().any(|diag| diag.code == unused), "the lints publish too: {diags:?}");

    client.replace_all(
        &uri,
        2,
        "\
--!strict
local count: number = 1
return count
",
    );
    let fixed = client.diagnostics(&uri, Some(2));
    assert!(type_errors(&fixed).is_empty(), "{fixed:?}");
}

#[test]
fn a_change_drops_the_check_of_the_version_it_replaces() {
    let mut client = Client::start();
    let uri = workspace_uri("replaced.luau");
    // Luau takes a while to check a function of many parameters.
    let params: Vec<String> = (0..4_000).map(|i| format!("a{i}")).collect();
    let slow = format!(
        "\
--!strict
local function f({}) end
local count: number = \"one\"
return f, count
",
        params.join(", ")
    );
    client.open(&uri, &slow);
    client.replace_all(
        &uri,
        2,
        "\
--!strict
local count: number = 1
return count
",
    );

    let first = client.next_published(&uri);
    assert_eq!(first.version, Some(2), "the replaced version publishes nothing");
    assert!(type_errors(&first.diagnostics).is_empty(), "{:?}", first.diagnostics);
}

#[test]
fn hover_shows_the_type_at_the_cursor() {
    let mut client = Client::start();
    let uri = workspace_uri("local.luau");
    client.open(
        &uri,
        "\
local count = 1
return count
",
    );

    let hovered = hover(&mut client, &uri, 1, 9);

    assert_eq!(
        markdown(&hovered),
        "\
```luau
number
```"
    );
    let range = hovered.range.expect("a range");
    assert_eq!((range.start, range.end), (Position::new(1, 7), Position::new(1, 12)));
}

#[test]
fn hover_documents_a_declaration_of_the_project_s_definitions() {
    let main = "\
send(\"hi\", 1)
app.net.open(\"host\")
";
    let (_directory, uri) = project(APP, main);
    let mut client = Client::start();
    client.open(&uri, main);

    let sent = hover(&mut client, &uri, 0, 1);
    assert_eq!(
        markdown(&sent),
        "\
```luau
(text: string, port: number) -> ()
```

Sends `text` to the server."
    );
    let opened = hover(&mut client, &uri, 1, 9);
    assert!(
        markdown(&opened).ends_with(
            "\
```

Opens a connection."
        ),
        "{}",
        markdown(&opened)
    );
}

#[test]
fn hover_documents_roblox_s_api() {
    let main = "local players = game:GetService(\"Players\")\n";
    let (_directory, uri) = roblox_project(main);
    let mut client = Client::start();
    client.open(&uri, main);

    let hovered = hover(&mut client, &uri, 0, 23);

    assert_eq!(
        markdown(&hovered),
        "\
```luau
(self: ServiceProvider, className: string) -> Instance
```

Returns the service with the requested class name, creating it if it does not exist."
    );
}

#[test]
fn completion_offers_luau_s_entries_with_types_and_documentation() {
    let main = "app.net.\n";
    let (_directory, uri) = project(APP, main);
    let mut client = Client::start();
    client.open(&uri, main);

    let response = client
        .request::<Completion>(CompletionParams {
            text_document_position: position_params(&uri, 0, 8),
            work_done_progress_params: WorkDoneProgressParams::default(),
            partial_result_params: PartialResultParams::default(),
            context: None,
        })
        .expect("completions");
    let CompletionResponse::Array(items) = response else {
        panic!("completion should return an array response");
    };

    let open = items.iter().find(|item| item.label == "open").expect("`open`");
    assert_eq!(open.detail.as_deref(), Some("(host: string) -> ()"));
    let Some(Documentation::MarkupContent(documentation)) = &open.documentation else {
        panic!("{open:?}");
    };
    assert_eq!(documentation.value, "Opens a connection.");
}

#[test]
fn signature_help_reads_the_callee_s_type() {
    let main = "send(\"hi\", 1)\n";
    let (_directory, uri) = project(APP, main);
    let mut client = Client::start();
    client.open(&uri, main);

    let help = client
        .request::<SignatureHelpRequest>(SignatureHelpParams {
            context: None,
            text_document_position_params: position_params(&uri, 0, 11),
            work_done_progress_params: WorkDoneProgressParams::default(),
        })
        .expect("signature help");

    let signature = &help.signatures[0];
    assert_eq!(signature.label, "send(text: string, port: number): ()");
    assert_eq!(help.active_parameter, Some(1));
    let params = signature.parameters.as_ref().expect("parameters");
    let labels: Vec<&str> = params
        .iter()
        .map(|param| {
            let ParameterLabel::LabelOffsets([start, end]) = param.label else {
                panic!("{param:?}");
            };
            &signature.label[start as usize..end as usize]
        })
        .collect();
    assert_eq!(labels, ["text: string", "port: number"]);
    let Some(Documentation::MarkupContent(documentation)) = &signature.documentation else {
        panic!("{signature:?}");
    };
    assert_eq!(documentation.value, "Sends `text` to the server.");
}

#[test]
fn goto_definition_finds_a_declaration_in_the_project_s_definition_file() {
    let main = "send(\"hi\", 1)\n";
    let (_directory, uri) = project(APP, main);
    let mut client = Client::start();
    client.open(&uri, main);

    let location = definition(&mut client, &uri, 0, 1);

    assert!(location.uri.path().as_str().ends_with("/app.d.luau"), "{:?}", location.uri);
    assert_eq!(location.range.start, Position::new(1, 17));
}

#[test]
fn goto_definition_serves_luau_s_own_definitions_under_its_scheme() {
    let mut client = Client::start();
    let uri = workspace_uri("print.luau");
    client.open(&uri, "print(1)\n");

    let location = definition(&mut client, &uri, 0, 2);
    assert_eq!(location.uri.as_str(), "luck:/luau.luau");
    let served = client
        .request::<TextDocumentContent>(TextDocumentContentParams { uri: location.uri.clone() });

    let line = served.text.lines().nth(location.range.start.line as usize).expect("the line");
    assert!(line.starts_with("declare function print"), "{line}");
}

#[test]
fn a_required_module_s_exported_type_reaches_the_requirer() {
    let directory = tempfile::tempdir().expect("tempdir");
    let point = "\
export type Point = { x: number, y: number }
return {}
";
    let point_uri = file(directory.path(), "point.luau", point);
    let main = "\
--!strict
local Point = require(\"./point\")
local origin: Point.Point = { x = 0, y = \"zero\" }
return origin
";
    let main_uri = file(directory.path(), "main.luau", main);
    let mut client = Client::start();
    client.open(&main_uri, main);

    let read_from_disk = client.diagnostics(&main_uri, Some(1));
    assert_eq!(type_errors(&read_from_disk).len(), 1, "{read_from_disk:?}");

    client.open(&point_uri, &point.replace("y: number", "y: string"));
    loop {
        let published = client.next_published(&main_uri);
        if type_errors(&published.diagnostics).is_empty() {
            break;
        }
    }
}
