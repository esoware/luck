//! A client that holds every file itself, as an editor in a browser does, and
//! gives its config as `initializationOptions`, with nothing on disk.

use std::str::FromStr as _;

use ls_types::notification::{DidChangeConfiguration, DidCloseTextDocument};
use ls_types::request::{Formatting, HoverRequest, Rename};
use ls_types::{
    Diagnostic, DidChangeConfigurationParams, DidCloseTextDocumentParams, DocumentFormattingParams,
    FormattingOptions, HoverContents, HoverParams, MessageType, NumberOrString, Position, Range,
    RenameParams, Uri, WorkDoneProgressParams,
};
use serde_json::{Value, json};

use crate::common::{Client, definition, document, position_params};

/// The URI an editor gives `name` in its own scheme.
fn uri(name: &str) -> Uri {
    Uri::from_str(&format!("inmemory://model/{name}")).expect("a valid URI")
}

fn folder() -> Uri {
    Uri::from_str("inmemory://model/").expect("a valid URI")
}

fn given_config() -> Value {
    json!({ "definitions": ["@roblox", "app.d.luau"], "security": "RobloxScriptSecurity" })
}

const DEFINITIONS: &str = "\
-- Shows `message` for `seconds` seconds.
declare function alert(message: string, seconds: number): ()
";

const UTIL: &str = "\
local M = {}
function M.greet(name: string): string
  return \"hi \" .. name
end
return M
";

