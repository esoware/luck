use std::fs;
use std::path::Path;

use ls_types::notification::DidChangeWorkspaceFolders;
use ls_types::request::{GotoDefinition, PrepareRenameRequest, Rename};
use ls_types::{
    DidChangeWorkspaceFoldersParams, FileChangeType, GotoDefinitionParams, GotoDefinitionResponse,
    Location, NumberOrString, PartialResultParams, Position, PrepareRenameResponse, Range,
    RenameParams, Uri, WorkDoneProgressParams, WorkspaceEdit, WorkspaceFolder,
    WorkspaceFoldersChangeEvent,
};
use tempfile::TempDir;

use crate::common::{Client, position_params, references};

const UTIL: &str = "\
local M = {}
function M.greet(name)
  return \"hi \" .. name
end
M.shout = function() return M.greet(\"!\") end
return M
";

const MAIN: &str = "\
local util = require(\"util\")
util.greet(\"you\")
print(util.shout())
";

/// A project of `files`, each written under the returned directory, with a
/// `luck.json` that pins its root, so no config above the temp directory
/// decides how it resolves.
fn project(files: &[(&str, &str)]) -> TempDir {
    let directory = tempfile::tempdir().expect("tempdir");
    fs::write(directory.path().join("luck.json"), "{}").expect("write luck.json");
    for (name, text) in files {
        fs::write(directory.path().join(name), text).expect("write a file");
    }
    directory
}

fn uri_of(directory: &Path, name: &str) -> Uri {
    Uri::from_file_path(directory.join(name)).expect("file path URI")
}

fn file_name(uri: &Uri) -> &str {
    uri.path().as_str().rsplit('/').next().unwrap()
}

/// Each location as its file's name and its start.
fn places(locations: &[Location]) -> Vec<(&str, Position)> {
    let mut found: Vec<(&str, Position)> =
        locations.iter().map(|location| (file_name(&location.uri), location.range.start)).collect();
    found.sort_unstable();
    found
}

fn rename(
    client: &mut Client,
    uri: &Uri,
    position: Position,
    new_name: &str,
) -> Result<WorkspaceEdit, String> {
    client
        .request_result::<Rename>(RenameParams {
            text_document_position: position_params(uri, position.line, position.character),
            new_name: new_name.to_owned(),
            work_done_progress_params: WorkDoneProgressParams::default(),
        })
        .map(|edit| edit.expect("a member is renameable"))
        .map_err(|refusal| refusal.message)
}

/// Each edit as its file's name, its start, and its text.
fn edits(edit: &WorkspaceEdit) -> Vec<(&str, Position, &str)> {
    let mut found: Vec<(&str, Position, &str)> = edit
        .changes
        .iter()
        .flatten()
        .flat_map(|(uri, edits)| {
            edits.iter().map(|text_edit| {
                (file_name(uri), text_edit.range.start, text_edit.new_text.as_str())
            })
        })
        .collect();
    found.sort_unstable();
    found
}

#[test]
fn a_member_goes_to_where_its_module_defines_it() {
    let directory = project(&[("util.lua", UTIL), ("main.lua", MAIN)]);
    let main = uri_of(directory.path(), "main.lua");
    let mut client = Client::start_in(directory.path());
    client.open(&main, MAIN);

    let response = client
        .request::<GotoDefinition>(GotoDefinitionParams {
            text_document_position_params: position_params(&main, 1, 7),
            work_done_progress_params: WorkDoneProgressParams::default(),
            partial_result_params: PartialResultParams::default(),
        })
        .expect("`greet` is defined in `util.lua`");
    let GotoDefinitionResponse::Scalar(location) = response else {
        panic!("one definition: {response:?}");
    };
    assert_eq!(file_name(&location.uri), "util.lua");
    assert_eq!(location.range, Range::new(Position::new(1, 11), Position::new(1, 16)));
}

#[test]
fn a_member_s_references_include_files_that_are_not_open() {
    let directory = project(&[("util.lua", UTIL), ("main.lua", MAIN)]);
    let util = uri_of(directory.path(), "util.lua");
    let mut client = Client::start_in(directory.path());
    client.open(&util, UTIL);

    let found = references(&mut client, &util, Position::new(1, 12), true);
    assert_eq!(
        places(&found),
        [
            ("main.lua", Position::new(1, 5)),
            ("util.lua", Position::new(1, 11)),
            ("util.lua", Position::new(4, 30)),
        ]
    );
    let reads = references(&mut client, &util, Position::new(1, 12), false);
    assert_eq!(places(&reads).len(), 2, "the declaration is left out: {reads:?}");
}

#[test]
fn a_member_renames_in_every_file_that_names_it() {
    let directory = project(&[("util.lua", UTIL), ("main.lua", MAIN)]);
    let main = uri_of(directory.path(), "main.lua");
    let mut client = Client::start_in(directory.path());
    client.open(&main, MAIN);

    let prepared = client
        .request::<PrepareRenameRequest>(position_params(&main, 1, 7))
        .expect("a member of a workspace module is renameable");
    let PrepareRenameResponse::RangeWithPlaceholder { range, placeholder } = prepared else {
        panic!("expected a range with a placeholder");
    };
    assert_eq!((range.start, placeholder.as_str()), (Position::new(1, 5), "greet"));

    let edit = rename(&mut client, &main, Position::new(1, 7), "welcome").expect("it renames");
    assert_eq!(
        edits(&edit),
        [
            ("main.lua", Position::new(1, 5), "welcome"),
            ("util.lua", Position::new(1, 11), "welcome"),
            ("util.lua", Position::new(4, 30), "welcome"),
        ]
    );
    let refusal = rename(&mut client, &main, Position::new(1, 7), "shout").unwrap_err();
    assert!(refusal.contains("already a member"), "{refusal}");
}

#[test]
fn a_luau_export_renames_in_its_module_and_its_readers() {
    let version = "\
export local version = 1
export function describe() return version end
";
    let main = "\
local info = require(\"./version\")
print(info.version, info.describe())
";
    let directory = project(&[("version.luau", version), ("main.luau", main)]);
    let main_uri = uri_of(directory.path(), "main.luau");
    let mut client = Client::start_in(directory.path());
    client.open_as(&main_uri, "luau", main);

    let edit = rename(&mut client, &main_uri, Position::new(1, 12), "release").expect("renames");
    assert_eq!(
        edits(&edit),
        [
            ("main.luau", Position::new(1, 11), "release"),
            ("version.luau", Position::new(0, 13), "release"),
            ("version.luau", Position::new(1, 34), "release"),
        ]
    );
    let refusal = rename(&mut client, &main_uri, Position::new(1, 12), "print").unwrap_err();
    assert!(refusal.contains("built-in global"), "an export is a local of its module: {refusal}");
}

#[test]
fn a_member_of_a_module_outside_the_workspace_is_not_renamed() {
    let directory = project(&[("util.lua", UTIL), ("main.lua", MAIN)]);
    let main = uri_of(directory.path(), "main.lua");
    let mut client = Client::start();
    client.open(&main, MAIN);

    let prepared = client.request::<PrepareRenameRequest>(position_params(&main, 1, 7));
    assert!(prepared.is_none(), "{prepared:?}");
    let refusal = rename(&mut client, &main, Position::new(1, 7), "welcome").unwrap_err();
    assert!(refusal.contains("outside the workspace"), "{refusal}");
}

#[test]
fn a_file_changed_on_disk_is_read_again() {
    let directory = project(&[("util.lua", UTIL), ("main.lua", MAIN)]);
    let util = uri_of(directory.path(), "util.lua");
    let mut client = Client::start_in(directory.path());
    client.open(&util, UTIL);
    assert_eq!(references(&mut client, &util, Position::new(1, 12), false).len(), 2);

    let main_path = directory.path().join("main.lua");
    fs::write(&main_path, format!("{MAIN}util.greet(\"again\")\n")).expect("rewrite main.lua");
    client.file_changed(&main_path, FileChangeType::CHANGED);
    assert_eq!(references(&mut client, &util, Position::new(1, 12), false).len(), 3);

    let other_path = directory.path().join("other.lua");
    fs::write(&other_path, "require(\"util\").greet()\n").expect("write other.lua");
    client.file_changed(&other_path, FileChangeType::CREATED);
    let found = references(&mut client, &util, Position::new(1, 12), false);
    assert!(places(&found).contains(&("other.lua", Position::new(0, 16))), "{found:?}");
}

#[test]
fn a_folder_added_to_the_workspace_is_walked() {
    let directory = project(&[("util.lua", UTIL)]);
    let sub = directory.path().join("sub");
    fs::create_dir_all(&sub).expect("mkdir sub");
    fs::write(sub.join("luck.json"), r#"{ "search_paths": ["../?.lua"] }"#).expect("luck.json");
    fs::write(sub.join("main.lua"), MAIN).expect("write sub/main.lua");
    let util = uri_of(directory.path(), "util.lua");
    let mut client = Client::start();
    client.open(&util, UTIL);
    assert_eq!(references(&mut client, &util, Position::new(1, 12), false).len(), 1);

    client.notify::<DidChangeWorkspaceFolders>(DidChangeWorkspaceFoldersParams {
        event: WorkspaceFoldersChangeEvent {
            added: vec![WorkspaceFolder {
                uri: Uri::from_file_path(directory.path()).expect("file path URI"),
                name: "added".to_owned(),
            }],
            removed: Vec::new(),
        },
    });
    let found = references(&mut client, &util, Position::new(1, 12), false);
    assert!(places(&found).contains(&("main.lua", Position::new(1, 5))), "{found:?}");
}

/// The walk loads each config on a thread of its own, which a nesting past
/// the parser's limit doesn't overflow, so the server reports the config as
/// every other path that loads it does.
#[test]
fn the_walk_reports_a_config_nested_past_the_limit() {
    let directory = tempfile::tempdir().expect("tempdir");
    let config_path = directory.path().join("luck.json");
    let nesting = 100_000;
    let config = format!(
        r#"{{ "lint": {{ "rules": {{ "cyclomatic_complexity": ["warn", {{ "max": 1 }}, {}{}] }} }} }}"#,
        "[".repeat(nesting),
        "]".repeat(nesting)
    );
    fs::write(&config_path, config).expect("write luck.json");
    fs::write(directory.path().join("main.lua"), "print(1)\n").expect("write main.lua");

    let mut client = Client::start_in(directory.path());
    let config_uri = Uri::from_file_path(&config_path).expect("file path URI");
    let diags = client.diagnostics(&config_uri, None);
    let [diag] = diags.as_slice() else { panic!("one config error: {diags:?}") };
    assert_eq!(diag.message, "invalid config: arrays and objects nest more than 128 levels deep");
    client.shut_down().expect("the server is still up");
}

#[test]
fn a_require_that_does_not_resolve_is_reported_until_its_module_exists() {
    let text = "\
local missing = require(\"missing\")
print(missing)
";
    let directory = project(&[("main.lua", text)]);
    let main = uri_of(directory.path(), "main.lua");
    let mut client = Client::start();
    client.open(&main, text);

    let diags = client.diagnostics(&main, Some(1));
    let [diag] = diags.as_slice() else { panic!("one unresolved require: {diags:?}") };
    assert_eq!(diag.code, Some(NumberOrString::String("E004".to_owned())));
    assert_eq!(diag.message.lines().next(), Some("module not found: `missing`"));
    assert_eq!(diag.range, Range::new(Position::new(0, 24), Position::new(0, 33)));

    let missing = directory.path().join("missing.lua");
    fs::write(&missing, "return {}\n").expect("write missing.lua");
    client.file_changed(&missing, FileChangeType::CREATED);
    let republished = client.next_published(&main);
    assert_eq!(republished.version, Some(1));
    assert!(republished.diagnostics.is_empty(), "{:?}", republished.diagnostics);
}