const MAIN: &str = "\
--!strict
local util = require(\"./util\")
alert(util.greet(\"you\"), \"eight\")
print(game.PlaceId)
";

/// A client of `given_config` with its definition file and `util.luau`
/// pushed, and `MAIN` open in the editor's first model.
fn started() -> (Client, Uri) {
    let client = Client::start_given(given_config(), &folder());
    client.push(&[(&uri("app.d.luau"), Some(DEFINITIONS)), (&uri("util.luau"), Some(UTIL))]);
    let main = uri("1");
    client.open(&main, MAIN);
    (client, main)
}

fn codes(diags: &[Diagnostic]) -> Vec<&str> {
    diags
        .iter()
        .filter_map(|diag| match &diag.code {
            Some(NumberOrString::String(code)) => Some(code.as_str()),
            _ => None,
        })
        .collect()
}

/// The next diagnostics published for `uri` with a type error, which go out
/// after the lints.
fn checked(client: &mut Client, uri: &Uri, version: Option<i32>) -> Vec<Diagnostic> {
    client.diagnostics_where(uri, version, |diags| codes(diags).contains(&"E015"))
}

/// The next diagnostics published for `uri` with one whose message is
/// `message`.
fn wait_for(client: &mut Client, uri: &Uri, message: &str) -> Vec<Diagnostic> {
    loop {
        let published = client.next_published(uri);
        if published.diagnostics.iter().any(|diag| diag.message == message) {
            return published.diagnostics;
        }
    }
}

#[test]
fn a_document_reads_the_pushed_definitions_and_modules() {
    let (mut client, main) = started();

    let diags = checked(&mut client, &main, Some(1));

    assert_eq!(codes(&diags), ["E015"], "only the wrong argument: {diags:?}");
    assert_eq!(diags[0].message, "Expected this to be 'number', but got 'string'");
    assert_eq!(diags[0].range, Range::new(Position::new(2, 25), Position::new(2, 32)));
}

#[test]
fn hover_documents_a_global_the_pushed_definition_file_declares() {
    let (mut client, main) = started();

    let hovered = client
        .request::<HoverRequest>(HoverParams {
            text_document_position_params: position_params(&main, 2, 1),
            work_done_progress_params: WorkDoneProgressParams::default(),
        })
        .expect("something to hover");

    let HoverContents::Markup(markup) = hovered.contents else {
        panic!("markup: {hovered:?}");
    };
    assert_eq!(
        markup.value,
        "\
```luau
(message: string, seconds: number) -> ()
```

Shows `message` for `seconds` seconds."
    );
}

#[test]
fn go_to_definition_reaches_the_pushed_files() {
    let (mut client, main) = started();

    let declared = definition(&mut client, &main, 2, 1);
    assert_eq!(declared.uri, uri("app.d.luau"));
    assert_eq!(declared.range.start, Position::new(1, 17));

    let member = definition(&mut client, &main, 2, 12);
    assert_eq!(member.uri, uri("util.luau"));
    assert_eq!(member.range, Range::new(Position::new(1, 11), Position::new(1, 16)));

    let module = definition(&mut client, &main, 1, 24);
    assert_eq!(module.uri, uri("util.luau"));
}

#[test]
fn a_member_renames_across_the_pushed_files() {
    let (mut client, main) = started();

    let edit = client
        .request::<Rename>(RenameParams {
            text_document_position: position_params(&main, 2, 12),
            new_name: "welcome".to_owned(),
            work_done_progress_params: WorkDoneProgressParams::default(),
        })
        .expect("a member renames");

    let mut edits: Vec<(String, Position)> = edit
        .changes
        .into_iter()
        .flatten()
        .flat_map(|(uri, edits)| {
            edits.into_iter().map(move |text_edit| (uri.as_str().to_owned(), text_edit.range.start))
        })
        .collect();
    edits.sort();
    assert_eq!(
        edits,
        [
            ("inmemory://model/1".to_owned(), Position::new(2, 11)),
            ("inmemory://model/util.luau".to_owned(), Position::new(1, 11)),
        ]
    );
}

#[test]
fn a_pushed_definition_file_s_update_reaches_the_documents() {
    let (mut client, main) = started();
    assert_eq!(codes(&checked(&mut client, &main, Some(1))), ["E015"]);

    client.push(&[(
        &uri("app.d.luau"),
        Some(&DEFINITIONS.replace("seconds: number", "seconds: string")),
    )]);

    loop {
        let published = client.next_published(&main);
        if published.diagnostics.is_empty() {
            break;
        }
    }
}

#[test]
fn a_deleted_module_no_longer_resolves() {
    let (mut client, main) = started();
    assert_eq!(codes(&checked(&mut client, &main, Some(1))), ["E015"]);

    client.push(&[(&uri("util.luau"), None)]);

    loop {
        let published = client.next_published(&main);
        let e004 = Some(NumberOrString::String("E004".to_owned()));
        if let Some(missing) = published.diagnostics.iter().find(|diag| diag.code == e004) {
            assert_eq!(missing.range, Range::new(Position::new(1, 21), Position::new(1, 29)));
            break;
        }
    }
}

#[test]
fn a_config_luau_s_aliases_reach_requires() {
    let client = Client::start_given(json!({}), &folder());
    client.push(&[
        (&uri(".config.luau"), Some("return { luau = { aliases = { lib = \"./lib\" } } }")),
        (&uri("lib/util.luau"), Some(UTIL)),
    ]);
    let main = uri("1");
    client.open(
        &main,
        "\
--!strict
local util = require(\"@lib/util\")
local count: number = util.greet(\"you\")
return count
",
    );
    let mut client = client;

    let diags = checked(&mut client, &main, Some(1));

    assert_eq!(codes(&diags), ["E015"], "the alias resolves, so `greet` is typed: {diags:?}");
    assert_eq!(diags[0].message, "Expected this to be 'number', but got 'string'");
}

#[test]
fn an_invalid_config_is_shown_and_a_new_one_replaces_it() {
    let mut client = Client::start_given(json!({ "definitions": ["app.luau"] }), &folder());
    client.push(&[(&uri("app.d.luau"), Some(DEFINITIONS))]);
    let main = uri("1");
    client.open(&main, "alert(\"print(1)\", 8)\n");
    let diags = client.diagnostics(&main, Some(1));
    assert!(codes(&diags).contains(&"undefined_variable"), "the defaults lack `alert`: {diags:?}");
    assert_eq!(
        client.shown(MessageType::ERROR),
        ["luck: the config the client gave doesn't load, so its files use the defaults: invalid \
          config: `definitions` names `app.luau`, but a definition file's name ends in \
          `.d.luau`"]
    );

    client.notify::<DidChangeConfiguration>(DidChangeConfigurationParams {
        settings: json!({ "definitions": ["app.d.luau"] }),
    });

    loop {
        let published = client.next_published(&main);
        if published.diagnostics.is_empty() {
            break;
        }
    }
}

#[test]
fn an_open_definition_file_parses_and_formats_as_one() {
    let client = Client::start_given(given_config(), &folder());
    let definitions = uri("app.d.luau");
    let unformatted = "declare   function alert(message:string,seconds:number):()\n";
    client.push(&[(&definitions, Some(unformatted))]);
    client.open(&definitions, unformatted);
    let mut client = client;

    let diags = client.diagnostics(&definitions, Some(1));
    assert!(diags.is_empty(), "`declare` is no syntax error, and no rule reads it: {diags:?}");

    let edits = client
        .request::<Formatting>(DocumentFormattingParams {
            text_document: document(&definitions),
            options: FormattingOptions { tab_size: 4, insert_spaces: false, ..Default::default() },
            work_done_progress_params: WorkDoneProgressParams::default(),
        })
        .expect("the file formats");
    assert_eq!(edits[0].new_text, "declare function alert(message: string, seconds: number): ()\n");
}

#[test]
fn a_definition_file_luau_cannot_load_is_reported_on_it() {
    let client = Client::start_given(json!({ "definitions": ["bad.d.luau"] }), &folder());
    let bad = uri("bad.d.luau");
    client.push(&[(&bad, Some("declare function make(): Missing\n"))]);
    client.open(&uri("1"), "make()\n");
    let mut client = client;

    let diags = client.diagnostics(&bad, None);

    assert_eq!(codes(&diags), ["E015"], "{diags:?}");
    assert_eq!(diags[0].message, "Unknown type 'Missing'");
    assert_eq!(diags[0].range, Range::new(Position::new(0, 25), Position::new(0, 32)));
}

#[test]
fn a_config_luau_that_never_returns_is_stopped() {
    let client = Client::start_given(json!({}), &folder());
    client.push(&[
        (&uri(".config.luau"), Some("while true do end\n")),
        (&uri("lib/util.luau"), Some(UTIL)),
    ]);
    let main = uri("1");
    client.open(&main, "local util = require(\"@lib/util\")\nreturn util\n");
    let mut client = client;

    let diags = client.diagnostics(&main, Some(1));

    assert_eq!(codes(&diags), ["E004"], "{diags:?}");
    assert!(diags[0].message.contains(".config.luau`, which doesn't load"), "{diags:?}");
}

#[test]
fn a_pushed_config_luau_s_change_reaches_the_checks() {
    let client = Client::start_given(json!({}), &folder());
    let config = uri(".config.luau");
    client.push(&[(&config, Some("return { luau = { languagemode = \"nocheck\" } }"))]);
    let main = uri("1");
    client.open(
        &main,
        "\
local count: number = \"one\"
return count
",
    );
    let mut client = client;
    let diags = client.diagnostics(&main, Some(1));
    assert!(!codes(&diags).contains(&"E015"), "nothing is checked: {diags:?}");

    client.push(&[(&config, Some("return { luau = { languagemode = \"strict\" } }"))]);

    loop {
        let published = client.next_published(&main);
        if codes(&published.diagnostics).contains(&"E015") {
            break;
        }
    }
}

/// Each change to the open definition file reloads what the checker
/// declares, and closing it goes back to the file pushed under it.
#[test]
fn an_open_definition_file_s_changes_reach_the_checks() {
    let client = Client::start_given(json!({ "definitions": ["app.d.luau"] }), &folder());
    let definitions = uri("app.d.luau");
    client.push(&[(&definitions, Some(DEFINITIONS))]);
    let main = uri("1");
    client.open(
        &main,
        "\
--!strict
alert(\"hi\", \"eight\")
",
    );
    let mut client = client;
    wait_for(&mut client, &main, "Expected this to be 'number', but got 'string'");

    client.open(&definitions, "declare function alert(message: string, seconds: Missing): ()\n");
    let rejected = loop {
        let published = client.next_published(&definitions);
        assert_ne!(published.version, None, "only the document's own publish carries it");
        if !published.diagnostics.is_empty() {
            break published.diagnostics;
        }
    };
    assert_eq!(codes(&rejected), ["E015"], "{rejected:?}");
    assert_eq!(rejected[0].message, "Unknown type 'Missing'");
    wait_for(&mut client, &main, "Unknown global 'alert'; consider assigning to it first");

    client.replace_all(
        &definitions,
        2,
        "declare function alert(message: string, seconds: boolean): ()\n",
    );
    client.diagnostics_where(&definitions, Some(2), <[Diagnostic]>::is_empty);
    wait_for(&mut client, &main, "Expected this to be 'boolean', but got 'string'");

    client.notify::<DidCloseTextDocument>(DidCloseTextDocumentParams {
        text_document: document(&definitions),
    });
    assert_eq!(client.next_published(&definitions).diagnostics, []);
    wait_for(&mut client, &main, "Expected this to be 'number', but got 'string'");
}

/// A definition file the config lists that the client never pushed is there
/// while it's open, so the project loads it once it opens, as it would one
/// open before the project loaded, and is gone once it closes.
#[test]
fn a_listed_definition_file_that_opens_after_the_project_loaded_reaches_the_checks() {
    let client = Client::start_given(json!({ "definitions": ["app.d.luau"] }), &folder());
    let main = uri("1");
    client.open(
        &main,
        "\
--!strict
alert(\"hi\", \"eight\")
",
    );
    let mut client = client;
    let missing = "Unknown global 'alert'; consider assigning to it first";
    let unloaded = wait_for(&mut client, &main, missing);
    assert!(codes(&unloaded).contains(&"undefined_variable"), "{unloaded:?}");

    let definitions = uri("app.d.luau");
    client.open(&definitions, DEFINITIONS);
    let loaded = wait_for(&mut client, &main, "Expected this to be 'number', but got 'string'");
    assert_eq!(codes(&loaded), ["E015"], "{loaded:?}");

    client.replace_all(
        &definitions,
        2,
        &DEFINITIONS.replace("seconds: number", "seconds: boolean"),
    );
    wait_for(&mut client, &main, "Expected this to be 'boolean', but got 'string'");

    client.notify::<DidCloseTextDocument>(DidCloseTextDocumentParams {
        text_document: document(&definitions),
    });
    let closed = wait_for(&mut client, &main, missing);
    assert!(codes(&closed).contains(&"undefined_variable"), "{closed:?}");
}

/// A listed definition file that doesn't parse as the project loads leaves
/// the rest of the project loaded, and its declarations arrive once its open
/// document parses.
#[test]
fn a_listed_definition_file_that_does_not_parse_adds_nothing_until_it_does() {
    let client =
        Client::start_given(json!({ "definitions": ["@roblox", "app.d.luau"] }), &folder());
    let definitions = uri("app.d.luau");
    client.open(&definitions, "declare function alert(message: string, seconds: number: ()\n");
    let main = uri("1");
    client.open(
        &main,
        "\
--!strict
alert(game.Name, \"eight\")
",
    );
    let mut client = client;
    let broken = client.diagnostics(&definitions, Some(1));
    assert!(!broken.is_empty(), "the syntax error is on the file");
    assert!(codes(&broken).iter().all(|code| *code == "E008"), "{broken:?}");
    let unloaded =
        wait_for(&mut client, &main, "Unknown global 'alert'; consider assigning to it first");
    let lint = Some(NumberOrString::String("undefined_variable".to_owned()));
    let undefined: Vec<Position> =
        unloaded.iter().filter(|diag| diag.code == lint).map(|diag| diag.range.start).collect();
    assert_eq!(undefined, [Position::new(1, 0)], "`alert` alone, as `@roblox` loads: {unloaded:?}");

    client.replace_all(&definitions, 2, DEFINITIONS);

    client.diagnostics_where(&definitions, Some(2), <[Diagnostic]>::is_empty);
    let loaded = wait_for(&mut client, &main, "Expected this to be 'number', but got 'string'");
    assert_eq!(codes(&loaded), ["E015"], "{loaded:?}");
}

/// A global an open definition file declares, and its pushed file doesn't,
/// is known to the lints and the checker alike.
#[test]
fn an_open_definition_file_s_globals_reach_the_lints_with_the_types() {
    fn wait_until_known(client: &mut Client, uri: &Uri, is_known: bool) {
        loop {
            let diags = client.next_published(uri).diagnostics;
            let is_linted = codes(&diags).contains(&"undefined_variable");
            let is_typed =
                diags.iter().any(|diag| diag.message.starts_with("Unknown global 'notify'"));
            if is_linted != is_known && is_typed != is_known {
                return;
            }
        }
    }

    let client = Client::start_given(json!({ "definitions": ["app.d.luau"] }), &folder());
    let definitions = uri("app.d.luau");
    client.push(&[(&definitions, Some(DEFINITIONS))]);
    let main = uri("1");
    client.open(
        &main,
        "\
--!strict
notify(\"hi\")
",
    );
    client.open(&definitions, DEFINITIONS);
    let mut client = client;
    wait_until_known(&mut client, &main, false);

    let declared = format!("{DEFINITIONS}declare function notify(message: string): ()\n");
    client.replace_all(&definitions, 2, &declared);
    wait_until_known(&mut client, &main, true);

    client.replace_all(&definitions, 3, DEFINITIONS);
    wait_until_known(&mut client, &main, false);
}

#[test]
fn a_deleted_definition_file_luau_cannot_load_has_its_problems_cleared() {
    let client = Client::start_given(json!({ "definitions": ["bad.d.luau"] }), &folder());
    let bad = uri("bad.d.luau");
    client.push(&[(&bad, Some("declare function make(): Missing\n"))]);
    client.open(&uri("1"), "make()\n");
    let mut client = client;
    assert_eq!(codes(&client.diagnostics(&bad, None)), ["E015"]);

    client.push(&[(&bad, None)]);

    assert_eq!(client.next_published(&bad).diagnostics, []);
}

#[test]
fn a_deleted_definition_file_that_does_not_parse_has_its_problems_cleared() {
    let client = Client::start_given(json!({ "definitions": ["broken.d.luau"] }), &folder());
    let broken = uri("broken.d.luau");
    client.push(&[(&broken, Some("declare function make(: ()\n"))]);
    client.open(&uri("1"), "make()\n");
    let mut client = client;
    let published = client.diagnostics(&broken, None);
    assert!(!published.is_empty(), "the syntax errors are on the file");
    assert!(codes(&published).iter().all(|code| *code == "E008"), "{published:?}");

    client.push(&[(&broken, None)]);

    assert_eq!(client.next_published(&broken).diagnostics, []);
}
